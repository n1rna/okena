//! Concrete task providers.
//!
//! Each provider translates one task-manager platform into the neutral types in
//! [`crate::task`]. Adding another means adding a module here, implementing
//! [`crate::provider::TaskProvider`] and registering it in `provider_for`.

pub mod azure_devops;
mod html;
pub mod linear;

pub use azure_devops::AzureDevOpsProvider;
pub use linear::LinearProvider;
