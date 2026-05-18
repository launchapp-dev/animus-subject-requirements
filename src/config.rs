//! Environment-driven configuration for the requirements backend plugin.
//!
//! All fields are populated from environment variables so the plugin can be
//! launched as a stdio child process without command-line argument plumbing.

use std::path::PathBuf;

use anyhow::Result;

/// Environment variable holding the root directory for requirement files.
pub const ENV_ROOT: &str = "ANIMUS_REQUIREMENTS_ROOT";

/// Environment variable holding the id prefix (e.g. `REQ` -> `REQ-0001`).
pub const ENV_ID_PREFIX: &str = "ANIMUS_REQUIREMENTS_ID_PREFIX";

/// Environment variable holding the index cache TTL in seconds.
pub const ENV_INDEX_TTL_SECS: &str = "ANIMUS_REQUIREMENTS_INDEX_TTL_SECS";

/// Environment variable some Animus runners use to surface the active project
/// root to plugins. Used to compute the default requirements root when
/// [`ENV_ROOT`] is unset.
pub const ENV_PROJECT_ROOT: &str = "ANIMUS_PROJECT_ROOT";

/// Default id prefix for requirement records.
pub const DEFAULT_ID_PREFIX: &str = "REQ";

/// Default index cache TTL in seconds.
pub const DEFAULT_INDEX_TTL_SECS: u64 = 60;

/// Runtime configuration for the requirements backend.
#[derive(Debug, Clone)]
pub struct RequirementsConfig {
    /// Root directory under which `REQ-NNNN.md` files live. Defaults to
    /// `<project_root>/.animus/requirements`.
    pub root: PathBuf,
    /// Prefix for new requirement ids (e.g. `REQ` -> `REQ-0001`).
    pub id_prefix: String,
    /// `_index.json` cache TTL — after this many seconds the index is
    /// rebuilt from the filesystem on the next [`backend::list`] call.
    pub index_ttl_secs: u64,
}

impl RequirementsConfig {
    /// Read the configuration from environment variables.
    ///
    /// Lenient: missing variables fall back to sensible defaults so the
    /// plugin can answer `--manifest` and basic ops without a `.env` file.
    pub fn from_env() -> Result<Self> {
        let root = match std::env::var(ENV_ROOT).ok().filter(|s| !s.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => default_root(),
        };
        let id_prefix = std::env::var(ENV_ID_PREFIX)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_ID_PREFIX.to_string());
        let index_ttl_secs = std::env::var(ENV_INDEX_TTL_SECS)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_INDEX_TTL_SECS);
        Ok(Self {
            root,
            id_prefix,
            index_ttl_secs,
        })
    }

    /// In-memory builder. Useful for tests and embedders that don't want to
    /// round-trip through the process environment.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            id_prefix: DEFAULT_ID_PREFIX.to_string(),
            index_ttl_secs: DEFAULT_INDEX_TTL_SECS,
        }
    }

    /// Override the id prefix.
    pub fn with_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Override the index TTL.
    pub fn with_index_ttl_secs(mut self, ttl: u64) -> Self {
        self.index_ttl_secs = ttl;
        self
    }
}

fn default_root() -> PathBuf {
    let base = std::env::var(ENV_PROJECT_ROOT)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    base.join(".animus").join("requirements")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_uses_defaults_for_prefix_and_ttl() {
        let cfg = RequirementsConfig::new("/tmp/reqs");
        assert_eq!(cfg.id_prefix, "REQ");
        assert_eq!(cfg.index_ttl_secs, 60);
        assert_eq!(cfg.root, PathBuf::from("/tmp/reqs"));
    }

    #[test]
    fn builders_override_defaults() {
        let cfg = RequirementsConfig::new("/tmp/reqs")
            .with_id_prefix("RQ")
            .with_index_ttl_secs(5);
        assert_eq!(cfg.id_prefix, "RQ");
        assert_eq!(cfg.index_ttl_secs, 5);
    }
}
