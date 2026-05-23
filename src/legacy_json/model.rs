//! Mirrored shape of the in-tree `RequirementItem` and its surrounding
//! types, kept structurally compatible with
//! `protocol::orchestrator::RequirementItem` in the `ao-cli` repo as of
//! v0.4.0.
//!
//! The plugin uses these as deserialization targets only — we never
//! mutate the legacy file. New writes always land in the Markdown
//! frontmatter format owned by [`crate::frontmatter`].
//!
//! ## Naming
//!
//! Field names are lifted verbatim from the in-tree definitions so the
//! existing JSON on disk round-trips without a custom serde adapter. If
//! the in-tree type evolves we'll need to either keep these in sync or
//! widen them to `serde_json::Value` for the affected fields.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Top-level shape of `~/.animus/<repo-scope>/core-state.json` as far as
/// the requirements plugin is concerned. Only the `requirements` field is
/// read — every other field is preserved as `serde_json::Value` so the
/// untyped portion of the file survives a future write.
///
/// We never write this struct back; the plugin only reads it. The
/// migrator deletes the legacy entries through a separate path that
/// edits the raw JSON value to avoid dropping unrelated state.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyCoreState {
    #[serde(default)]
    pub requirements: HashMap<String, LegacyRequirementJson>,
}

/// Mirror of `protocol::orchestrator::RequirementItem` from the in-tree
/// `ao-cli` workspace. Matches field-for-field including `serde(default)`
/// behavior so existing JSON deserializes without modification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyRequirementJson {
    pub id: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub legacy_id: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(rename = "type", default)]
    pub requirement_type: Option<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub priority: LegacyPriority,
    #[serde(default)]
    pub status: LegacyStatus,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub links: LegacyLinks,
    #[serde(default)]
    pub comments: Vec<LegacyComment>,
    #[serde(default)]
    pub relative_path: Option<String>,
    #[serde(default)]
    pub linked_task_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Mirror of `protocol::orchestrator::RequirementLinks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LegacyLinks {
    #[serde(default)]
    pub tasks: Vec<String>,
    #[serde(default)]
    pub workflows: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub mockups: Vec<String>,
    #[serde(default)]
    pub flows: Vec<String>,
    #[serde(default)]
    pub related_requirements: Vec<String>,
}

/// Mirror of `protocol::orchestrator::RequirementComment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyComment {
    pub author: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub phase: Option<String>,
}

/// Mirror of `protocol::common::RequirementPriority`. The MoSCoW four
/// (`must` / `should` / `could` / `wont`) — kept as a string in
/// frontmatter via [`legacy_priority_label`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LegacyPriority {
    Must,
    #[default]
    Should,
    Could,
    Wont,
}

/// Mirror of `protocol::orchestrator::RequirementStatus`. Eleven-state
/// lifecycle — wider than the four [`crate::status_map::RequirementNativeStatus`]
/// values the standalone plugin uses natively, so the converter folds
/// the extra states down via [`legacy_status_to_native`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LegacyStatus {
    #[default]
    Draft,
    Refined,
    Planned,
    #[serde(alias = "in_progress")]
    InProgress,
    Done,
    PoReview,
    EmReview,
    NeedsRework,
    Approved,
    Implemented,
    Deprecated,
}

/// Human label preserved as `priority:` on the new frontmatter.
pub fn legacy_priority_label(priority: LegacyPriority) -> &'static str {
    match priority {
        LegacyPriority::Must => "must",
        LegacyPriority::Should => "should",
        LegacyPriority::Could => "could",
        LegacyPriority::Wont => "wont",
    }
}

/// Collapse the in-tree status enum onto the four-state standalone
/// model. The full string is preserved under `custom_fields.legacy_status`
/// so downstream consumers can still see the original.
pub fn legacy_status_to_native(status: LegacyStatus) -> crate::status_map::RequirementNativeStatus {
    use crate::status_map::RequirementNativeStatus as N;
    match status {
        LegacyStatus::Draft => N::Drafted,
        LegacyStatus::Refined => N::Refined,
        LegacyStatus::Planned | LegacyStatus::Approved => N::Approved,
        LegacyStatus::InProgress | LegacyStatus::PoReview | LegacyStatus::EmReview => N::Refined,
        LegacyStatus::Done | LegacyStatus::Implemented => N::Approved,
        LegacyStatus::NeedsRework => N::Refined,
        LegacyStatus::Deprecated => N::Deprecated,
    }
}

/// String form of [`LegacyStatus`] for round-tripping into `custom_fields`.
pub fn legacy_status_str(status: LegacyStatus) -> &'static str {
    match status {
        LegacyStatus::Draft => "draft",
        LegacyStatus::Refined => "refined",
        LegacyStatus::Planned => "planned",
        LegacyStatus::InProgress => "in-progress",
        LegacyStatus::Done => "done",
        LegacyStatus::PoReview => "po-review",
        LegacyStatus::EmReview => "em-review",
        LegacyStatus::NeedsRework => "needs-rework",
        LegacyStatus::Approved => "approved",
        LegacyStatus::Implemented => "implemented",
        LegacyStatus::Deprecated => "deprecated",
    }
}

/// Convert a [`LegacyRequirementJson`] into a [`crate::frontmatter::RequirementFile`]
/// preserving every field. Rich fields not modeled by the standalone
/// frontmatter (acceptance criteria, comments, linked tasks, links,
/// legacy_id, category, requirement_type, source, original status) are
/// stashed under `custom_fields` so a round-trip through the Markdown
/// format doesn't lose data.
pub fn legacy_to_requirement_file(
    legacy: &LegacyRequirementJson,
) -> crate::frontmatter::RequirementFile {
    use crate::frontmatter::{Frontmatter, RequirementFile};
    use std::collections::BTreeMap;

    let mut custom_fields: BTreeMap<String, JsonValue> = BTreeMap::new();
    if !legacy.acceptance_criteria.is_empty() {
        custom_fields.insert(
            "acceptance_criteria".to_string(),
            JsonValue::Array(
                legacy
                    .acceptance_criteria
                    .iter()
                    .map(|s| JsonValue::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if !legacy.comments.is_empty() {
        custom_fields.insert(
            "comments".to_string(),
            serde_json::to_value(&legacy.comments).unwrap_or(JsonValue::Null),
        );
    }
    if !legacy.linked_task_ids.is_empty() {
        custom_fields.insert(
            "linked_task_ids".to_string(),
            JsonValue::Array(
                legacy
                    .linked_task_ids
                    .iter()
                    .map(|s| JsonValue::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(legacy_id) = &legacy.legacy_id {
        custom_fields.insert(
            "legacy_id".to_string(),
            JsonValue::String(legacy_id.clone()),
        );
    }
    if let Some(category) = &legacy.category {
        custom_fields.insert(
            "category".to_string(),
            JsonValue::String(category.clone()),
        );
    }
    if let Some(req_type) = &legacy.requirement_type {
        custom_fields.insert(
            "requirement_type".to_string(),
            JsonValue::String(req_type.clone()),
        );
    }
    if !legacy.source.is_empty() {
        custom_fields.insert(
            "source".to_string(),
            JsonValue::String(legacy.source.clone()),
        );
    }
    if let Some(relative_path) = &legacy.relative_path {
        custom_fields.insert(
            "legacy_relative_path".to_string(),
            JsonValue::String(relative_path.clone()),
        );
    }
    custom_fields.insert(
        "legacy_status".to_string(),
        JsonValue::String(legacy_status_str(legacy.status).to_string()),
    );

    // Links: only emit the buckets that have entries so round-tripping
    // doesn't pollute frontmatter with empty arrays.
    let links_json = serde_json::to_value(&legacy.links).unwrap_or(JsonValue::Null);
    if let JsonValue::Object(map) = &links_json {
        let nonempty: serde_json::Map<String, JsonValue> = map
            .iter()
            .filter(|(_, v)| match v {
                JsonValue::Array(a) => !a.is_empty(),
                _ => false,
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !nonempty.is_empty() {
            custom_fields.insert("legacy_links".to_string(), JsonValue::Object(nonempty));
        }
    }

    // Mark the origin so callers (CLI, web UI, audit log) can tell which
    // store the entry came from without re-checking disk.
    custom_fields.insert(
        "origin_store".to_string(),
        JsonValue::String("legacy_json".to_string()),
    );

    let native_id = legacy.id.clone();
    let full_id = crate::store::full_id_from_native(&native_id);

    let frontmatter = Frontmatter {
        id: full_id,
        kind: "requirement".to_string(),
        title: legacy.title.clone(),
        status: legacy_status_to_native(legacy.status),
        priority: Some(legacy_priority_label(legacy.priority).to_string()),
        labels: legacy.tags.clone(),
        parent_id: None,
        linked_tasks: legacy.linked_task_ids.clone(),
        linked_workflows: legacy.links.workflows.clone(),
        acceptance_criteria: legacy.acceptance_criteria.clone(),
        created_at: legacy.created_at,
        updated_at: legacy.updated_at,
        refined_at: None,
        refined_by: None,
        custom_fields,
    };

    let body = match (&legacy.body, legacy.description.is_empty()) {
        (Some(b), _) if !b.is_empty() => b.clone(),
        (_, false) => format!("\n{}\n", legacy.description),
        _ => String::new(),
    };

    RequirementFile { frontmatter, body }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LegacyRequirementJson {
        LegacyRequirementJson {
            id: "REQ-0001".to_string(),
            title: "Sample".to_string(),
            description: "desc".to_string(),
            body: None,
            legacy_id: Some("OLD-1".to_string()),
            category: Some("auth".to_string()),
            requirement_type: Some("functional".to_string()),
            acceptance_criteria: vec!["ac-1".to_string()],
            priority: LegacyPriority::Must,
            status: LegacyStatus::Refined,
            source: "test".to_string(),
            tags: vec!["a".to_string()],
            links: LegacyLinks {
                tasks: vec!["T-1".to_string()],
                ..Default::default()
            },
            comments: vec![LegacyComment {
                author: "alice".to_string(),
                content: "looks good".to_string(),
                timestamp: chrono::Utc::now(),
                phase: None,
            }],
            relative_path: Some("generated/REQ-0001.json".to_string()),
            linked_task_ids: vec!["TASK-1".to_string()],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn legacy_to_frontmatter_preserves_rich_fields() {
        let legacy = sample();
        let file = legacy_to_requirement_file(&legacy);
        assert_eq!(file.frontmatter.id, "requirement:REQ-0001");
        assert_eq!(file.frontmatter.title, "Sample");
        assert_eq!(file.frontmatter.priority.as_deref(), Some("must"));
        assert_eq!(file.frontmatter.labels, vec!["a"]);
        assert_eq!(file.frontmatter.linked_tasks, vec!["TASK-1"]);
        assert_eq!(file.frontmatter.acceptance_criteria, vec!["ac-1"]);
        let cf = &file.frontmatter.custom_fields;
        assert_eq!(
            cf.get("legacy_id").and_then(JsonValue::as_str),
            Some("OLD-1")
        );
        assert_eq!(
            cf.get("category").and_then(JsonValue::as_str),
            Some("auth")
        );
        assert_eq!(
            cf.get("requirement_type").and_then(JsonValue::as_str),
            Some("functional")
        );
        assert!(cf.contains_key("comments"));
        assert!(cf.contains_key("legacy_links"));
        assert_eq!(
            cf.get("origin_store").and_then(JsonValue::as_str),
            Some("legacy_json")
        );
    }

    #[test]
    fn status_collapse_keeps_original_under_custom() {
        let mut legacy = sample();
        legacy.status = LegacyStatus::PoReview;
        let file = legacy_to_requirement_file(&legacy);
        assert_eq!(
            file.frontmatter
                .custom_fields
                .get("legacy_status")
                .and_then(JsonValue::as_str),
            Some("po-review")
        );
    }

    #[test]
    fn body_falls_back_to_description() {
        let legacy = sample();
        let file = legacy_to_requirement_file(&legacy);
        assert!(file.body.contains("desc"));
    }

    #[test]
    fn deserialize_minimal_json() {
        let raw = r#"{
            "id": "REQ-0042",
            "title": "Minimal",
            "description": "",
            "created_at": "2026-05-01T00:00:00Z",
            "updated_at": "2026-05-01T00:00:00Z"
        }"#;
        let parsed: LegacyRequirementJson = serde_json::from_str(raw).expect("parses");
        assert_eq!(parsed.id, "REQ-0042");
        assert_eq!(parsed.priority, LegacyPriority::Should);
        assert_eq!(parsed.status, LegacyStatus::Draft);
    }
}
