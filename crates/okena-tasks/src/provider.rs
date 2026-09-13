//! The provider abstraction.
//!
//! Adding Jira or Azure DevOps later means implementing [`TaskProvider`] and
//! registering it — no changes in the daemon or the harness UI.

use okena_core::tasks::{Task, TaskId, TaskState};
use std::fmt;

/// How a provider's credentials are supplied.
///
/// Both variants end up as a bearer token on the wire; they differ in where the
/// token came from and whether it can expire. Keeping them distinct lets the UI
/// say "your login expired, re-authorize" rather than "401".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    /// A long-lived key the user pasted in. Never expires on its own.
    ApiKey(String),
    /// An OAuth access token. May expire; `refresh_token` re-mints it.
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        /// Expiry as a Unix timestamp in seconds, when the provider states one.
        expires_at: Option<u64>,
    },
    /// A personal access token that is only meaningful against one
    /// organization — Azure DevOps. A bare key cannot say where to send it,
    /// which is why this is not an `ApiKey`.
    PersonalAccessToken {
        token: String,
        /// Normalized organization root, e.g. `https://dev.azure.com/contoso`.
        organization_url: String,
        /// Who the token belongs to, recorded when it was verified so the
        /// settings page can say "Connected as …" without a network call.
        account: Option<String>,
    },
}

impl Credential {
    /// The secret to send. How it is framed on the wire (raw, `Bearer`,
    /// `Basic`) is the provider's business.
    pub fn bearer(&self) -> &str {
        match self {
            Credential::ApiKey(k) => k,
            Credential::OAuth { access_token, .. } => access_token,
            Credential::PersonalAccessToken { token, .. } => token,
        }
    }

    /// Whether an OAuth token is past its stated expiry, with `skew_secs` of
    /// slack so a token that dies mid-flight is refreshed first.
    pub fn is_expired(&self, now_unix: u64, skew_secs: u64) -> bool {
        match self {
            Credential::ApiKey(_) | Credential::PersonalAccessToken { .. } => false,
            Credential::OAuth { expires_at, .. } => {
                expires_at.is_some_and(|e| now_unix.saturating_add(skew_secs) >= e)
            }
        }
    }
}

// A credential is a secret: keep it out of logs even when a surrounding struct
// is derived-Debug'd.
impl fmt::Debug for CredentialRedacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Credential::ApiKey(_) => f.write_str("ApiKey(<redacted>)"),
            Credential::OAuth { expires_at, .. } => {
                write!(f, "OAuth(<redacted>, expires_at={expires_at:?})")
            }
            Credential::PersonalAccessToken {
                organization_url, ..
            } => write!(f, "PersonalAccessToken(<redacted>, {organization_url})"),
        }
    }
}

/// Wrapper that renders a [`Credential`] with its secret elided.
pub struct CredentialRedacted<'a>(pub &'a Credential);

/// Whether a provider is ready to make calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    /// No credential stored — the user has not connected this provider.
    Disconnected,
    /// Credential present and usable.
    Connected {
        /// Display name of the authenticated user, when known.
        account: Option<String>,
    },
    /// Credential present but rejected or expired; the user must re-authorize.
    Expired,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("not authenticated with {provider}")]
    NotAuthenticated { provider: &'static str },
    /// The provider rejected our credential (401/403). Distinct from a
    /// transport failure so the UI can prompt for re-auth instead of retrying.
    #[error("{provider} rejected the stored credential")]
    Unauthorized { provider: &'static str },
    #[error("{provider} request failed: {message}")]
    Transport {
        provider: &'static str,
        message: String,
    },
    /// The response parsed as JSON but didn't match the expected shape, or the
    /// provider returned an API-level error inside a 200.
    #[error("{provider} returned an unexpected response: {message}")]
    Protocol {
        provider: &'static str,
        message: String,
    },
    /// The provider has no concept of what was asked, or okena has not taught
    /// it yet. Distinct from a failure: nothing went wrong and retrying will
    /// not help, so the UI hides the affordance rather than showing an error.
    #[error("{provider} does not support {what}")]
    Unsupported {
        provider: &'static str,
        what: &'static str,
    },
    /// The request was well-formed but the caller has to decide something
    /// first — most often which team or project a new task belongs to.
    #[error("{message}")]
    NeedsChoice { message: String },
}

/// Where tasks live on a provider: a Linear team, an Azure DevOps project.
///
/// Named for what it does rather than after any one provider's word for it,
/// since the next backend will call it something else again.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskContainer {
    pub id: String,
    pub name: String,
    /// Short prefix the provider puts on keys, e.g. `QBL`. Empty when it has
    /// no such notion.
    #[serde(default)]
    pub key: String,
}

/// A task to be created.
///
/// Deliberately not a `Task`: a draft has no id, key, url or state, and
/// modelling it as a half-filled `Task` would put five meaningless fields in
/// front of every caller.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskDraft {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Where it sits in the breakdown. Providers map this onto whatever they
    /// have — Linear has no native kind, so okena carries it as a label.
    #[serde(default)]
    pub kind: okena_core::tasks::TaskKind,
    /// Parent's provider id, when this is a sub-task. A child inherits its
    /// parent's container, so `container_id` is ignored when this is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_external_id: Option<String>,
    /// Which team or project to create it in. Ignored for a sub-task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
}

/// Changes to an existing task. A field left `None` is left as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Markdown. An empty string clears the description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl TaskPatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.description.is_none()
    }
}

/// A task source. Implementations are expected to be cheap to construct and
/// are called from daemon worker threads, so methods are blocking.
///
/// Methods that take a [`TaskId`] accept either the provider's own id or the
/// task's display key (`QBL-371`, `#42`) in `external_id`: an agent is handed
/// the key, and making it look up an id first would be a round-trip for
/// nothing.
pub trait TaskProvider: Send + Sync {
    /// Stable identifier, e.g. `"linear"`. Must match [`TaskId::provider`].
    fn id(&self) -> &'static str;

    /// Human-facing name for the UI, e.g. `"Linear"`.
    fn display_name(&self) -> &'static str;

    fn auth_status(&self) -> AuthStatus;

    /// Tasks assigned to the authenticated user, newest activity first.
    ///
    /// Closed tasks are excluded — the harness is a work queue, not an archive.
    fn list_assigned(&self) -> Result<Vec<Task>, TaskError>;

    /// Move a task to a new state. Providers map the normalized category back
    /// onto one of their own workflow states; when a team has several states in
    /// the same category the provider picks its canonical one.
    fn set_state(&self, id: &TaskId, state: TaskState) -> Result<(), TaskError>;

    /// Teams or projects the authenticated user can file tasks in.
    ///
    /// Only needed to create a top-level task; a sub-task inherits its
    /// parent's. Providers that cannot enumerate them say so, and the UI then
    /// offers sub-tasks only.
    fn list_containers(&self) -> Result<Vec<TaskContainer>, TaskError> {
        Err(TaskError::Unsupported {
            provider: self.id(),
            what: "listing teams",
        })
    }

    /// Create a task and return it as the provider now sees it.
    ///
    /// Returns the created task rather than an id so the caller can show it
    /// without a second round-trip — and so the key, url and branch name the
    /// provider assigns are the provider's, not a guess.
    fn create_task(&self, _draft: &TaskDraft) -> Result<Task, TaskError> {
        Err(TaskError::Unsupported {
            provider: self.id(),
            what: "creating tasks",
        })
    }

    /// Sub-tasks of `id`, whether or not they are assigned to the user.
    ///
    /// Distinct from `list_assigned`, which is a personal work queue: breaking
    /// a task down means seeing every child, including ones assigned to
    /// somebody else or to nobody.
    fn list_children(&self, _id: &TaskId) -> Result<Vec<Task>, TaskError> {
        Err(TaskError::Unsupported {
            provider: self.id(),
            what: "listing sub-tasks",
        })
    }

    /// One task, with its description, whoever it is assigned to.
    fn get_task(&self, _id: &TaskId) -> Result<Task, TaskError> {
        Err(TaskError::Unsupported {
            provider: self.id(),
            what: "reading a task",
        })
    }

    /// Change a task's title or description, and return it as it now is.
    fn update_task(&self, _id: &TaskId, _patch: &TaskPatch) -> Result<Task, TaskError> {
        Err(TaskError::Unsupported {
            provider: self.id(),
            what: "editing tasks",
        })
    }

    /// Add a comment to a task. `body` is Markdown.
    fn add_comment(&self, _id: &TaskId, _body: &str) -> Result<(), TaskError> {
        Err(TaskError::Unsupported {
            provider: self.id(),
            what: "commenting on tasks",
        })
    }

    /// Branch name to use when starting a worktree for `task`.
    ///
    /// okena's own format for every provider, rebuilt from the task rather
    /// than read from `task.branch_name`, so a task assembled elsewhere cannot
    /// smuggle in a provider's suggestion. The key stays in the name, which is
    /// all Linear needs to link the branch and its PR to the issue.
    fn branch_name(&self, task: &Task) -> String {
        task_branch_name(task.kind, &task.display_key, &task.title)
    }
}

/// okena's branch name for a task: `<prefix>/<key>-<title-slug>`, e.g.
/// `feat/qbl-360-harness-edit-specs`. The prefix comes from the kind; the
/// 60-character cap applies to the slug alone.
pub fn task_branch_name(kind: okena_core::tasks::TaskKind, key: &str, title: &str) -> String {
    let slug = slugify_branch(key, title);
    if slug.is_empty() {
        return String::new();
    }
    format!("{}/{slug}", kind.branch_prefix())
}

/// Build a git-safe branch name from a task key and title.
///
/// Git refuses refs containing a space, `~^:?*[\`, a `..`, a trailing `.` or
/// `.lock`, and leading/trailing `/` — so everything outside a conservative
/// allowlist collapses to `-`.
pub fn slugify_branch(key: &str, title: &str) -> String {
    let mut out = String::with_capacity(key.len() + title.len() + 1);
    let mut last_dash = false;
    for ch in format!("{key}-{title}").chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    // A trailing separator would leave `feature-` or a bare `.`-adjacent ref.
    while out.ends_with('-') {
        out.pop();
    }
    // Keep it comfortably under filesystem path limits: worktree directories
    // are derived from the branch name.
    out.truncate(60);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_git_safe() {
        assert_eq!(
            slugify_branch("LIN-12", "Fix: the thing/that broke?"),
            "lin-12-fix-the-thing-that-broke"
        );
    }

    #[test]
    fn slug_collapses_runs_and_trims() {
        assert_eq!(slugify_branch("AB-1", "  a   b  "), "ab-1-a-b");
    }

    #[test]
    fn slug_truncates_without_trailing_separator() {
        let s = slugify_branch("LIN-1", &"word ".repeat(40));
        assert!(s.len() <= 60, "got {} chars", s.len());
        assert!(!s.ends_with('-'));
    }

    #[test]
    fn branch_is_prefixed_by_kind() {
        use okena_core::tasks::TaskKind;
        let name = |k| task_branch_name(k, "QBL-360", "Harness: edit specs");
        assert_eq!(name(TaskKind::Epic), "feat/qbl-360-harness-edit-specs");
        assert_eq!(name(TaskKind::Feature), "feat/qbl-360-harness-edit-specs");
        assert_eq!(name(TaskKind::Story), "feat/qbl-360-harness-edit-specs");
        assert_eq!(name(TaskKind::Defect), "fix/qbl-360-harness-edit-specs");
        assert_eq!(name(TaskKind::Task), "chore/qbl-360-harness-edit-specs");
    }

    #[test]
    fn prefix_sits_outside_the_slug_cap() {
        let s = task_branch_name(
            okena_core::tasks::TaskKind::Feature,
            "LIN-1",
            &"word ".repeat(40),
        );
        let slug = s.strip_prefix("feat/").expect("prefixed");
        assert_eq!(slug, slugify_branch("LIN-1", &"word ".repeat(40)));
    }

    #[test]
    fn nothing_to_slug_gives_no_branch() {
        // An empty name is what `start_work` refuses; a bare `chore/` is not.
        assert_eq!(task_branch_name(Default::default(), "", "?!"), "");
    }

    #[test]
    fn api_key_never_expires() {
        let c = Credential::ApiKey("k".into());
        assert!(!c.is_expired(u64::MAX, 0));
    }

    #[test]
    fn oauth_expiry_respects_skew() {
        let c = Credential::OAuth {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: Some(1_000),
        };
        assert!(!c.is_expired(900, 30));
        assert!(c.is_expired(980, 30));
    }

    #[test]
    fn credential_debug_redacts_secret() {
        let c = Credential::ApiKey("super-secret".into());
        let rendered = format!("{:?}", CredentialRedacted(&c));
        assert!(!rendered.contains("super-secret"), "leaked: {rendered}");

        let pat = Credential::PersonalAccessToken {
            token: "super-secret".into(),
            organization_url: "https://dev.azure.com/contoso".into(),
            account: None,
        };
        let rendered = format!("{:?}", CredentialRedacted(&pat));
        assert!(!rendered.contains("super-secret"), "leaked: {rendered}");
        assert!(!pat.is_expired(u64::MAX, 0));
    }
}
