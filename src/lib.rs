//! Requirements subject backend plugin for Animus.
//!
//! This crate is consumed by `src/main.rs` (the stdio plugin binary) and by
//! `tests/contract.rs`. It exposes:
//!
//! - [`config::RequirementsConfig`] — environment-driven configuration
//! - [`store::RequirementsStore`] — read/write logic + `_index.json` caching
//! - [`backend::RequirementsBackend`] — the `SubjectBackend` implementation
//! - [`frontmatter`] — YAML frontmatter parsing with requirement-specific fields
//! - [`status_map`] — requirement native status -> [`SubjectStatus`]
//! - [`id_gen`] — sequential `REQ-NNNN` id generation
//! - [`watcher`] — `notify`-based file change watching
//!
//! Unlike upstream-API-backed backends (Linear, Jira, GitHub Issues), this
//! plugin OWNS the data. Requirements live as `.md` files with structured
//! YAML frontmatter under a project-local directory tree. Git-native: every
//! mutation is a file change reviewable in a PR.

pub mod backend;
pub mod config;
pub mod frontmatter;
pub mod id_gen;
pub mod legacy_json;
pub mod status_map;
pub mod store;
pub mod watcher;
