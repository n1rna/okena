//! Runs extensions installed from git as WASM components, in the daemon.
//!
//! - [`manifest`] reads `extension.toml`.
//! - [`permissions`] enforces what the user approved on every host call.
//! - [`deps`] is the dependency check on required tools.
//! - [`runtime`] is the wasmtime host and the calls an extension may make.
//! - [`install`] and [`store`] fetch, build and record extensions on disk.
//! - [`host`] is what the daemon runs: every extension, a worker for each
//!   enabled one, and install / update / remove.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod convert;
pub mod deps;
pub mod exec;
pub mod host;
pub mod install;
pub mod kv;
pub mod manifest;
pub mod permissions;
pub mod runtime;
pub mod store;

mod bindings {
    wasmtime::component::bindgen!({
        path: "../okena-extension-api/wit",
        world: "extension",
    });
}
