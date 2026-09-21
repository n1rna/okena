//! Connection and credential storage, one file per profile.
//!
//! Lives at `<profile root>/tasks_credentials.json`, mode 0600 — the same
//! at-rest treatment `remote_secret` gets, and per-profile for the same reason:
//! a `dev` profile must not read the credentials a `default` profile holds.
//!
//! Only the connections and their credentials are persisted. Task lists are
//! remote state and are never written to disk — a stale cached issue list is
//! worse than an empty one.
//!
//! Credentials are keyed by **connection id**, not by provider kind. Before
//! spaces there was one login per kind and the two were the same string, so a
//! file written then still reads: every `providers` key names a connection
//! whose kind is that same id ([`connections_of`]), which is why nobody has to
//! sign in again after the update.

use okena_core::connections::{Connection, mint_id};
use crate::provider::Credential;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const FILE_NAME: &str = "tasks_credentials.json";

/// On-disk shape.
#[derive(Default, Serialize, Deserialize)]
struct CredentialFile {
    /// Credentials by connection id. Named `providers` because that is what
    /// the key meant when the file was first written, and renaming it would
    /// cost every existing profile its login for nothing.
    #[serde(default)]
    providers: HashMap<String, StoredCredential>,
    /// The connections themselves, in the order they were added. Absent in a
    /// file written before spaces — see [`connections_of`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    connections: Vec<Connection>,
}

/// The connections a file describes, in display order.
///
/// A `providers` key with no listed connection is one from before spaces: it
/// becomes a connection whose id and kind are that key, named after the
/// backend. Nothing is written back here — the list is materialized the first
/// time something actually edits it, so merely reading a profile never
/// rewrites its credential file.
fn connections_of(file: &CredentialFile) -> Vec<Connection> {
    let mut out = file.connections.clone();
    // Deterministic order for the synthesized ones: the kinds this build
    // knows, in display order, then anything else alphabetically.
    let mut loose: Vec<&String> = file
        .providers
        .keys()
        .filter(|id| !out.iter().any(|c| &&c.id == id))
        .collect();
    loose.sort_by_key(|id| {
        (
            crate::KNOWN_PROVIDERS
                .iter()
                .position(|k| k == &id.as_str())
                .unwrap_or(usize::MAX),
            (*id).clone(),
        )
    });
    for id in loose {
        out.push(Connection::new(
            id.clone(),
            id.clone(),
            okena_core::connections::kind_display_name(id),
        ));
    }
    out
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredCredential {
    ApiKey {
        key: String,
    },
    OAuth {
        access_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
    },
    /// A new tag rather than a field on `ApiKey`, so a file written before it
    /// existed still reads exactly as it did.
    PersonalAccessToken {
        token: String,
        organization_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
}

impl From<&Credential> for StoredCredential {
    fn from(c: &Credential) -> Self {
        match c {
            Credential::ApiKey(key) => StoredCredential::ApiKey { key: key.clone() },
            Credential::PersonalAccessToken {
                token,
                organization_url,
                account,
            } => StoredCredential::PersonalAccessToken {
                token: token.clone(),
                organization_url: organization_url.clone(),
                account: account.clone(),
            },
            Credential::OAuth {
                access_token,
                refresh_token,
                expires_at,
            } => StoredCredential::OAuth {
                access_token: access_token.clone(),
                refresh_token: refresh_token.clone(),
                expires_at: *expires_at,
            },
        }
    }
}

impl From<StoredCredential> for Credential {
    fn from(s: StoredCredential) -> Self {
        match s {
            StoredCredential::ApiKey { key } => Credential::ApiKey(key),
            StoredCredential::PersonalAccessToken {
                token,
                organization_url,
                account,
            } => Credential::PersonalAccessToken {
                token,
                organization_url,
                account,
            },
            StoredCredential::OAuth {
                access_token,
                refresh_token,
                expires_at,
            } => Credential::OAuth {
                access_token,
                refresh_token,
                expires_at,
            },
        }
    }
}

fn path() -> Option<PathBuf> {
    okena_core::profiles::try_current().map(|p| p.root.join(FILE_NAME))
}

fn read_file(at: &std::path::Path) -> CredentialFile {
    match std::fs::read_to_string(at) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            // A corrupt file must not wedge the harness: log and start clean,
            // which surfaces as "not connected" and prompts a reconnect.
            log::warn!("[tasks] could not parse {}: {e}", at.display());
            CredentialFile::default()
        }),
        Err(_) => CredentialFile::default(),
    }
}

fn write_file(at: &std::path::Path, file: &CredentialFile) -> Result<(), String> {
    let json = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Write-then-rename so an interrupted save can't truncate a good file, and
    // chmod the temp *before* it holds a secret.
    let tmp = at.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, at).map_err(|e| e.to_string())
}

/// Load the stored credential for `provider`, if any.
pub fn load(provider: &str) -> Option<Credential> {
    let at = path()?;
    read_file(&at)
        .providers
        .remove(provider)
        .map(Credential::from)
}

/// Persist `credential` for `provider`, replacing any existing one.
pub fn save(provider: &str, credential: &Credential) -> Result<(), String> {
    let at = path().ok_or("no active profile — cannot store credentials")?;
    let mut file = read_file(&at);
    file.providers
        .insert(provider.to_string(), StoredCredential::from(credential));
    write_file(&at, &file)
}

/// Forget the credential for `provider`. Succeeds when none was stored.
pub fn clear(provider: &str) -> Result<(), String> {
    let at = path().ok_or("no active profile — cannot clear credentials")?;
    let mut file = read_file(&at);
    if file.providers.remove(provider).is_none() {
        return Ok(());
    }
    write_file(&at, &file)
}

/// Every connection this profile holds, in display order.
pub fn connections() -> Vec<Connection> {
    let Some(at) = path() else {
        return Vec::new();
    };
    connections_of(&read_file(&at))
}

/// One connection by id.
pub fn connection(id: &str) -> Option<Connection> {
    connections().into_iter().find(|c| c.id == id)
}

/// Add a connection to `kind`, named `name`, and return it.
///
/// The credential is stored separately, under the returned id — a connection
/// exists before it is signed in, which is what lets the add-a-space form show
/// the login form for a connection it has already named.
pub fn add_connection(kind: &str, name: &str) -> Result<Connection, String> {
    let at = path().ok_or("no active profile — cannot store connections")?;
    let mut file = read_file(&at);
    // Materialize first, so a pre-spaces `linear` credential becomes a listed
    // connection rather than colliding with the id minted here.
    let mut existing = connections_of(&file);
    let taken: Vec<String> = existing.iter().map(|c| c.id.clone()).collect();
    let added = Connection::new(mint_id(kind, &taken), kind, name);
    existing.push(added.clone());
    file.connections = existing;
    write_file(&at, &file)?;
    Ok(added)
}

/// Rename a connection. Unknown ids are an error rather than a silent no-op:
/// a rename that vanishes is worse than one that says it could not happen.
pub fn rename_connection(id: &str, name: &str) -> Result<(), String> {
    let at = path().ok_or("no active profile — cannot store connections")?;
    let mut file = read_file(&at);
    let mut existing = connections_of(&file);
    let Some(entry) = existing.iter_mut().find(|c| c.id == id) else {
        return Err(format!("no connection called {id}"));
    };
    entry.name = name.to_string();
    file.connections = existing;
    write_file(&at, &file)
}

/// Forget a connection and the credential filed under it.
///
/// Whether a connection a space still uses may be removed is the caller's
/// call, not this one's — see the settings page, which refuses and names them.
pub fn remove_connection(id: &str) -> Result<(), String> {
    let at = path().ok_or("no active profile — cannot store connections")?;
    let mut file = read_file(&at);
    let mut existing = connections_of(&file);
    existing.retain(|c| c.id != id);
    file.connections = existing;
    file.providers.remove(id);
    write_file(&at, &file)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These exercise the serialization layer directly rather than through
    // `load`/`save`: the profile global is process-wide and set once at
    // startup, so a test can't point it at a temp dir without racing every
    // other test in the binary.

    #[test]
    fn api_key_round_trips() {
        let c = Credential::ApiKey("lin_api_1".into());
        let back: Credential = StoredCredential::from(&c).into();
        assert_eq!(back, c);
    }

    #[test]
    fn oauth_round_trips_with_expiry() {
        let c = Credential::OAuth {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at: Some(42),
        };
        let back: Credential = StoredCredential::from(&c).into();
        assert_eq!(back, c);
    }

    #[test]
    fn personal_access_token_round_trips_with_its_organization() {
        let c = Credential::PersonalAccessToken {
            token: "pat".into(),
            organization_url: "https://dev.azure.com/contoso".into(),
            account: Some("Nima".into()),
        };
        let back: Credential = StoredCredential::from(&c).into();
        assert_eq!(back, c);
    }

    #[test]
    fn a_file_written_before_azure_devops_still_reads() {
        // The exact shape older builds wrote. Adding a provider must not cost
        // anyone their stored Linear key.
        let json = r#"{"providers":{"linear":{"kind":"api_key","key":"lin_api_1"}}}"#;
        let mut file: CredentialFile = serde_json::from_str(json).expect("old file decodes");
        let linear = file.providers.remove("linear").map(Credential::from);
        assert_eq!(linear, Some(Credential::ApiKey("lin_api_1".into())));
    }

    #[test]
    fn both_providers_live_side_by_side_in_one_file() {
        let mut f = CredentialFile::default();
        f.providers.insert(
            "linear".into(),
            StoredCredential::from(&Credential::ApiKey("k".into())),
        );
        f.providers.insert(
            "azure_devops".into(),
            StoredCredential::from(&Credential::PersonalAccessToken {
                token: "pat".into(),
                organization_url: "https://dev.azure.com/contoso".into(),
                account: None,
            }),
        );
        let json = serde_json::to_string(&f).expect("serialize");
        assert!(
            json.contains("\"kind\":\"personal_access_token\""),
            "got {json}"
        );
        let back: CredentialFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.providers.len(), 2);
    }

    #[test]
    fn file_is_keyed_by_provider() {
        let mut f = CredentialFile::default();
        f.providers.insert(
            "linear".into(),
            StoredCredential::from(&Credential::ApiKey("k".into())),
        );
        let json = serde_json::to_string(&f).expect("serialize");
        let back: CredentialFile = serde_json::from_str(&json).expect("deserialize");
        assert!(back.providers.contains_key("linear"));
        // Tagged representation keeps the two kinds distinguishable on disk.
        assert!(json.contains("\"kind\":\"api_key\""), "got {json}");
    }

    #[test]
    fn a_credential_file_from_before_spaces_reads_as_one_connection_per_login() {
        // The whole no-one-signs-in-again promise: the key that used to name a
        // provider now names a connection to that same provider, holding the
        // same credential.
        let json = r#"{"providers":{"linear":{"kind":"api_key","key":"lin_api_1"}}}"#;
        let file: CredentialFile = serde_json::from_str(json).expect("old file decodes");
        let connections = connections_of(&file);
        assert_eq!(
            connections,
            vec![Connection::new("linear", "linear", "Linear")]
        );
    }

    #[test]
    fn both_legacy_logins_become_connections_in_display_order() {
        let json = r#"{"providers":{
            "azure_devops":{"kind":"personal_access_token","token":"p","organization_url":"https://dev.azure.com/contoso"},
            "linear":{"kind":"api_key","key":"k"}
        }}"#;
        let file: CredentialFile = serde_json::from_str(json).expect("decodes");
        let ids: Vec<String> = connections_of(&file).into_iter().map(|c| c.id).collect();
        // KNOWN_PROVIDERS order, not the HashMap's.
        assert_eq!(ids, ["linear", "azure_devops"]);
    }

    #[test]
    fn a_listed_connection_is_not_synthesized_a_second_time() {
        let mut file = CredentialFile::default();
        file.connections
            .push(Connection::new("linear", "linear", "Acme Linear"));
        file.providers.insert(
            "linear".into(),
            StoredCredential::from(&Credential::ApiKey("k".into())),
        );
        let connections = connections_of(&file);
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].name, "Acme Linear");
    }

    #[test]
    fn a_second_account_of_the_same_kind_sits_beside_the_first() {
        let mut file = CredentialFile::default();
        file.providers.insert(
            "linear".into(),
            StoredCredential::from(&Credential::ApiKey("k".into())),
        );
        file.connections = connections_of(&file);
        file.connections
            .push(Connection::new("linear-2", "linear", "Client A Linear"));
        let ids: Vec<String> = connections_of(&file).into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["linear", "linear-2"]);
    }

    #[test]
    fn connections_round_trip_alongside_the_credentials() {
        let mut f = CredentialFile::default();
        f.connections
            .push(Connection::new("linear-2", "linear", "Client A"));
        f.providers.insert(
            "linear-2".into(),
            StoredCredential::from(&Credential::ApiKey("k2".into())),
        );
        let json = serde_json::to_string(&f).expect("serialize");
        let back: CredentialFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.connections, f.connections);
        assert!(back.providers.contains_key("linear-2"));
    }

    #[test]
    fn a_profile_with_no_connections_writes_no_connections_key() {
        // A file that only ever held a legacy login must not grow a key older
        // builds would not expect.
        let mut f = CredentialFile::default();
        f.providers.insert(
            "linear".into(),
            StoredCredential::from(&Credential::ApiKey("k".into())),
        );
        let json = serde_json::to_string(&f).expect("serialize");
        assert!(!json.contains("connections"), "got {json}");
    }

    #[test]
    fn an_unknown_kind_keeps_its_own_id_as_its_name() {
        let mut file = CredentialFile::default();
        file.providers.insert(
            "jira".into(),
            StoredCredential::from(&Credential::ApiKey("k".into())),
        );
        assert_eq!(connections_of(&file)[0].name, "jira");
    }

    #[test]
    fn corrupt_file_reads_as_empty_rather_than_panicking() {
        let dir = std::env::temp_dir().join(format!("okena-tasks-store-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let at = dir.join("corrupt.json");
        std::fs::write(&at, "{ not json").expect("write fixture");
        assert!(read_file(&at).providers.is_empty());
        let _ = std::fs::remove_file(&at);
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let at = std::env::temp_dir().join("okena-tasks-store-does-not-exist.json");
        let _ = std::fs::remove_file(&at);
        assert!(read_file(&at).providers.is_empty());
    }
}
