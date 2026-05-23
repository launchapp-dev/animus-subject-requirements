//! Read-only access to the in-tree `core-state.json` requirements map.
//!
//! The store is constructed with a path that may or may not exist. All
//! ops short-circuit to "empty" when the file is missing so the standalone
//! plugin works identically against new projects with no legacy state.
//!
//! The store reads on every call — it does not cache. The legacy
//! requirements set is typically <100 entries on existing projects, and
//! the staleness window for the cache would be hard to reason about now
//! that the file is jointly owned with the in-tree daemon during the
//! transition.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::frontmatter::RequirementFile;
use crate::legacy_json::model::{
    legacy_to_requirement_file, LegacyCoreState, LegacyRequirementJson,
};

/// Read-only handle to the in-tree `core-state.json` requirements map.
#[derive(Debug, Clone)]
pub struct LegacyJsonStore {
    path: PathBuf,
}

impl LegacyJsonStore {
    /// Construct a store rooted at the given file path. The path is not
    /// required to exist — callers can probe via [`Self::is_present`].
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path to the underlying `core-state.json`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the legacy file exists on disk. Cheap; stats only.
    pub async fn is_present(&self) -> bool {
        tokio::fs::metadata(&self.path).await.is_ok()
    }

    /// Read the legacy file and return every entry as a
    /// [`RequirementFile`]. Returns an empty vector when the file does
    /// not exist.
    pub async fn list(&self) -> anyhow::Result<Vec<(String, RequirementFile)>> {
        let state = match self.load_state().await? {
            Some(s) => s,
            None => return Ok(Vec::new()),
        };
        let mut out: Vec<(String, RequirementFile)> = state
            .requirements
            .into_iter()
            .map(|(id, item)| {
                let file = legacy_to_requirement_file(&item);
                (id, file)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Read one legacy entry by native id (`REQ-NNNN`). Returns `None` if
    /// either the file or the entry is missing.
    pub async fn get(&self, native_id: &str) -> anyhow::Result<Option<RequirementFile>> {
        let state = match self.load_state().await? {
            Some(s) => s,
            None => return Ok(None),
        };
        Ok(state
            .requirements
            .get(native_id)
            .map(legacy_to_requirement_file))
    }

    /// Snapshot of the raw legacy items, used by the migrator.
    pub async fn raw_items(&self) -> anyhow::Result<Vec<LegacyRequirementJson>> {
        let state = match self.load_state().await? {
            Some(s) => s,
            None => return Ok(Vec::new()),
        };
        let mut items: Vec<_> = state.requirements.into_values().collect();
        items.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(items)
    }

    async fn load_state(&self) -> anyhow::Result<Option<LegacyCoreState>> {
        let raw = match tokio::fs::read_to_string(&self.path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", self.path.display())),
        };
        // The file may grow new top-level fields the plugin doesn't know
        // about — `serde(deny_unknown_fields)` is intentionally off.
        let state: LegacyCoreState = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {} as core-state", self.path.display()))?;
        Ok(Some(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn write_legacy(dir: &TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("core-state.json");
        std::fs::write(&path, body).expect("write fixture");
        path
    }

    #[tokio::test]
    async fn list_returns_empty_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let store = LegacyJsonStore::new(dir.path().join("nope.json"));
        let entries = store.list().await.expect("list");
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn list_returns_entries_with_rich_fields() {
        let dir = TempDir::new().unwrap();
        let body = r#"{
            "requirements": {
                "REQ-0001": {
                    "id": "REQ-0001",
                    "title": "Sample",
                    "description": "Body text",
                    "acceptance_criteria": ["ac-1", "ac-2"],
                    "priority": "must",
                    "status": "approved",
                    "tags": ["auth"],
                    "linked_task_ids": ["TASK-1"],
                    "comments": [],
                    "links": {"tasks": [], "workflows": ["wf-1"], "tests": [], "mockups": [], "flows": [], "related_requirements": []},
                    "created_at": "2026-05-01T00:00:00Z",
                    "updated_at": "2026-05-02T00:00:00Z"
                }
            }
        }"#;
        let path = write_legacy(&dir, body);
        let store = LegacyJsonStore::new(path);
        let entries = store.list().await.expect("list");
        assert_eq!(entries.len(), 1);
        let (id, file) = &entries[0];
        assert_eq!(id, "REQ-0001");
        assert_eq!(file.frontmatter.title, "Sample");
        assert_eq!(file.frontmatter.acceptance_criteria.len(), 2);
        assert_eq!(file.frontmatter.linked_workflows, vec!["wf-1"]);
    }

    #[tokio::test]
    async fn get_returns_none_for_missing_id() {
        let dir = TempDir::new().unwrap();
        let body = r#"{"requirements": {}}"#;
        let path = write_legacy(&dir, body);
        let store = LegacyJsonStore::new(path);
        assert!(store.get("REQ-0001").await.expect("get").is_none());
    }
}
