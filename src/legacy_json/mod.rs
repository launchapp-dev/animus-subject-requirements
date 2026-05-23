//! Legacy in-tree JSON compatibility for the requirements backend.
//!
//! Animus' pre-plugin requirements lived inside a single
//! `core-state.json` file under `~/.animus/<repo-scope>/`. Each entry was
//! a [`crate::legacy_json::model::LegacyRequirementJson`] keyed by id in
//! a flat `requirements` map.
//!
//! This module lets the standalone subject backend keep working against
//! projects that still have legacy data:
//!
//! - [`store::LegacyJsonStore`] reads the legacy file and surfaces each
//!   entry as a `RequirementFile` (frontmatter + body).
//! - [`migrate::migrate_legacy_to_markdown`] one-shot converts the JSON
//!   file into individual `REQ-NNNN.md` files under the standalone
//!   plugin's root directory.
//!
//! Read-only legacy compat is always on whenever a `core-state.json`
//! exists at the configured legacy path. Migration is opt-in via the
//! [`crate::config::ENV_MIGRATE_LEGACY`] env var.

pub mod migrate;
pub mod model;
pub mod store;
