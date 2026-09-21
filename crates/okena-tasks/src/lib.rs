#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! okena-tasks — task-manager integration for the engineering harness.
//!
//! GPUI-free by design: the daemon owns task state (ADR-0001), so this crate
//! must be usable from `okena-daemon`. Nothing here may depend on gpui.
//!
//! Layering mirrors `okena-core` / `okena-transport`: the provider-neutral
//! types live in [`okena_core::tasks`] (so `okena-state` can persist a
//! `TaskRef` without inheriting this crate's networking dependencies), while
//! this crate owns the client side — auth and the per-platform providers.
//!
//! - [`provider`] — the `TaskProvider` trait every platform implements.
//! - [`providers`] — concrete implementations (Linear, Azure DevOps).
//! - [`store`] — per-profile credential persistence.

pub mod provider;
pub mod providers;
pub mod store;

/// Connection types live in `okena-core` (they cross the wire); re-exported
/// here because this crate is where connections are used.
pub use okena_core::connections::{Connection, kind_display_name, mint_id};
pub use okena_core::tasks::{Task, TaskId, TaskRef, TaskScope, TaskState};
pub use provider::{AuthStatus, Credential, TaskError, TaskPatch, TaskProvider, task_branch_name};
pub use providers::{AzureDevOpsProvider, LinearProvider};

/// Build the provider for `connection_id` using the credential stored under
/// it, or `None` when it resolves to no backend this build knows.
///
/// The argument names a *connection*, not a provider kind — two spaces can
/// read two different Linear accounts, and only the connection tells them
/// apart. An id no connection claims is read as a kind, which is exactly what
/// it was before spaces existed: `provider_for("linear")` still builds Linear
/// against the credential filed under `linear`.
///
/// A provider is constructed per call rather than cached: the credential can
/// change under us (connect / disconnect / refresh) and these are cheap structs
/// wrapping a token, so a stale cached instance would be the only real hazard.
pub fn provider_for(connection_id: &str) -> Option<Box<dyn TaskProvider>> {
    let kind = store::connection(connection_id)
        .map(|c| c.kind)
        .unwrap_or_else(|| connection_id.to_string());
    let credential = store::load(connection_id);
    match kind.as_str() {
        "linear" => Some(Box::new(LinearProvider::new(credential))),
        providers::azure_devops::PROVIDER_ID => {
            Some(Box::new(AzureDevOpsProvider::new(credential)))
        }
        _ => None,
    }
}

/// Provider kinds this build knows about, in display order.
pub const KNOWN_PROVIDERS: &[&str] = &["linear", providers::azure_devops::PROVIDER_ID];

/// The provider kind the harness uses when settings name none.
pub const DEFAULT_PROVIDER: &str = "linear";

