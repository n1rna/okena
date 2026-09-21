//! A named login to one task backend.
//!
//! In `okena-core` rather than `okena-tasks` because a connection crosses the
//! wire (a client lists them, names them and picks one for a space) and is
//! persisted in a space; `okena-tasks` owns the client side and re-exports it.
//!
//! Before spaces there was one login per provider *kind*, and the kind's id
//! (`"linear"`) was all anyone needed to name it. A space picks one backend to
//! read, and two spaces may want two different Linear accounts — so what gets
//! named is the *connection*, and the kind becomes one of its fields.
//!
//! The id is what travels: it is what a space stores, what `provider_for`
//! resolves, and the key the credential is filed under. The first connection
//! of a kind deliberately takes the kind's own id, so a profile that connected
//! Linear before spaces existed already has a connection called `linear`
//! holding the credential it always had — nobody signs in again.

use serde::{Deserialize, Serialize};

/// One saved login, told apart from the others by `name`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    /// Stable id. Also the key its credential is stored under.
    pub id: String,
    /// Which backend this talks to: a provider id such as `linear`.
    pub kind: String,
    /// What to call it in the UI. Free text — "Acme Linear", "Personal".
    pub name: String,
}

impl Connection {
    pub fn new(id: impl Into<String>, kind: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
        }
    }
}

/// What to call a provider kind before a connection has its own name.
///
/// Falls back to the kind itself so a build that meets an id it does not know
/// shows that id rather than an empty label.
pub fn kind_display_name(kind: &str) -> String {
    match kind {
        "linear" => "Linear".to_string(),
        "azure_devops" => "Azure DevOps".to_string(),
        other => other.to_string(),
    }
}

/// Mint an id for a new connection of `kind`, given the ids already taken.
///
/// The first one is the bare kind, which is what makes a pre-spaces credential
/// readable without a migration step. After that they are numbered.
pub fn mint_id(kind: &str, taken: &[String]) -> String {
    let is_taken = |id: &str| taken.iter().any(|t| t == id);
    if !is_taken(kind) {
        return kind.to_string();
    }
    // Starts at 2 because the bare kind is conceptually the first. Searching
    // one past the number taken always finds a free id (there are more
    // candidates than ids in use), so the fallback is unreachable and exists
    // only to keep this total.
    (2..=taken.len() + 2)
        .map(|n| format!("{kind}-{n}"))
        .find(|id| !is_taken(id))
        .unwrap_or_else(|| format!("{kind}-{}", taken.len() + 2))
}

#[cfg(test)]
mod tests {
    use super::mint_id;

    #[test]
    fn the_first_connection_of_a_kind_takes_the_kinds_own_id() {
        // This is the whole migration: an existing `linear` credential is
        // already filed under the id the first Linear connection will have.
        assert_eq!(mint_id("linear", &[]), "linear");
    }

    #[test]
    fn a_second_account_of_the_same_kind_is_numbered() {
        assert_eq!(mint_id("linear", &["linear".to_string()]), "linear-2");
        assert_eq!(
            mint_id("linear", &["linear".to_string(), "linear-2".to_string()]),
            "linear-3"
        );
    }

    #[test]
    fn a_gap_left_by_a_removed_connection_is_reused() {
        assert_eq!(
            mint_id("linear", &["linear".to_string(), "linear-3".to_string()]),
            "linear-2"
        );
    }

    #[test]
    fn kinds_do_not_collide_with_each_other() {
        let taken = vec!["linear".to_string(), "linear-2".to_string()];
        assert_eq!(mint_id("azure_devops", &taken), "azure_devops");
    }
}
