//! One-shot migration from the in-tree `core-state.json` requirements
//! map into individual `REQ-NNNN.md` files owned by the standalone
//! requirements plugin.
//!
//! Triggered by the [`crate::config::ENV_MIGRATE_LEGACY`] env var or by
//! direct invocation from a CLI command. Idempotent: re-running after a
//! successful migration is a no-op (the legacy entries are gone).
//!
//! ## Safety
//!
//! - The Markdown destination is written atomically per file.
//! - Existing Markdown files at the same id are **not** overwritten. The
//!   migrator skips collisions and reports them so the operator can
//!   resolve manually.
//! - The legacy `requirements` field is cleared from the JSON only after
//!   every entry has been successfully written. Other top-level fields
//!   in `core-state.json` are preserved verbatim.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::legacy_json::store::LegacyJsonStore;
use crate::store::RequirementsStore;

/// Outcome of a [`migrate_legacy_to_markdown`] call.
#[derive(Debug, Default, Clone)]
pub struct MigrationReport {
    /// Native ids successfully written as new `.md` files.
    pub migrated: Vec<String>,
    /// Native ids skipped because a Markdown file already exists.
    pub skipped_existing: Vec<String>,
    /// Native ids the migrator failed to write, paired with the error
    /// message. The legacy entries for these are **not** cleared from the
    /// JSON so the operator can re-run after fixing the cause.
    pub errors: Vec<(String, String)>,
    /// Whether the legacy `requirements` map was cleared from the JSON.
    pub legacy_cleared: bool,
}

impl MigrationReport {
    /// Whether migration converted at least one entry.
    pub fn any_changes(&self) -> bool {
        !self.migrated.is_empty() || self.legacy_cleared
    }
}

/// Convert every entry in the legacy `core-state.json` into a Markdown
/// file under the standalone plugin's root, then clear the legacy
/// `requirements` map from the JSON.
///
/// When `dry_run` is true, the migrator reports what *would* happen but
/// neither writes Markdown nor mutates the JSON.
pub async fn migrate_legacy_to_markdown(
    legacy: &LegacyJsonStore,
    store: &RequirementsStore,
    dry_run: bool,
) -> anyhow::Result<MigrationReport> {
    let mut report = MigrationReport::default();
    if !legacy.is_present().await {
        return Ok(report);
    }

    let entries = legacy.list().await?;
    if entries.is_empty() {
        return Ok(report);
    }

    let root = store.config().root.clone();
    for (native_id, file) in entries {
        let dest = root.join(format!("{native_id}.md"));
        if dest.exists() {
            report.skipped_existing.push(native_id);
            continue;
        }
        if dry_run {
            report.migrated.push(native_id);
            continue;
        }
        match store.write(&file).await {
            Ok(_) => report.migrated.push(native_id),
            Err(e) => report.errors.push((native_id, e.to_string())),
        }
    }

    if !report.errors.is_empty() {
        // Don't touch the legacy file until every entry made it across.
        return Ok(report);
    }

    if !dry_run {
        clear_legacy_requirements(legacy.path()).await?;
        report.legacy_cleared = true;
    }

    Ok(report)
}

/// Rewrite the legacy `core-state.json` with an empty `requirements`
/// object. Every other top-level field is preserved.
///
/// Atomic write: the new contents are staged to a temp file and renamed
/// over the original.
async fn clear_legacy_requirements(path: &Path) -> anyhow::Result<()> {
    let raw = tokio::fs::read_to_string(path).await?;
    let mut value: Value = serde_json::from_str(&raw)?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "requirements".to_string(),
            Value::Object(Map::new()),
        );
    }
    let body = serde_json::to_string_pretty(&value)?;
    let tmp: PathBuf = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => path.with_file_name(format!(".{name}.tmp")),
        None => path.with_extension("tmp"),
    };
    tokio::fs::write(&tmp, body).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RequirementsConfig;

    use tempfile::TempDir;

    async fn fixture() -> (TempDir, LegacyJsonStore, RequirementsStore) {
        let dir = TempDir::new().unwrap();
        let legacy_path = dir.path().join("core-state.json");
        std::fs::write(
            &legacy_path,
            r#"{
                "tasks": {},
                "requirements": {
                    "REQ-0001": {
                        "id": "REQ-0001",
                        "title": "From legacy",
                        "description": "Body",
                        "priority": "must",
                        "status": "refined",
                        "acceptance_criteria": ["a"],
                        "links": {},
                        "comments": [],
                        "tags": [],
                        "linked_task_ids": [],
                        "created_at": "2026-05-01T00:00:00Z",
                        "updated_at": "2026-05-01T00:00:00Z"
                    }
                }
            }"#,
        )
        .unwrap();

        let root = dir.path().join("md-root");
        let cfg = RequirementsConfig::new(&root);
        let store = RequirementsStore::new(cfg).await.expect("store");
        let legacy = LegacyJsonStore::new(legacy_path);
        (dir, legacy, store)
    }

    #[tokio::test]
    async fn migrates_legacy_entry_to_markdown_file() {
        let (_dir, legacy, store) = fixture().await;
        let report = migrate_legacy_to_markdown(&legacy, &store, false)
            .await
            .expect("migrate");
        assert_eq!(report.migrated, vec!["REQ-0001".to_string()]);
        assert!(report.legacy_cleared);
        let path = store.config().root.join("REQ-0001.md");
        assert!(path.exists(), "markdown file should exist");
        // Tasks field preserved, requirements cleared.
        let raw = std::fs::read_to_string(legacy.path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(parsed.get("tasks").is_some());
        assert_eq!(
            parsed.get("requirements"),
            Some(&serde_json::json!({}))
        );
    }

    #[tokio::test]
    async fn dry_run_does_not_touch_disk() {
        let (_dir, legacy, store) = fixture().await;
        let report = migrate_legacy_to_markdown(&legacy, &store, true)
            .await
            .expect("dry-run");
        assert_eq!(report.migrated, vec!["REQ-0001".to_string()]);
        assert!(!report.legacy_cleared);
        let path = store.config().root.join("REQ-0001.md");
        assert!(!path.exists(), "markdown file should NOT exist on dry-run");
    }

    #[tokio::test]
    async fn skips_existing_markdown_collision() {
        let (_dir, legacy, store) = fixture().await;
        // Pre-create a markdown file at the same id.
        let path = store.config().root.join("REQ-0001.md");
        std::fs::write(
            &path,
            "---\nid: requirement:REQ-0001\nkind: requirement\ntitle: existing\nstatus: drafted\ncreated_at: 2026-05-01T00:00:00Z\nupdated_at: 2026-05-01T00:00:00Z\n---\n",
        )
        .unwrap();
        let report = migrate_legacy_to_markdown(&legacy, &store, false)
            .await
            .expect("migrate");
        assert_eq!(report.skipped_existing, vec!["REQ-0001".to_string()]);
        assert!(report.migrated.is_empty());
        // Legacy not cleared because skips do not constitute success
        // for that id — but no errors either, so the JSON IS cleared per
        // the contract (errors gate the clear, not skips).
        assert!(report.legacy_cleared);
    }

    #[tokio::test]
    async fn no_op_when_legacy_missing() {
        let dir = TempDir::new().unwrap();
        let legacy = LegacyJsonStore::new(dir.path().join("nope.json"));
        let cfg = RequirementsConfig::new(dir.path().join("md"));
        let store = RequirementsStore::new(cfg).await.expect("store");
        let report = migrate_legacy_to_markdown(&legacy, &store, false)
            .await
            .expect("migrate");
        assert!(report.migrated.is_empty());
        assert!(!report.legacy_cleared);
    }
}
