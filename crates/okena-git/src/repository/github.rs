//! GitHub API access for the PR/CI poll path: the base repository resolved
//! much as non-interactive `gh` resolves it, but with `origin` ahead of
//! `upstream` (see [`remote_name_score`]), the token from the sources `gh` reads
//! (env, then one cached `gh auth token` spawn), and REST/GraphQL calls over the
//! shared [`okena_transport::http`] bus so connections are reused.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gix::bstr::ByteSlice;
use okena_core::process::{command, safe_output_with_timeout};
use okena_transport::http::{self, HttpRequest, HttpResponse};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;

/// Per-request cap, matching the old per-`gh`-invocation timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a token from `gh auth token` is trusted before asking again.
const TOKEN_TTL: Duration = Duration::from_secs(30 * 60);
/// A missing token is re-checked this often so a fresh `gh auth login` is noticed.
const MISSING_TOKEN_TTL: Duration = Duration::from_secs(60);
const DEFAULT_HOST: &str = "github.com";

/// A GitHub repository: `owner/name` on a normalised `host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRepo {
    pub host: String,
    pub owner: String,
    pub name: String,
}

/// Whether the repo has any remote on a known GitHub host (https or ssh) —
/// see [`KnownHosts`].
///
/// Used to gate PR/CI polling: repos with no GitHub remote (local-only,
/// GitLab, Bitbucket, …) can never have GitHub PRs/checks, so every lookup
/// for them just fails after a round-trip. Skipping them is the bulk of the
/// poller's fan-out on a machine with many non-GitHub projects.
pub fn has_github_remote(path: &Path) -> bool {
    has_remote_on(path, &KnownHosts::current())
}

fn has_remote_on(path: &Path, known: &KnownHosts) -> bool {
    let Some(repo) = crate::gix_helpers::open(path) else {
        return false;
    };
    let names = repo.remote_names();
    for name in names.iter() {
        let Ok(remote) = repo.find_remote(name.as_ref()) else {
            continue;
        };
        for dir in [gix::remote::Direction::Fetch, gix::remote::Direction::Push] {
            if let Some(host) = remote.url(dir).and_then(|u| u.host())
                && known.contains(&normalize_github_host(host))
            {
                return true;
            }
        }
    }
    false
}

/// Hosts listed in Settings as GitHub Enterprise hosts, normalised. The daemon
/// installs them with [`set_enterprise_hosts`] whenever settings are stored.
static CONFIGURED_HOSTS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Replace the GitHub Enterprise hosts taken from Settings. Entries may be
/// bare hosts or URLs; anything past the host is ignored.
pub fn set_enterprise_hosts(hosts: &[String]) {
    let mut normalized: Vec<String> = Vec::new();
    for host in hosts.iter().filter_map(|entry| host_from_setting(entry)) {
        if !normalized.contains(&host) {
            normalized.push(host);
        }
    }
    *CONFIGURED_HOSTS.write() = normalized;
}

/// `github.acme.corp` from `github.acme.corp`, `https://github.acme.corp/`
/// or `git@github.acme.corp`.
fn host_from_setting(entry: &str) -> Option<String> {
    let entry = entry.trim();
    let rest = entry.split_once("://").map_or(entry, |(_, rest)| rest);
    let authority = rest.split(['/', ':']).next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty()).then(|| normalize_github_host(host))
}

/// The hosts okena treats as GitHub: github.com, any GHE.com tenant, the hosts
/// gh is logged into, `GH_HOST`, and the enterprise hosts listed in Settings.
#[derive(Debug, Default)]
struct KnownHosts {
    gh_host: Option<String>,
    logged_in: Vec<String>,
    configured: Vec<String>,
}

impl KnownHosts {
    fn current() -> Self {
        Self {
            gh_host: std::env::var("GH_HOST")
                .ok()
                .filter(|host| !host.trim().is_empty())
                .map(|host| normalize_github_host(&host)),
            logged_in: gh_logged_in_hosts(),
            configured: CONFIGURED_HOSTS.read().clone(),
        }
    }

    /// Whether the normalised `host` is a GitHub host.
    fn contains(&self, host: &str) -> bool {
        matches!(host_kind(host), HostKind::Dotcom | HostKind::Tenancy)
            || self.gh_host.as_deref() == Some(host)
            || self.logged_in.iter().any(|known| known == host)
            || self.configured.iter().any(|known| known == host)
    }
}

/// gh's config directory, per go-gh's `config.ConfigDir`: `GH_CONFIG_DIR`,
/// else `$XDG_CONFIG_HOME/gh`, else `%AppData%/GitHub CLI` on Windows, else
/// `~/.config/gh`.
fn gh_config_dir(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let var = |name: &str| var(name).filter(|value| !value.is_empty());
    if let Some(dir) = var("GH_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(dir).join("gh"));
    }
    if cfg!(windows)
        && let Some(dir) = var("AppData")
    {
        return Some(PathBuf::from(dir).join("GitHub CLI"));
    }
    dirs::home_dir().map(|home| home.join(".config").join("gh"))
}

/// The hosts a gh `hosts.yml` lists: its top-level keys.
fn parse_gh_hosts(yaml: &str) -> Vec<String> {
    yaml.lines()
        .filter(|line| !line.starts_with([' ', '\t', '#', '-']))
        .filter_map(|line| line.split_once(':').map(|(key, _)| key))
        .map(|key| key.trim().trim_matches(['"', '\'']))
        .filter(|key| !key.is_empty())
        .map(normalize_github_host)
        .collect()
}

/// `hosts.yml` as last read, by path and modification time.
struct GhHostsFile {
    path: PathBuf,
    modified: Option<SystemTime>,
    hosts: Vec<String>,
}

static GH_HOSTS: Mutex<Option<GhHostsFile>> = Mutex::new(None);

/// The hosts gh is logged into, from its `hosts.yml` — re-read only when the
/// file changes, so a `gh auth login --hostname` is picked up.
fn gh_logged_in_hosts() -> Vec<String> {
    let Some(path) = gh_config_dir(|name| std::env::var(name).ok()).map(|dir| dir.join("hosts.yml"))
    else {
        return Vec::new();
    };
    let modified = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    let mut cache = GH_HOSTS.lock();
    if let Some(file) = cache.as_ref()
        && file.path == path
        && file.modified == modified
    {
        return file.hosts.clone();
    }
    let hosts = modified
        .and_then(|_| std::fs::read_to_string(&path).ok())
        .map(|yaml| parse_gh_hosts(&yaml))
        .unwrap_or_default();
    *cache = Some(GhHostsFile {
        path,
        modified,
        hosts: hosts.clone(),
    });
    hosts
}

/// Suffix of the GHE.com tenancy hosts gh knows, such as `acme.ghe.com`.
const TENANCY_SUFFIX: &str = ".ghe.com";

/// Lowercase, with any `*.github.com` alias folded to `github.com` and any
/// subdomain of a GHE.com tenant (`api.acme.ghe.com`) folded to the tenant —
/// gh's `NormalizeHostname`.
pub fn normalize_github_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.ends_with(".github.com") {
        return DEFAULT_HOST.to_string();
    }
    if let Some(labels) = host.strip_suffix(TENANCY_SUFFIX)
        && !labels.is_empty()
    {
        let tenant = labels.rsplit('.').next().unwrap_or(labels);
        return format!("{tenant}{TENANCY_SUFFIX}");
    }
    host
}

/// How gh talks to a host: its API endpoints and which token env vars apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostKind {
    /// `github.com`.
    Dotcom,
    /// A GHE.com tenant, `<tenant>.ghe.com` — gh's "tenancy" host.
    Tenancy,
    /// Self-hosted GitHub Enterprise Server.
    Server,
}

/// The kind of a normalised host.
fn host_kind(host: &str) -> HostKind {
    if host == DEFAULT_HOST {
        HostKind::Dotcom
    } else if host.len() > TENANCY_SUFFIX.len() && host.ends_with(TENANCY_SUFFIX) {
        HostKind::Tenancy
    } else {
        HostKind::Server
    }
}

/// `owner/name@host` from a remote URL, per gh's `ghrepo.FromURL`: the first
/// two path segments, `.git` dropped. Handles `https://`, `ssh://` and the
/// scp-like `git@host:owner/name` form (gix parses all three).
fn repo_from_url(url: &gix::Url) -> Option<GithubRepo> {
    let host = url.host()?;
    let path = url.path.to_str().ok()?;
    let mut segments = path.trim_matches('/').split('/');
    let owner = segments.next().filter(|s| !s.is_empty())?;
    let name = segments.next().filter(|s| !s.is_empty())?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    Some(GithubRepo {
        host: normalize_github_host(host),
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// A git remote that maps to a GitHub repository, plus its `gh repo
/// set-default` marker (`remote.<name>.gh-resolved`) if any.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteCandidate {
    name: String,
    repo: GithubRepo,
    resolved: Option<String>,
}

fn remote_candidates(repo: &gix::Repository) -> Vec<RemoteCandidate> {
    let config = repo.config_snapshot();
    let names = repo.remote_names();
    let mut candidates = Vec::new();
    for name in names.iter() {
        let Ok(remote) = repo.find_remote(name.as_ref()) else {
            continue;
        };
        // gh maps the fetch URL and falls back to the push URL.
        let parsed = remote
            .url(gix::remote::Direction::Fetch)
            .or_else(|| remote.url(gix::remote::Direction::Push))
            .and_then(repo_from_url);
        let Some(parsed) = parsed else {
            continue;
        };
        let resolved = config
            .string(format!("remote.{name}.gh-resolved").as_str())
            .map(|value| value.to_string());
        candidates.push(RemoteCandidate {
            name: name.to_string(),
            repo: parsed,
            resolved,
        });
    }
    candidates
}

/// Remote priority: `origin` > `upstream` > `github` > anything else.
///
/// Not gh's order, which puts `upstream` first: on a fork, okena's branches
/// are pushed to `origin` and their pull requests opened there, so reading
/// `upstream` found none of them. `gh repo set-default` still wins over this.
fn remote_name_score(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "origin" => 3,
        "upstream" => 2,
        "github" => 1,
        _ => 0,
    }
}

/// `owner/name` or `host/owner/name`, as stored by `gh repo set-default`.
fn repo_from_full_name(full_name: &str, host: &str) -> Option<GithubRepo> {
    let parts: Vec<&str> = full_name.split('/').collect();
    let (owner, name) = match parts.as_slice() {
        [owner, name] => (*owner, *name),
        [_, owner, name] => (*owner, *name),
        _ => return None,
    };
    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some(GithubRepo {
        host: host.to_string(),
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// The base repository: only remotes on a GitHub host count, a `gh repo
/// set-default` choice wins, else the highest-priority remote name by
/// [`remote_name_score`] (ties keep `git remote` order).
fn select_base_repo(
    candidates: Vec<RemoteCandidate>,
    is_github_host: impl Fn(&str) -> bool,
) -> Option<GithubRepo> {
    let mut candidates: Vec<RemoteCandidate> = candidates
        .into_iter()
        .filter(|candidate| is_github_host(&candidate.repo.host))
        .collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(remote_name_score(&candidate.name)));
    for candidate in &candidates {
        match candidate.resolved.as_deref() {
            Some("base") => return Some(candidate.repo.clone()),
            Some(full_name) => return repo_from_full_name(full_name, &candidate.repo.host),
            None => {}
        }
    }
    candidates
        .into_iter()
        .next()
        .map(|candidate| candidate.repo)
}

/// The GitHub repository `gh` would run against for this checkout, among
/// remotes on any known GitHub host.
pub(crate) fn resolve_base_repo(path: &Path) -> Option<GithubRepo> {
    let repo = crate::gix_helpers::open(path)?;
    let known = KnownHosts::current();
    select_base_repo(remote_candidates(&repo), |host| known.contains(host))
}

/// The GitHub repository `gh` would run against for this checkout — the
/// repository its pull request links name, host included.
pub fn github_repo(path: &Path) -> Option<GithubRepo> {
    resolve_base_repo(path)
}

/// The key, lowercased, of the GitHub repository `gh` would run against for
/// this checkout — the key a repository's open pull requests are fetched and
/// shared under, so every checkout of it asks once. `owner/name` on
/// github.com, `host/owner/name` on any other host.
pub fn github_repo_key(path: &Path) -> Option<String> {
    resolve_base_repo(path).map(|repo| repo_key(&repo))
}

fn repo_key(repo: &GithubRepo) -> String {
    let key = if repo.host == DEFAULT_HOST {
        format!("{}/{}", repo.owner, repo.name)
    } else {
        format!("{}/{}/{}", repo.host, repo.owner, repo.name)
    };
    key.to_ascii_lowercase()
}

/// The GitHub repository behind `origin`, where this checkout's own branches
/// are pushed. A PR whose head lives anywhere else — a fork's branch that
/// happens to share a name — is not this checkout's.
pub(crate) fn origin_repo(path: &Path) -> Option<GithubRepo> {
    let repo = crate::gix_helpers::open(path)?;
    let remote = repo.find_remote("origin").ok()?;
    remote
        .url(gix::remote::Direction::Push)
        .or_else(|| remote.url(gix::remote::Direction::Fetch))
        .and_then(repo_from_url)
}

struct TokenEntry {
    token: Option<String>,
    fetched_at: Instant,
}

/// Process-wide token cache keyed by host: one `gh auth token` spawn per TTL
/// instead of one per query.
static TOKENS: Mutex<Option<HashMap<String, TokenEntry>>> = Mutex::new(None);

/// The env vars gh reads a token for `host` from, in order: gh's
/// `TokenFromEnvOrConfig`, where only a GHE Server host is "enterprise" — a
/// GHE.com tenant reads the same ones as github.com.
fn token_env_vars(host: &str) -> [&'static str; 2] {
    match host_kind(host) {
        HostKind::Dotcom | HostKind::Tenancy => ["GH_TOKEN", "GITHUB_TOKEN"],
        HostKind::Server => ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"],
    }
}

/// The env override gh itself honours before touching its keyring.
pub(crate) fn env_token(host: &str) -> Option<String> {
    token_env_vars(host).iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn gh_cli_token(host: &str) -> Option<String> {
    let output = safe_output_with_timeout(
        command("gh").args(["auth", "token", "--hostname", host]),
        REQUEST_TIMEOUT,
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

fn cached_token(
    cache: &mut HashMap<String, TokenEntry>,
    host: &str,
    fetch: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if let Some(entry) = cache.get(host) {
        let ttl = if entry.token.is_some() {
            TOKEN_TTL
        } else {
            MISSING_TOKEN_TTL
        };
        if entry.fetched_at.elapsed() < ttl {
            return entry.token.clone();
        }
    }
    let token = fetch();
    cache.insert(
        host.to_string(),
        TokenEntry {
            token: token.clone(),
            fetched_at: Instant::now(),
        },
    );
    token
}

/// Token for `host`: env first (as gh does), then the cached `gh auth token`.
/// The lock is held across the spawn so concurrent poll workers share one.
fn token_for(host: &str) -> Option<String> {
    if let Some(token) = env_token(host) {
        return Some(token);
    }
    let mut guard = TOKENS.lock();
    let cache = guard.get_or_insert_with(HashMap::new);
    cached_token(cache, host, || gh_cli_token(host))
}

/// Put `token` in the cache for `host` as if `gh auth token` had answered it,
/// so a test neither spawns gh nor depends on the machine's logins.
#[cfg(test)]
pub(crate) fn seed_token(host: &str, token: Option<&str>) {
    TOKENS.lock().get_or_insert_with(HashMap::new).insert(
        host.to_string(),
        TokenEntry {
            token: token.map(String::from),
            fetched_at: Instant::now(),
        },
    );
}

fn forget_token(host: &str) {
    if let Some(cache) = TOKENS.lock().as_mut() {
        cache.remove(host);
    }
}

/// gh's `RESTPrefix`: `api.<host>/` for github.com and GHE.com tenants,
/// `<host>/api/v3/` for GHE Server.
fn rest_prefix(host: &str) -> String {
    match host_kind(host) {
        HostKind::Dotcom | HostKind::Tenancy => format!("https://api.{host}/"),
        HostKind::Server => format!("https://{host}/api/v3/"),
    }
}

/// gh's `GraphQLEndpoint`.
fn graphql_endpoint(host: &str) -> String {
    match host_kind(host) {
        HostKind::Dotcom | HostKind::Tenancy => format!("https://api.{host}/graphql"),
        HostKind::Server => format!("https://{host}/api/graphql"),
    }
}

/// Why a call produced no data. `RateLimited` is kept apart so the poller can
/// park its whole fan-out; everything else is the old "`gh` failed" outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiError {
    RateLimited,
    Failed,
    /// GitHub answered that what was asked for does not exist — a deleted PR
    /// or repository, or one this token can no longer see. An answer, unlike
    /// `Failed`: a caller waiting to learn something is gone has learned it.
    NotFound,
}

/// GitHub refuses with 403/429 for both the primary hourly limit
/// (`x-ratelimit-remaining: 0`) and the secondary abuse limit (only the
/// message says so); the wording matches what `gh` printed on stderr.
fn is_rate_limited(resp: &HttpResponse) -> bool {
    if !matches!(resp.status(), 403 | 429) {
        return false;
    }
    if resp.header("x-ratelimit-remaining") == Some("0") {
        return true;
    }
    resp.json::<Value>()
        .ok()
        .and_then(|body| body.get("message")?.as_str().map(str::to_lowercase))
        .is_some_and(|message| message.contains("rate limit"))
}

/// A missing PR or repository arrives as a 200 with a `NOT_FOUND` error and a
/// null node — never as a clean null.
fn is_graphql_not_found(error: &Value) -> bool {
    error.get("type").and_then(Value::as_str) == Some("NOT_FOUND")
}

/// The primary limit on GraphQL arrives as a 200 with a typed error.
fn is_graphql_rate_limit(error: &Value) -> bool {
    error.get("type").and_then(Value::as_str) == Some("RATE_LIMITED")
        || error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.to_lowercase().contains("rate limit"))
}

/// An authenticated client for one GitHub host.
pub(crate) struct GithubClient {
    host: String,
    token: String,
    /// The errors the last GraphQL call reported, as GitHub sent them, so a
    /// caller can tell a field this host rejects from any other failure by
    /// its `type`, `path` and `extensions` rather than its wording.
    last_errors: Vec<Value>,
}

impl GithubClient {
    /// `None` when no token is obtainable — the caller treats it as `gh` failing.
    pub(crate) fn for_host(host: &str) -> Option<Self> {
        let token = token_for(host)?;
        Some(Self {
            host: host.to_string(),
            token,
            last_errors: Vec::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_token(host: &str, token: &str) -> Self {
        Self {
            host: host.to_string(),
            token: token.to_string(),
            last_errors: Vec::new(),
        }
    }

    /// The errors the last [`graphql`](Self::graphql) call reported.
    pub(crate) fn last_errors(&self) -> &[Value] {
        &self.last_errors
    }

    /// Send, retrying once with a fresh token on 401, and turn a rate-limit
    /// refusal into [`ApiError::RateLimited`]. Any other status is returned.
    fn send(&mut self, build: impl Fn() -> HttpRequest) -> Result<HttpResponse, ApiError> {
        let mut resp = self.send_once(&build)?;
        if resp.status() == 401 {
            forget_token(&self.host);
            self.token = token_for(&self.host).ok_or(ApiError::Failed)?;
            resp = self.send_once(&build)?;
        }
        if is_rate_limited(&resp) {
            return Err(ApiError::RateLimited);
        }
        Ok(resp)
    }

    fn send_once(&self, build: &impl Fn() -> HttpRequest) -> Result<HttpResponse, ApiError> {
        http::send(
            build()
                .bearer(&self.token)
                .timeout(REQUEST_TIMEOUT)
                .label("github.api"),
        )
        .map_err(|_| ApiError::Failed)
    }

    fn rest_get(&mut self, url: &str) -> Result<HttpResponse, ApiError> {
        let resp = self.send(|| {
            HttpRequest::get(url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
        })?;
        if !resp.is_success() {
            return Err(ApiError::Failed);
        }
        Ok(resp)
    }

    /// GET one REST resource (`path` is relative to the API root).
    pub(crate) fn rest_get_json(&mut self, path: &str) -> Result<Value, ApiError> {
        let url = format!("{}{path}", rest_prefix(&self.host));
        self.rest_get(&url)?.json().map_err(|_| ApiError::Failed)
    }

    /// GET a paginated REST collection, following `Link: rel="next"` and
    /// concatenating the `key` arrays — what `gh api --paginate` did.
    pub(crate) fn rest_get_all(&mut self, path: &str, key: &str) -> Result<Vec<Value>, ApiError> {
        let mut url = format!("{}{path}", rest_prefix(&self.host));
        let mut items = Vec::new();
        loop {
            let resp = self.rest_get(&url)?;
            let body: Value = resp.json().map_err(|_| ApiError::Failed)?;
            if let Some(page) = body.get(key).and_then(Value::as_array) {
                items.extend(page.iter().cloned());
            }
            match resp.next_link() {
                Some(next) => url = next.to_string(),
                None => return Ok(items),
            }
        }
    }

    /// POST a GraphQL query and return its `data`. Any reported error fails the
    /// call the way it failed `gh`, except a rate-limit error.
    pub(crate) fn graphql(&mut self, query: &str, variables: Value) -> Result<Value, ApiError> {
        self.last_errors.clear();
        let url = graphql_endpoint(&self.host);
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = self.send(|| {
            HttpRequest::post(&url)
                .json(&body)
                .header("Accept", "application/json")
        })?;
        if !resp.is_success() {
            return Err(ApiError::Failed);
        }
        let mut payload: Value = resp.json().map_err(|_| ApiError::Failed)?;
        if let Some(errors) = payload.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            self.last_errors = errors.clone();
            return Err(if errors.iter().any(is_graphql_rate_limit) {
                ApiError::RateLimited
            } else if errors.iter().all(is_graphql_not_found) {
                ApiError::NotFound
            } else {
                ApiError::Failed
            });
        }
        match payload.get_mut("data") {
            Some(data) if !data.is_null() => Ok(data.take()),
            _ => Err(ApiError::Failed),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use okena_transport::http::testing;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    // The HTTP bus mock is process-global; tests that install one must not
    // overlap, here or in the modules that build on this client.
    static MOCK_LOCK: StdMutex<()> = StdMutex::new(());

    pub(crate) fn mock_guard() -> std::sync::MutexGuard<'static, ()> {
        MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn parse(url: &str) -> gix::Url {
        gix::url::parse(gix::bstr::BStr::new(url)).expect("valid url")
    }

    fn repo(host: &str, owner: &str, name: &str) -> GithubRepo {
        GithubRepo {
            host: host.into(),
            owner: owner.into(),
            name: name.into(),
        }
    }

    fn candidate(name: &str, repo: GithubRepo, resolved: Option<&str>) -> RemoteCandidate {
        RemoteCandidate {
            name: name.into(),
            repo,
            resolved: resolved.map(String::from),
        }
    }

    #[test]
    fn remote_url_forms_map_to_owner_and_name() {
        let expected = repo("github.com", "contember", "okena");
        for url in [
            "https://github.com/contember/okena.git",
            "https://github.com/contember/okena",
            "https://github.com/contember/okena/",
            "git@github.com:contember/okena.git",
            "ssh://git@github.com/contember/okena.git",
            "ssh://git@ssh.github.com:443/contember/okena.git",
            "https://user:pass@github.com/contember/okena.git",
            "HTTPS://GitHub.com/contember/okena",
        ] {
            assert_eq!(repo_from_url(&parse(url)), Some(expected.clone()), "{url}");
        }
    }

    #[test]
    fn remote_url_keeps_other_hosts_and_extra_segments() {
        assert_eq!(
            repo_from_url(&parse("https://ghe.example.com/team/app.git")),
            Some(repo("ghe.example.com", "team", "app"))
        );
        // gh takes the first two segments and ignores the rest.
        assert_eq!(
            repo_from_url(&parse("https://github.com/o/r/tree/main")),
            Some(repo("github.com", "o", "r"))
        );
    }

    #[test]
    fn remote_url_without_owner_and_name_is_rejected() {
        assert_eq!(repo_from_url(&parse("https://github.com/only-owner")), None);
        assert_eq!(repo_from_url(&parse("https://github.com/")), None);
        assert_eq!(repo_from_url(&parse("/local/path/repo.git")), None);
    }

    #[test]
    fn base_repo_prefers_origin_where_branches_are_pushed() {
        // A fork: its PRs are opened in `origin`, not in `upstream`.
        let candidates = vec![
            candidate("origin", repo("github.com", "me", "fork"), None),
            candidate("upstream", repo("github.com", "org", "main"), None),
            candidate("backup", repo("github.com", "x", "y"), None),
        ];
        assert_eq!(
            select_base_repo(candidates, only("github.com")),
            Some(repo("github.com", "me", "fork"))
        );

        // Without `origin`, `upstream` comes before `github`.
        let candidates = vec![
            candidate("github", repo("github.com", "me", "mirror"), None),
            candidate("upstream", repo("github.com", "org", "main"), None),
        ];
        assert_eq!(
            select_base_repo(candidates, only("github.com")),
            Some(repo("github.com", "org", "main"))
        );

        // Ties keep `git remote` (alphabetical) order.
        let candidates = vec![
            candidate("alpha", repo("github.com", "a", "a"), None),
            candidate("beta", repo("github.com", "b", "b"), None),
        ];
        assert_eq!(
            select_base_repo(candidates, only("github.com")),
            Some(repo("github.com", "a", "a"))
        );
    }

    #[test]
    fn base_repo_honours_gh_repo_set_default() {
        let candidates = vec![
            candidate("origin", repo("github.com", "me", "fork"), Some("base")),
            candidate("upstream", repo("github.com", "org", "main"), None),
        ];
        assert_eq!(
            select_base_repo(candidates, only("github.com")),
            Some(repo("github.com", "me", "fork"))
        );

        let candidates = vec![candidate(
            "origin",
            repo("github.com", "me", "fork"),
            Some("other/target"),
        )];
        assert_eq!(
            select_base_repo(candidates, only("github.com")),
            Some(repo("github.com", "other", "target"))
        );

        let candidates = vec![candidate(
            "origin",
            repo("github.com", "me", "fork"),
            Some("github.com/other/target"),
        )];
        assert_eq!(
            select_base_repo(candidates, only("github.com")),
            Some(repo("github.com", "other", "target"))
        );
    }

    #[test]
    fn base_repo_only_considers_the_active_host() {
        let candidates = vec![
            candidate("upstream", repo("ghe.example.com", "org", "main"), None),
            candidate("origin", repo("github.com", "me", "fork"), None),
        ];
        assert_eq!(
            select_base_repo(candidates.clone(), only("github.com")),
            Some(repo("github.com", "me", "fork"))
        );
        assert_eq!(
            select_base_repo(candidates, only("ghe.example.com")),
            Some(repo("ghe.example.com", "org", "main"))
        );
        assert_eq!(select_base_repo(Vec::new(), only("github.com")), None);
    }

    #[test]
    fn base_repo_is_read_from_the_checkout() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let (_tmp, path) = init_temp_repo();
        git_in(
            &path,
            &["remote", "add", "origin", "git@github.com:me/fork.git"],
        );
        git_in(
            &path,
            &[
                "remote",
                "add",
                "upstream",
                "https://github.com/org/main.git",
            ],
        );
        let candidates = remote_candidates(&gix::open(&path).expect("repo"));
        assert_eq!(
            select_base_repo(candidates, only("github.com")),
            Some(repo("github.com", "me", "fork"))
        );

        // `gh repo set-default` still decides over the remote names.
        git_in(&path, &["config", "remote.upstream.gh-resolved", "base"]);
        // Opened directly: the shared handle cache would hide the config edit.
        let candidates = remote_candidates(&gix::open(&path).expect("repo"));
        assert_eq!(
            select_base_repo(candidates, only("github.com")),
            Some(repo("github.com", "org", "main"))
        );
    }

    fn only(host: &str) -> impl Fn(&str) -> bool + '_ {
        move |candidate| candidate == host
    }

    /// gh's `hosts.yml` as `gh auth login --hostname` leaves it.
    const HOSTS_YML: &str = "github.com:\n    git_protocol: ssh\n    users:\n        me:\n    user: me\nGHE.Internal.Example:\n    users:\n        me:\n    user: me\n";

    fn known_hosts() -> KnownHosts {
        KnownHosts {
            gh_host: Some("gh-host.example".into()),
            logged_in: parse_gh_hosts(HOSTS_YML),
            configured: vec!["settings.example".into()],
        }
    }

    #[test]
    fn github_hosts_are_github_com_ghe_tenants_gh_logins_gh_host_and_settings() {
        let known = known_hosts();
        for host in [
            "github.com",
            "acme.ghe.com",
            "ghe.internal.example",
            "settings.example",
            "gh-host.example",
        ] {
            assert!(known.contains(host), "{host} should be a GitHub host");
        }
        for host in ["gitlab.com", "bitbucket.org", "ghe.com", "example.com"] {
            assert!(!known.contains(host), "{host} should not be a GitHub host");
        }
        // Nothing but the built-in rules without gh logins, GH_HOST or Settings.
        let bare = KnownHosts::default();
        assert!(bare.contains("github.com") && bare.contains("acme.ghe.com"));
        assert!(!bare.contains("settings.example") && !bare.contains("gitlab.com"));
    }

    #[test]
    fn gh_hosts_file_lists_its_top_level_hosts() {
        assert_eq!(
            parse_gh_hosts(HOSTS_YML),
            ["github.com", "ghe.internal.example"]
        );
        assert_eq!(
            parse_gh_hosts("# comment\n\"quoted.example\":\n  user: me\n"),
            ["quoted.example"]
        );
        assert!(parse_gh_hosts("").is_empty());
    }

    #[test]
    fn gh_config_dir_follows_go_gh() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(
            gh_config_dir(env(&[("GH_CONFIG_DIR", "/gh"), ("XDG_CONFIG_HOME", "/xdg")])),
            Some(PathBuf::from("/gh"))
        );
        assert_eq!(
            gh_config_dir(env(&[("GH_CONFIG_DIR", ""), ("XDG_CONFIG_HOME", "/xdg")])),
            Some(PathBuf::from("/xdg").join("gh"))
        );
        if !cfg!(windows) {
            assert_eq!(
                gh_config_dir(env(&[])),
                dirs::home_dir().map(|home| home.join(".config").join("gh"))
            );
        }
    }

    #[test]
    fn settings_entries_become_hosts() {
        for entry in [
            "github.acme.corp",
            " GitHub.Acme.Corp ",
            "https://github.acme.corp/",
            "https://github.acme.corp/team/app",
            "git@github.acme.corp:team/app.git",
            "github.acme.corp:8443",
        ] {
            assert_eq!(
                host_from_setting(entry).as_deref(),
                Some("github.acme.corp"),
                "{entry}"
            );
        }
        assert_eq!(host_from_setting("  "), None);
    }

    #[test]
    fn enterprise_remotes_resolve_in_https_ssh_and_scp_form() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let known = known_hosts();
        for (url, host) in [
            ("https://settings.example/team/app.git", "settings.example"),
            ("ssh://git@settings.example/team/app.git", "settings.example"),
            ("git@settings.example:team/app.git", "settings.example"),
            ("git@acme.ghe.com:team/app.git", "acme.ghe.com"),
            ("https://ghe.internal.example/team/app", "ghe.internal.example"),
        ] {
            let (_tmp, path) = init_temp_repo();
            git_in(&path, &["remote", "add", "origin", url]);
            let candidates = remote_candidates(&gix::open(&path).expect("repo"));
            assert_eq!(
                select_base_repo(candidates, |h| known.contains(h)),
                Some(repo(host, "team", "app")),
                "{url}"
            );
            assert!(has_remote_on(&path, &known), "{url}");
            // Without gh logins or Settings only the GHE.com tenant is known.
            assert_eq!(
                has_remote_on(&path, &KnownHosts::default()),
                host == "acme.ghe.com",
                "{url}"
            );
        }

        // GitLab and unknown hosts stay out of both the gate and resolution.
        let (_tmp, path) = init_temp_repo();
        git_in(&path, &["remote", "add", "origin", "git@gitlab.com:team/app.git"]);
        git_in(&path, &["remote", "add", "upstream", "https://code.example/team/app.git"]);
        assert!(!has_remote_on(&path, &known));
        let candidates = remote_candidates(&gix::open(&path).expect("repo"));
        assert_eq!(select_base_repo(candidates, |h| known.contains(h)), None);
    }

    #[test]
    fn a_host_added_in_settings_is_gated_in_and_removing_it_gates_it_out() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let _guard = mock_guard();
        let (_tmp, path) = init_temp_repo();
        git_in(
            &path,
            &["remote", "add", "origin", "https://only-in-settings.example/team/app.git"],
        );
        set_enterprise_hosts(&[]);
        assert!(!has_github_remote(&path));
        assert_eq!(resolve_base_repo(&path), None);

        set_enterprise_hosts(&["https://Only-In-Settings.example/".into()]);
        assert!(has_github_remote(&path));
        assert_eq!(
            resolve_base_repo(&path),
            Some(repo("only-in-settings.example", "team", "app"))
        );

        set_enterprise_hosts(&[]);
        assert!(!has_github_remote(&path));
        assert_eq!(resolve_base_repo(&path), None);
    }

    #[test]
    fn repo_keys_keep_github_com_short_and_name_every_other_host() {
        assert_eq!(repo_key(&repo("github.com", "N1rna", "Okena")), "n1rna/okena");
        assert_eq!(
            repo_key(&repo("acme.ghe.com", "N1rna", "Okena")),
            "acme.ghe.com/n1rna/okena"
        );
        assert_ne!(
            repo_key(&repo("github.acme.corp", "o", "r")),
            repo_key(&repo("github.com", "o", "r"))
        );
    }

    #[test]
    fn api_endpoints_follow_gh_instance_rules() {
        assert_eq!(rest_prefix("github.com"), "https://api.github.com/");
        assert_eq!(
            graphql_endpoint("github.com"),
            "https://api.github.com/graphql"
        );
        assert_eq!(
            rest_prefix("ghe.example.com"),
            "https://ghe.example.com/api/v3/"
        );
        assert_eq!(
            graphql_endpoint("ghe.example.com"),
            "https://ghe.example.com/api/graphql"
        );
        assert_eq!(normalize_github_host("SSH.GitHub.com"), "github.com");
        assert_eq!(normalize_github_host("GHE.Example.com"), "ghe.example.com");
    }

    #[test]
    fn each_kind_of_host_gets_gh_endpoints_and_token_env_vars() {
        // github.com
        assert_eq!(host_kind("github.com"), HostKind::Dotcom);
        assert_eq!(token_env_vars("github.com"), ["GH_TOKEN", "GITHUB_TOKEN"]);

        // A GHE.com tenant: its own `api.` host, and gh's tenancy env vars.
        assert_eq!(host_kind("acme.ghe.com"), HostKind::Tenancy);
        assert_eq!(rest_prefix("acme.ghe.com"), "https://api.acme.ghe.com/");
        assert_eq!(
            graphql_endpoint("acme.ghe.com"),
            "https://api.acme.ghe.com/graphql"
        );
        assert_eq!(token_env_vars("acme.ghe.com"), ["GH_TOKEN", "GITHUB_TOKEN"]);
        assert_eq!(normalize_github_host("API.Acme.GHE.com"), "acme.ghe.com");
        assert_eq!(normalize_github_host("acme.ghe.com"), "acme.ghe.com");

        // GHE Server.
        assert_eq!(host_kind("github.acme.corp"), HostKind::Server);
        assert_eq!(
            rest_prefix("github.acme.corp"),
            "https://github.acme.corp/api/v3/"
        );
        assert_eq!(
            graphql_endpoint("github.acme.corp"),
            "https://github.acme.corp/api/graphql"
        );
        assert_eq!(
            token_env_vars("github.acme.corp"),
            ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"]
        );
        // `ghe.com` itself is no tenant.
        assert_eq!(host_kind("ghe.com"), HostKind::Server);
    }

    #[test]
    fn token_cache_spawns_once_per_ttl() {
        let mut cache = HashMap::new();
        let calls = AtomicUsize::new(0);
        let fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Some("tok".to_string())
        };
        assert_eq!(
            cached_token(&mut cache, "github.com", fetch).as_deref(),
            Some("tok")
        );
        assert_eq!(
            cached_token(&mut cache, "github.com", fetch).as_deref(),
            Some("tok")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Expired entry: asked again.
        if let Some(stale) = Instant::now().checked_sub(TOKEN_TTL + Duration::from_secs(1)) {
            cache.get_mut("github.com").expect("cached").fetched_at = stale;
            assert_eq!(
                cached_token(&mut cache, "github.com", fetch).as_deref(),
                Some("tok")
            );
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }

    #[test]
    fn a_missing_token_is_cached_briefly() {
        let mut cache = HashMap::new();
        let calls = AtomicUsize::new(0);
        let fetch = || {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        };
        assert_eq!(cached_token(&mut cache, "github.com", fetch), None);
        assert_eq!(cached_token(&mut cache, "github.com", fetch), None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        if let Some(stale) = Instant::now().checked_sub(MISSING_TOKEN_TTL + Duration::from_secs(1))
        {
            cache.get_mut("github.com").expect("cached").fetched_at = stale;
            assert_eq!(cached_token(&mut cache, "github.com", fetch), None);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }

    pub(crate) fn response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
        HttpResponse::new(
            status,
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body.as_bytes().to_vec(),
        )
    }

    #[test]
    fn rate_limit_is_told_apart_from_other_failures() {
        assert!(is_rate_limited(&response(
            403,
            &[("x-ratelimit-remaining", "0")],
            r#"{"message":"API rate limit exceeded for user ID 1."}"#
        )));
        assert!(is_rate_limited(&response(
            403,
            &[("X-RateLimit-Remaining", "4999")],
            r#"{"message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."}"#
        )));
        assert!(is_rate_limited(&response(
            429,
            &[("retry-after", "60")],
            r#"{"message":"You have exceeded a secondary rate limit."}"#
        )));
        assert!(!is_rate_limited(&response(
            403,
            &[("x-ratelimit-remaining", "4000")],
            r#"{"message":"Resource not accessible by integration"}"#
        )));
        assert!(!is_rate_limited(&response(
            404,
            &[],
            r#"{"message":"Not Found"}"#
        )));
        // A success is never a rate-limit hit, whatever the body says.
        assert!(!is_rate_limited(&response(
            200,
            &[("x-ratelimit-remaining", "0")],
            r#"{"message":"rate limit"}"#
        )));
    }

    #[test]
    fn graphql_rate_limit_errors_are_recognised() {
        assert!(is_graphql_rate_limit(&serde_json::json!({
            "type": "RATE_LIMITED",
            "message": "API rate limit exceeded for user ID 1."
        })));
        assert!(is_graphql_rate_limit(&serde_json::json!({
            "message": "You have exceeded a secondary rate limit."
        })));
        assert!(!is_graphql_rate_limit(&serde_json::json!({
            "type": "NOT_FOUND",
            "message": "Could not resolve to a Repository with the name 'o/r'."
        })));
    }

    #[test]
    fn github_saying_a_pr_or_repo_does_not_exist_is_not_found_not_a_failure() {
        // Verbatim from GitHub: a missing PR number in a real repo, and a
        // missing repository. Neither comes back as a clean null.
        const MISSING_PR: &str = r#"{"data":{"repository":{"pullRequest":null}},"errors":[{"type":"NOT_FOUND","path":["repository","pullRequest"],"locations":[{"line":1,"column":45}],"message":"Could not resolve to a PullRequest with the number of 999999."}]}"#;
        const MISSING_REPO: &str = r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","path":["repository"],"locations":[{"line":1,"column":3}],"message":"Could not resolve to a Repository with the name 'n1rna/does-not-exist-qbl372'."}]}"#;
        const NOT_FOUND_AND_MORE: &str = r#"{"data":null,"errors":[{"type":"NOT_FOUND","message":"Could not resolve"},{"type":"FORBIDDEN","message":"Resource not accessible"}]}"#;

        let _guard = mock_guard();
        let _mock = testing::mock(|req| {
            let query = req
                .json_body()
                .and_then(|b| b["query"].as_str())
                .unwrap_or("");
            Ok(response(
                200,
                &[],
                if query.contains("pr") {
                    MISSING_PR
                } else if query.contains("repo") {
                    MISSING_REPO
                } else {
                    NOT_FOUND_AND_MORE
                },
            ))
        });

        let mut client = GithubClient::with_token("github.com", "tok");
        assert_eq!(
            client.graphql("query pr", Value::Null),
            Err(ApiError::NotFound)
        );
        assert_eq!(
            client.graphql("query repo", Value::Null),
            Err(ApiError::NotFound)
        );
        // Anything besides NOT_FOUND among the errors is still a failure.
        assert_eq!(
            client.graphql("query mixed", Value::Null),
            Err(ApiError::Failed)
        );
    }

    #[test]
    fn paginated_rest_collections_follow_the_link_header() {
        let _guard = mock_guard();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let seen_in_mock = seen.clone();
        let _mock = testing::mock(move |req| {
            seen_in_mock
                .lock()
                .expect("lock")
                .push(req.url().to_string());
            let next =
                "https://api.github.com/repos/o/r/commits/abc/check-runs?per_page=100&page=2";
            if req.url().ends_with("page=2") {
                Ok(response(200, &[], r#"{"check_runs":[{"name":"b"}]}"#))
            } else {
                Ok(response(
                    200,
                    &[("link", &format!("<{next}>; rel=\"next\""))],
                    r#"{"check_runs":[{"name":"a"}]}"#,
                ))
            }
        });

        let mut client = GithubClient::with_token("github.com", "tok");
        let runs = client
            .rest_get_all(
                "repos/o/r/commits/abc/check-runs?per_page=100",
                "check_runs",
            )
            .expect("two pages");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0]["name"], "a");
        assert_eq!(runs[1]["name"], "b");
        assert_eq!(
            seen.lock().expect("lock").as_slice(),
            [
                "https://api.github.com/repos/o/r/commits/abc/check-runs?per_page=100",
                "https://api.github.com/repos/o/r/commits/abc/check-runs?per_page=100&page=2",
            ]
        );
    }

    #[test]
    fn rest_calls_carry_the_github_headers() {
        let _guard = mock_guard();
        let _mock = testing::mock(|req| {
            assert_eq!(
                req.header_value("Accept"),
                Some("application/vnd.github+json")
            );
            assert_eq!(req.header_value("X-GitHub-Api-Version"), Some("2022-11-28"));
            assert_eq!(req.header_value("Authorization"), Some("Bearer tok"));
            Ok(response(200, &[], r#"{"statuses":[]}"#))
        });
        let mut client = GithubClient::with_token("github.com", "tok");
        assert!(client.rest_get_json("repos/o/r/commits/abc/status").is_ok());
    }

    #[test]
    fn refusals_map_to_rate_limited_and_the_rest_to_failed() {
        let _guard = mock_guard();
        let _mock = testing::mock(|req| {
            let url = req.url();
            if url.ends_with("/graphql") {
                let query = req
                    .json_body()
                    .and_then(|b| b["query"].as_str())
                    .unwrap_or("");
                return Ok(if query.contains("limited") {
                    response(
                        200,
                        &[],
                        r#"{"data":null,"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#,
                    )
                } else if query.contains("missing") {
                    response(
                        200,
                        &[],
                        r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","message":"Could not resolve"}]}"#,
                    )
                } else {
                    response(200, &[], r#"{"data":{"repository":{"ok":true}}}"#)
                });
            }
            Ok(if url.ends_with("/limited") {
                response(
                    403,
                    &[("x-ratelimit-remaining", "0")],
                    r#"{"message":"API rate limit exceeded"}"#,
                )
            } else {
                response(404, &[], r#"{"message":"Not Found"}"#)
            })
        });

        let mut client = GithubClient::with_token("github.com", "tok");
        assert_eq!(
            client.graphql("query limited", Value::Null),
            Err(ApiError::RateLimited)
        );
        assert_eq!(
            client.graphql("query missing", Value::Null),
            Err(ApiError::NotFound)
        );
        assert_eq!(
            client.graphql("query fine", Value::Null),
            Ok(serde_json::json!({"repository": {"ok": true}}))
        );
        assert_eq!(client.rest_get_json("limited"), Err(ApiError::RateLimited));
        assert_eq!(client.rest_get_json("absent"), Err(ApiError::Failed));
    }
}
