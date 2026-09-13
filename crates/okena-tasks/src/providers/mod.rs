//! Concrete task providers.
//!
//! Each provider translates one task-manager platform into the neutral types in
//! [`crate::task`]. Adding another means adding a module here, implementing
//! [`crate::provider::TaskProvider`] and registering it in `provider_for`.

pub mod azure_devops;
mod html;
pub mod linear;

/// The HTTP bus's mock is process-global, so provider tests that mock it take
/// turns — across providers, not only within one.
#[cfg(test)]
pub(crate) static NET: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use azure_devops::AzureDevOpsProvider;
pub use linear::LinearProvider;
