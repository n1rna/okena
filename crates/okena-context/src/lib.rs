//! Launch context (QBL-406): what an agent can be handed beside its goal.
//!
//! - [`items`] reads one root into items: a project map's entries, an OpenSpec
//!   root's specs and changes, a knowledge root's docs, skills and agents, a
//!   freeform origin's documents.
//! - [`catalog`] lists the roots, who owns them and which projects follow
//!   them, and resolves a client's refs again.
//! - `index` (feature `index`, the daemon's) is the live, watched, ranked
//!   search over a catalog, on fff.
//!
//! Roots themselves come from the discovery okena already runs for the
//! Library (`okena_knowledge::discover`, `okena_openspec::discover`, the
//! freeform folders a space lists); this crate only says what is inside them.

pub mod catalog;
#[cfg(feature = "index")]
pub mod index;
pub mod items;
