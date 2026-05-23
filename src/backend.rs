//! [`RequirementsBackend`] — `SubjectBackend` impl for the requirements
//! plugin.
//!
//! The backend speaks the normalized `subject/*` JSON-RPC vocabulary on
//! the wire and translates to/from the on-disk requirement format via
//! [`RequirementsStore`] and [`Frontmatter`].
//!
//! Id convention: `requirement:REQ-NNNN`. The native portion (`REQ-NNNN`)
//! doubles as the on-disk filename.

use std::collections::BTreeMap;
use std::sync::Arc;

use animus_plugin_protocol::{HealthCheckResult, HealthStatus};
use animus_subject_protocol::{
    BackendError, CustomFieldKind, CustomFieldSpec, EventStream, StatusDispatchHint, Subject,
    SubjectBackend, SubjectFilter, SubjectId, SubjectList, SubjectPatch, SubjectSchema,
    SubjectStatus,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value as JsonValue;

use crate::config::RequirementsConfig;
use crate::frontmatter::{Frontmatter, RequirementFile};
use crate::legacy_json::migrate::{migrate_legacy_to_markdown, MigrationReport};
use crate::legacy_json::store::LegacyJsonStore;
use crate::status_map::{self, RequirementNativeStatus};
use crate::store::{
    full_id_from_native, native_id_from_full, next_id, CachedEntry, RequirementsStore, StoreError,
};
use crate::watcher;

const KIND: &str = "requirement";

/// The backend.
#[derive(Debug, Clone)]
pub struct RequirementsBackend {
    store: Arc<RequirementsStore>,
    legacy: Option<Arc<LegacyJsonStore>>,
}

impl RequirementsBackend {
    /// Build a new backend from a [`RequirementsConfig`]. Ensures the
    /// root directory exists. When `config.legacy_json_path` is set, the
    /// legacy store is attached and (if `migrate_legacy_on_start` is true
    /// and the legacy file exists) a one-shot migration runs before the
    /// backend returns.
    pub async fn new(config: RequirementsConfig) -> anyhow::Result<Self> {
        let legacy_path = config.legacy_json_path.clone();
        let migrate_on_start = config.migrate_legacy_on_start;
        let store = RequirementsStore::new(config).await?;
        let legacy = legacy_path.map(|p| Arc::new(LegacyJsonStore::new(p)));

        if migrate_on_start {
            if let Some(legacy_store) = legacy.as_ref() {
                let report = migrate_legacy_to_markdown(legacy_store.as_ref(), &store, false)
                    .await
                    .map_err(|e| anyhow::anyhow!("legacy migration failed: {e}"))?;
                log_migration_report(&report);
            }
        }

        Ok(Self {
            store: Arc::new(store),
            legacy,
        })
    }

    /// Borrow the underlying store. Tests use this to introspect state.
    pub fn store(&self) -> &Arc<RequirementsStore> {
        &self.store
    }

    /// Borrow the legacy store, if compatibility is enabled.
    pub fn legacy_store(&self) -> Option<&Arc<LegacyJsonStore>> {
        self.legacy.as_ref()
    }

    /// Delete a requirement by id. Deletes the on-disk Markdown file if
    /// present; if the requirement only exists in legacy JSON, the call
    /// is a no-op and returns `Ok(false)`. Returns `Ok(true)` when a
    /// Markdown file was deleted.
    ///
    /// Note: not part of the [`SubjectBackend`] trait surface (which has
    /// no `delete` method as of `animus-subject-protocol` v0.1.6). The
    /// in-tree adapter exposed this op directly to CLI callers; the
    /// equivalent here is a typed method invoked from in-process glue.
    pub async fn delete(&self, id: &SubjectId) -> Result<bool, BackendError> {
        let native = Self::native_id(id)?;
        let path = self.store.path_for(&native);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                let _ = self.store.rebuild_index().await.map_err(map_store_err)?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No-op when the entry only ever lived in legacy JSON.
                // Hard-delete from the legacy file is intentionally not
                // supported here — operators should migrate first, then
                // delete, so the two stores stay strictly read-only-from-
                // legacy / writable-to-markdown.
                Ok(false)
            }
            Err(e) => Err(BackendError::Other(anyhow::anyhow!(
                "delete failed at {}: {e}",
                path.display()
            ))),
        }
    }

    /// Convert a [`SubjectId`] into the native (`REQ-NNNN`) portion.
    fn native_id(id: &SubjectId) -> Result<String, BackendError> {
        let raw = id.as_str();
        native_id_from_full(raw).ok_or_else(|| {
            BackendError::InvalidRequest(format!(
                "subject id {raw:?} is not a requirement id (expected `requirement:<native>`)"
            ))
        })
    }

    /// Apply the patch to a [`Frontmatter`] in-place. Returns the
    /// previous native status so callers can decide whether to stamp
    /// `refined_at` / emit a status-changed event.
    fn apply_patch(
        frontmatter: &mut Frontmatter,
        patch: &SubjectPatch,
    ) -> Result<(RequirementNativeStatus, RequirementNativeStatus), BackendError> {
        let previous = frontmatter.status;

        if let Some(status) = patch.status {
            frontmatter.status = status_map::subject_to_native(status);
        }

        if let Some(assignee_patch) = &patch.assignee {
            match assignee_patch {
                Some(value) => {
                    frontmatter
                        .custom_fields
                        .insert("assignee".to_string(), JsonValue::String(value.clone()));
                }
                None => {
                    frontmatter.custom_fields.remove("assignee");
                }
            }
        }

        if !patch.labels_add.is_empty() || !patch.labels_remove.is_empty() {
            let removals: std::collections::HashSet<&str> =
                patch.labels_remove.iter().map(String::as_str).collect();
            frontmatter
                .labels
                .retain(|label| !removals.contains(label.as_str()));
            for label in &patch.labels_add {
                if !frontmatter.labels.iter().any(|l| l == label) {
                    frontmatter.labels.push(label.clone());
                }
            }
        }

        for (key, value) in &patch.custom {
            if value.is_null() {
                frontmatter.custom_fields.remove(key);
            } else {
                frontmatter.custom_fields.insert(key.clone(), value.clone());
            }
        }

        if let Some(comment) = &patch.comment {
            // Record the comment under `custom_fields.last_comment`. We
            // don't reorder the body to avoid surprising humans editing
            // the file in parallel.
            frontmatter.custom_fields.insert(
                "last_comment".to_string(),
                JsonValue::String(comment.clone()),
            );
        }

        Ok((previous, frontmatter.status))
    }

    /// Same logic as [`Self::passes_filter`] but operating on an
    /// already-constructed [`Subject`] (used for legacy-JSON-sourced
    /// entries that bypass the cached index path).
    fn passes_subject_filter(
        subject: &Subject,
        frontmatter: &Frontmatter,
        filter: &SubjectFilter,
    ) -> bool {
        if !filter.kind.is_empty() && !filter.kind.iter().any(|k| k == &subject.kind) {
            return false;
        }
        if !filter.status.is_empty() && !filter.status.contains(&subject.status) {
            return false;
        }
        if let Some(native) = &filter.native_status {
            if frontmatter.status.as_str() != native {
                return false;
            }
        }
        if !filter.assignee.is_empty() {
            match subject.assignee.as_deref() {
                Some(value) if filter.assignee.iter().any(|a| a == value) => {}
                _ => return false,
            }
        }
        if !filter.labels_any.is_empty()
            && !filter
                .labels_any
                .iter()
                .any(|wanted| subject.labels.iter().any(|l| l == wanted))
        {
            return false;
        }
        if !filter.labels_all.is_empty()
            && !filter
                .labels_all
                .iter()
                .all(|wanted| subject.labels.iter().any(|l| l == wanted))
        {
            return false;
        }
        if let Some(updated_since) = filter.updated_since {
            if subject.updated_at < updated_since {
                return false;
            }
        }
        true
    }

    fn passes_filter(entry: &CachedEntry, filter: &SubjectFilter) -> bool {
        if entry.archived {
            return false;
        }
        if !filter.kind.is_empty() && !filter.kind.iter().any(|k| k == KIND) {
            return false;
        }
        if !filter.status.is_empty() {
            let normalized = status_map::native_to_subject(entry.frontmatter.status);
            if !filter.status.contains(&normalized) {
                return false;
            }
        }
        if let Some(native) = &filter.native_status {
            if entry.frontmatter.status.as_str() != native {
                return false;
            }
        }
        if !filter.assignee.is_empty() {
            let actual = entry
                .frontmatter
                .custom_fields
                .get("assignee")
                .and_then(JsonValue::as_str);
            match actual {
                Some(value) if filter.assignee.iter().any(|a| a == value) => {}
                _ => return false,
            }
        }
        if !filter.labels_any.is_empty()
            && !filter
                .labels_any
                .iter()
                .any(|wanted| entry.frontmatter.labels.iter().any(|l| l == wanted))
        {
            return false;
        }
        if !filter.labels_all.is_empty()
            && !filter
                .labels_all
                .iter()
                .all(|wanted| entry.frontmatter.labels.iter().any(|l| l == wanted))
        {
            return false;
        }
        if let Some(updated_since) = filter.updated_since {
            if entry.frontmatter.updated_at < updated_since {
                return false;
            }
        }
        true
    }
}

#[async_trait]
impl SubjectBackend for RequirementsBackend {
    async fn list(&self, filter: SubjectFilter) -> Result<SubjectList, BackendError> {
        let index = self.store.load_index().await.map_err(map_store_err)?;
        let mut subjects: Vec<Subject> = index
            .entries
            .iter()
            .filter(|(_, entry)| Self::passes_filter(entry, &filter))
            .map(|(native, entry)| cached_entry_to_subject(native, entry))
            .collect();

        // Union legacy JSON entries. Markdown wins on id collision —
        // every native id already in `subjects` is skipped on the legacy
        // side. Filters are applied to the synthesized Subject.
        if let Some(legacy) = self.legacy.as_ref() {
            let known: std::collections::HashSet<String> =
                subjects.iter().map(|s| s.id.0.clone()).collect();
            match legacy.list().await {
                Ok(entries) => {
                    for (native, file) in entries {
                        let full_id = full_id_from_native(&native);
                        if known.contains(&full_id) {
                            continue;
                        }
                        let subject = frontmatter_to_subject(&native, &file.frontmatter, false);
                        if Self::passes_subject_filter(&subject, &file.frontmatter, &filter) {
                            subjects.push(subject);
                        }
                    }
                }
                Err(e) => {
                    // Legacy compat is best-effort; a broken legacy file
                    // must not take the whole backend down. Surface as
                    // a tracing warning instead.
                    tracing::warn!(error = %e, "legacy_json list failed; serving Markdown only");
                }
            }
        }

        // Stable: by updated_at descending then id ascending.
        subjects.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.0.cmp(&b.id.0)));

        let limit = filter.limit.unwrap_or(50).clamp(1, 500) as usize;
        let start = filter
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or_default();
        let end = (start + limit).min(subjects.len());
        let page = subjects[start..end].to_vec();
        let next_cursor = if end < subjects.len() {
            Some(end.to_string())
        } else {
            None
        };

        Ok(SubjectList {
            subjects: page,
            next_cursor,
            fetched_at: Utc::now(),
        })
    }

    async fn get(&self, id: &SubjectId) -> Result<Subject, BackendError> {
        let native = Self::native_id(id)?;
        match self.store.read(&native).await {
            Ok(file) => Ok(frontmatter_to_subject(&native, &file.frontmatter, false)),
            Err(StoreError::NotFound(_)) => {
                if let Some(legacy) = self.legacy.as_ref() {
                    if let Some(file) = legacy
                        .get(&native)
                        .await
                        .map_err(|e| BackendError::Other(anyhow::anyhow!(e)))?
                    {
                        return Ok(frontmatter_to_subject(&native, &file.frontmatter, false));
                    }
                }
                Err(BackendError::NotFound(native))
            }
            Err(other) => Err(map_store_err(other)),
        }
    }

    async fn update(&self, id: &SubjectId, patch: SubjectPatch) -> Result<Subject, BackendError> {
        let native = Self::native_id(id)?;
        let mut file = self.store.read(&native).await.map_err(map_store_err)?;
        let (previous, next_status) = Self::apply_patch(&mut file.frontmatter, &patch)?;
        let now = Utc::now();
        file.frontmatter.updated_at = now;
        if crate::store::should_stamp_refinement(previous, next_status) {
            file.frontmatter.refined_at = Some(now);
            if let Some(comment) = &patch.comment {
                file.frontmatter.refined_by = Some(comment.clone());
            } else if let Some(JsonValue::String(actor)) = patch.custom.get("refined_by") {
                file.frontmatter.refined_by = Some(actor.clone());
            }
        }
        if let Some(JsonValue::String(actor)) = patch.custom.get("refined_by") {
            file.frontmatter.refined_by = Some(actor.clone());
        }
        let archived = matches!(next_status, RequirementNativeStatus::Deprecated)
            && self
                .store
                .path_for(&native)
                .starts_with(self.store.config().root.join("archived"));
        let _ = archived; // archival is opt-in; deprecation alone does not move files in v0.1.
        self.store.write(&file).await.map_err(map_store_err)?;
        // Invalidate the in-memory cache so a subsequent list() re-reads.
        let _ = self.store.rebuild_index().await.map_err(map_store_err)?;
        Ok(frontmatter_to_subject(&native, &file.frontmatter, false))
    }

    async fn watch(&self) -> Option<EventStream> {
        watcher::spawn(self.store.clone())
    }

    fn schema(&self) -> SubjectSchema {
        let native_status_values = RequirementNativeStatus::ALL
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();
        let status_dispatch_hints = RequirementNativeStatus::ALL
            .iter()
            .map(|s| StatusDispatchHint {
                native_status: s.as_str().to_string(),
                maps_to: status_map::native_to_subject(*s),
                dispatch_label: Some(dispatch_label_for(*s).to_string()),
                description: Some(description_for(*s).to_string()),
            })
            .collect();
        SubjectSchema {
            kinds: vec![KIND.to_string()],
            status_values: vec![
                SubjectStatus::Ready,
                SubjectStatus::InProgress,
                SubjectStatus::Done,
                SubjectStatus::Cancelled,
            ],
            supports_watch: true,
            supports_create: true,
            supports_pagination: true,
            native_status_values,
            status_dispatch_hints,
            custom_fields: vec![
                CustomFieldSpec {
                    key: "priority".to_string(),
                    kind: CustomFieldKind::String,
                    values: None,
                },
                CustomFieldSpec {
                    key: "linked_tasks".to_string(),
                    kind: CustomFieldKind::String,
                    values: None,
                },
                CustomFieldSpec {
                    key: "linked_workflows".to_string(),
                    kind: CustomFieldKind::String,
                    values: None,
                },
                CustomFieldSpec {
                    key: "acceptance_criteria".to_string(),
                    kind: CustomFieldKind::String,
                    values: None,
                },
                CustomFieldSpec {
                    key: "refined_at".to_string(),
                    kind: CustomFieldKind::Date,
                    values: None,
                },
                CustomFieldSpec {
                    key: "refined_by".to_string(),
                    kind: CustomFieldKind::String,
                    values: None,
                },
            ],
        }
    }

    async fn health(&self) -> Result<HealthCheckResult, BackendError> {
        let root = &self.store.config().root;
        // Probe read + write access by stat + a tmp write.
        let mut last_error: Option<String> = None;
        if let Err(e) = tokio::fs::metadata(root).await {
            last_error = Some(format!("root unreadable: {e}"));
        } else {
            let probe = root.join(".animus-health-probe");
            match tokio::fs::write(&probe, b"ok").await {
                Ok(()) => {
                    let _ = tokio::fs::remove_file(&probe).await;
                }
                Err(e) => last_error = Some(format!("root unwritable: {e}")),
            }
        }
        let status = if last_error.is_some() {
            HealthStatus::Unhealthy
        } else {
            HealthStatus::Healthy
        };
        Ok(HealthCheckResult {
            status,
            uptime_ms: None,
            memory_usage_bytes: None,
            last_error,
        })
    }
}

/// Build a [`Subject`] from a cached entry. Used by both `list()` and the
/// watcher event builder.
pub fn cached_entry_to_subject(native_id: &str, entry: &CachedEntry) -> Subject {
    frontmatter_to_subject(native_id, &entry.frontmatter, entry.archived)
}

fn frontmatter_to_subject(native_id: &str, frontmatter: &Frontmatter, archived: bool) -> Subject {
    let normalized = status_map::native_to_subject(frontmatter.status);
    let mut custom: BTreeMap<String, JsonValue> = BTreeMap::new();
    if !frontmatter.acceptance_criteria.is_empty() {
        custom.insert(
            "acceptance_criteria".to_string(),
            JsonValue::Array(
                frontmatter
                    .acceptance_criteria
                    .iter()
                    .map(|s| JsonValue::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !frontmatter.linked_tasks.is_empty() {
        custom.insert(
            "linked_tasks".to_string(),
            JsonValue::Array(
                frontmatter
                    .linked_tasks
                    .iter()
                    .map(|s| JsonValue::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !frontmatter.linked_workflows.is_empty() {
        custom.insert(
            "linked_workflows".to_string(),
            JsonValue::Array(
                frontmatter
                    .linked_workflows
                    .iter()
                    .map(|s| JsonValue::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(refined_at) = frontmatter.refined_at {
        custom.insert(
            "refined_at".to_string(),
            JsonValue::String(refined_at.to_rfc3339()),
        );
    }
    if let Some(refined_by) = &frontmatter.refined_by {
        custom.insert(
            "refined_by".to_string(),
            JsonValue::String(refined_by.clone()),
        );
    }
    if let Some(priority) = &frontmatter.priority {
        custom.insert(
            "priority_label".to_string(),
            JsonValue::String(priority.clone()),
        );
    }
    if archived {
        custom.insert("archived".to_string(), JsonValue::Bool(true));
    }
    for (k, v) in &frontmatter.custom_fields {
        custom.insert(k.clone(), v.clone());
    }

    let assignee = frontmatter
        .custom_fields
        .get("assignee")
        .and_then(JsonValue::as_str)
        .map(str::to_string);

    Subject {
        id: SubjectId::new(full_id_from_native(native_id)),
        kind: KIND.to_string(),
        title: frontmatter.title.clone(),
        description: None,
        status: normalized,
        priority: priority_to_u8(frontmatter.priority.as_deref()),
        assignee,
        labels: frontmatter.labels.clone(),
        parent: frontmatter.parent_id.clone().map(SubjectId::new),
        children: frontmatter
            .linked_tasks
            .iter()
            .map(|t| SubjectId::new(t.clone()))
            .collect(),
        url: None,
        created_at: frontmatter.created_at,
        updated_at: frontmatter.updated_at,
        custom,
        native_status: Some(frontmatter.status.as_str().to_string()),
        status_metadata: JsonValue::Null,
        attachments: Vec::new(),
    }
}

fn priority_to_u8(label: Option<&str>) -> Option<u8> {
    match label?.to_ascii_lowercase().as_str() {
        "critical" | "p0" | "must-have" => Some(4),
        "high" | "p1" => Some(3),
        "medium" | "p2" | "should-have" => Some(2),
        "low" | "p3" | "nice-to-have" => Some(1),
        "none" | "p4" => Some(0),
        _ => None,
    }
}

fn dispatch_label_for(status: RequirementNativeStatus) -> &'static str {
    match status {
        RequirementNativeStatus::Drafted => "requirement-drafted",
        RequirementNativeStatus::Refined => "requirement-refining",
        RequirementNativeStatus::Approved => "requirement-approved",
        RequirementNativeStatus::Deprecated => "requirement-deprecated",
    }
}

fn description_for(status: RequirementNativeStatus) -> &'static str {
    match status {
        RequirementNativeStatus::Drafted => "Newly captured; awaiting refinement",
        RequirementNativeStatus::Refined => "Iteratively being clarified",
        RequirementNativeStatus::Approved => "Approved for downstream dispatch",
        RequirementNativeStatus::Deprecated => "Abandoned without implementation",
    }
}

fn log_migration_report(report: &MigrationReport) {
    if !report.any_changes() && report.errors.is_empty() && report.skipped_existing.is_empty() {
        return;
    }
    tracing::info!(
        migrated = report.migrated.len(),
        skipped_existing = report.skipped_existing.len(),
        errors = report.errors.len(),
        legacy_cleared = report.legacy_cleared,
        "legacy_json migration complete"
    );
    for (id, err) in &report.errors {
        tracing::warn!(id = %id, error = %err, "legacy_json migration failed for entry");
    }
}

fn map_store_err(err: StoreError) -> BackendError {
    match err {
        StoreError::NotFound(id) => BackendError::NotFound(id),
        StoreError::InvalidId(msg) => BackendError::InvalidRequest(msg),
        StoreError::Parse { path, source } => BackendError::Other(anyhow::anyhow!(
            "parse error at {}: {source}",
            path.display()
        )),
        StoreError::Json(e) => BackendError::Other(anyhow::anyhow!("json error: {e}")),
        StoreError::Io { path, source } => {
            BackendError::Other(anyhow::anyhow!("io error at {}: {source}", path.display()))
        }
    }
}

/// Create a new requirement on disk. Not part of the [`SubjectBackend`]
/// trait surface (which lacks a `create` method as of protocol v0.1.x);
/// exposed for in-process callers and for the contract tests.
///
/// The frontmatter id is overridden with the next sequential
/// `REQ-NNNN` derived from the cached index. Returns the persisted
/// [`Frontmatter`].
pub async fn create_requirement(
    backend: &RequirementsBackend,
    title: impl Into<String>,
    body: impl Into<String>,
) -> Result<Frontmatter, BackendError> {
    let index = backend.store.load_index().await.map_err(map_store_err)?;
    let prefix = backend.store.config().id_prefix.clone();
    let native = next_id(&prefix, &index);
    let id = full_id_from_native(&native);
    let mut frontmatter = Frontmatter::new(id, title);
    frontmatter.created_at = Utc::now();
    frontmatter.updated_at = frontmatter.created_at;
    let file = RequirementFile {
        frontmatter: frontmatter.clone(),
        body: body.into(),
    };
    backend.store.write(&file).await.map_err(map_store_err)?;
    let _ = backend.store.rebuild_index().await.map_err(map_store_err)?;
    Ok(frontmatter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn priority_label_to_u8() {
        assert_eq!(priority_to_u8(Some("critical")), Some(4));
        assert_eq!(priority_to_u8(Some("HIGH")), Some(3));
        assert_eq!(priority_to_u8(Some("medium")), Some(2));
        assert_eq!(priority_to_u8(Some("low")), Some(1));
        assert_eq!(priority_to_u8(Some("none")), Some(0));
        assert_eq!(priority_to_u8(Some("???")), None);
        assert_eq!(priority_to_u8(None), None);
    }

    #[test]
    fn passes_filter_excludes_archived_by_default() {
        let entry = CachedEntry {
            frontmatter: Frontmatter::new("requirement:REQ-1", "test"),
            relative_path: "archived/REQ-1.md".to_string(),
            archived: true,
        };
        let filter = SubjectFilter::default();
        assert!(!RequirementsBackend::passes_filter(&entry, &filter));
    }

    #[test]
    fn passes_filter_label_any() {
        let mut fm = Frontmatter::new("requirement:REQ-1", "test");
        fm.labels = vec!["auth".to_string(), "p1".to_string()];
        let entry = CachedEntry {
            frontmatter: fm,
            relative_path: "REQ-1.md".to_string(),
            archived: false,
        };
        let filter = SubjectFilter {
            labels_any: vec!["auth".to_string()],
            ..Default::default()
        };
        assert!(RequirementsBackend::passes_filter(&entry, &filter));
        let filter = SubjectFilter {
            labels_any: vec!["billing".to_string()],
            ..Default::default()
        };
        assert!(!RequirementsBackend::passes_filter(&entry, &filter));
    }

    #[test]
    fn apply_patch_records_label_add_and_remove() {
        let mut fm = Frontmatter::new("requirement:REQ-1", "test");
        fm.labels = vec!["old".to_string(), "keep".to_string()];
        let patch = SubjectPatch {
            labels_add: vec!["new".to_string()],
            labels_remove: vec!["old".to_string()],
            ..Default::default()
        };
        RequirementsBackend::apply_patch(&mut fm, &patch).unwrap();
        assert_eq!(fm.labels, vec!["keep".to_string(), "new".to_string()]);
    }

    #[test]
    fn apply_patch_assignee_set_and_clear() {
        let mut fm = Frontmatter::new("requirement:REQ-1", "test");
        let patch_set = SubjectPatch {
            assignee: Some(Some("alice".to_string())),
            ..Default::default()
        };
        RequirementsBackend::apply_patch(&mut fm, &patch_set).unwrap();
        assert_eq!(
            fm.custom_fields.get("assignee"),
            Some(&JsonValue::String("alice".to_string()))
        );

        let patch_clear = SubjectPatch {
            assignee: Some(None),
            ..Default::default()
        };
        RequirementsBackend::apply_patch(&mut fm, &patch_clear).unwrap();
        assert!(!fm.custom_fields.contains_key("assignee"));
    }

    #[tokio::test]
    async fn list_unions_legacy_json_when_configured() {
        use crate::config::RequirementsConfig;

        let dir = tempfile::TempDir::new().unwrap();
        let legacy_path = dir.path().join("core-state.json");
        std::fs::write(
            &legacy_path,
            r#"{
                "requirements": {
                    "REQ-9001": {
                        "id": "REQ-9001",
                        "title": "Legacy entry",
                        "description": "from json",
                        "priority": "must",
                        "status": "refined",
                        "tags": [],
                        "acceptance_criteria": [],
                        "comments": [],
                        "links": {},
                        "linked_task_ids": [],
                        "created_at": "2026-05-01T00:00:00Z",
                        "updated_at": "2026-05-01T00:00:00Z"
                    }
                }
            }"#,
        )
        .unwrap();
        let cfg = RequirementsConfig::new(dir.path().join("md"))
            .with_legacy_json_path(&legacy_path);
        let backend = RequirementsBackend::new(cfg).await.expect("backend");

        let list = backend.list(SubjectFilter::default()).await.expect("list");
        assert_eq!(list.subjects.len(), 1);
        assert_eq!(list.subjects[0].title, "Legacy entry");
        // Direct get returns the same entry.
        let got = backend
            .get(&SubjectId::new("requirement:REQ-9001".to_string()))
            .await
            .expect("get");
        assert_eq!(got.title, "Legacy entry");
    }

    #[tokio::test]
    async fn delete_removes_markdown_file() {
        use crate::config::RequirementsConfig;
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = RequirementsConfig::new(dir.path().join("md"));
        let backend = RequirementsBackend::new(cfg).await.expect("backend");
        let fm = create_requirement(&backend, "delete me", "body")
            .await
            .expect("create");
        let id = SubjectId::new(fm.id.clone());
        let path = backend.store.path_for(&native_id_from_full(&fm.id).unwrap());
        assert!(path.exists());

        let deleted = backend.delete(&id).await.expect("delete");
        assert!(deleted);
        assert!(!path.exists());

        // Second delete returns false (no-op).
        let again = backend.delete(&id).await.expect("delete-again");
        assert!(!again);
    }

    #[tokio::test]
    async fn delete_is_noop_for_legacy_only_id() {
        use crate::config::RequirementsConfig;
        let dir = tempfile::TempDir::new().unwrap();
        let legacy_path = dir.path().join("core-state.json");
        std::fs::write(
            &legacy_path,
            r#"{
                "requirements": {
                    "REQ-5": {
                        "id": "REQ-5",
                        "title": "Legacy only",
                        "description": "",
                        "priority": "should",
                        "status": "refined",
                        "tags": [],
                        "acceptance_criteria": [],
                        "comments": [],
                        "links": {},
                        "linked_task_ids": [],
                        "created_at": "2026-05-01T00:00:00Z",
                        "updated_at": "2026-05-01T00:00:00Z"
                    }
                }
            }"#,
        )
        .unwrap();
        let cfg = RequirementsConfig::new(dir.path().join("md"))
            .with_legacy_json_path(&legacy_path);
        let backend = RequirementsBackend::new(cfg).await.expect("backend");
        let deleted = backend
            .delete(&SubjectId::new("requirement:REQ-5".to_string()))
            .await
            .expect("delete");
        assert!(!deleted, "legacy-only ids are intentionally not hard-deleted");
        // Still visible via list (proves we didn't accidentally write).
        let list = backend.list(SubjectFilter::default()).await.expect("list");
        assert_eq!(list.subjects.len(), 1);
    }

    #[tokio::test]
    async fn migrate_on_start_converts_legacy() {
        use crate::config::RequirementsConfig;
        let dir = tempfile::TempDir::new().unwrap();
        let legacy_path = dir.path().join("core-state.json");
        std::fs::write(
            &legacy_path,
            r#"{
                "requirements": {
                    "REQ-100": {
                        "id": "REQ-100",
                        "title": "to migrate",
                        "description": "",
                        "priority": "must",
                        "status": "approved",
                        "tags": [],
                        "acceptance_criteria": [],
                        "comments": [],
                        "links": {},
                        "linked_task_ids": [],
                        "created_at": "2026-05-01T00:00:00Z",
                        "updated_at": "2026-05-01T00:00:00Z"
                    }
                }
            }"#,
        )
        .unwrap();
        let root = dir.path().join("md");
        let cfg = RequirementsConfig::new(&root)
            .with_legacy_json_path(&legacy_path)
            .with_migrate_legacy_on_start(true);
        let _backend = RequirementsBackend::new(cfg).await.expect("backend");
        let migrated = root.join("REQ-100.md");
        assert!(migrated.exists(), "migration should have written the file");
        let raw = std::fs::read_to_string(&legacy_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.get("requirements"), Some(&serde_json::json!({})));
    }

    #[test]
    fn apply_patch_custom_null_clears() {
        let mut fm = Frontmatter::new("requirement:REQ-1", "test");
        fm.custom_fields
            .insert("origin".to_string(), JsonValue::String("x".to_string()));
        let mut custom = BTreeMap::new();
        custom.insert("origin".to_string(), JsonValue::Null);
        let patch = SubjectPatch {
            custom,
            ..Default::default()
        };
        RequirementsBackend::apply_patch(&mut fm, &patch).unwrap();
        assert!(!fm.custom_fields.contains_key("origin"));
    }
}
