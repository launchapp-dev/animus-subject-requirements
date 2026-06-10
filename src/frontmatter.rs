//! YAML frontmatter parsing and serialization for requirement records.
//!
//! Each requirement file looks like:
//!
//! ```markdown
//! ---
//! id: requirement:REQ-0001
//! kind: requirement
//! title: "..."
//! status: refined
//! priority: high
//! labels: [auth, p1]
//! parent_id: null
//! linked_tasks: [task:TASK-0042, task:TASK-0099]
//! linked_workflows: ["delivery"]
//! acceptance_criteria:
//!   - "Google + GitHub OAuth providers supported"
//! created_at: 2026-05-18T12:00:00Z
//! updated_at: 2026-05-18T13:30:00Z
//! refined_at: 2026-05-18T13:30:00Z
//! refined_by: alice@example.com
//! custom_fields:
//!   origin: stakeholder-interview-2026-q2
//! ---
//!
//! # REQ-0001: ...
//!
//! ## Context
//! ...
//! ```
//!
//! [`Frontmatter`] is the in-memory representation of the YAML block.
//! [`RequirementFile`] pairs the frontmatter with the markdown body.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::status_map::RequirementNativeStatus;

/// Parsed YAML frontmatter from a requirement file.
///
/// All optional fields use `#[serde(default, skip_serializing_if = ...)]`
/// so requirements written by hand can omit anything they don't need —
/// the round-trip preserves the omission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frontmatter {
    /// Fully-qualified subject id, e.g. `requirement:REQ-0001`. Always
    /// emitted on write; required on read so we can dispatch to the right
    /// backend.
    pub id: String,

    /// Subject kind. Always `"requirement"` for this backend; we keep it
    /// explicit so the frontmatter is self-describing.
    #[serde(default = "default_kind")]
    pub kind: String,

    /// Short human title.
    pub title: String,

    /// Native requirement lifecycle state.
    #[serde(default)]
    pub status: RequirementNativeStatus,

    /// Optional priority. Free-form ("high", "p1", "must-have") for now —
    /// workflow YAML maps this to a numeric priority when needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,

    /// Labels / tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,

    /// Parent requirement / epic id, if this requirement decomposes another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Task ids this requirement unlocks once approved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_tasks: Vec<String>,

    /// Workflow ids that consume this requirement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_workflows: Vec<String>,

    /// Acceptance criteria — bulleted list of conditions the implementation
    /// must satisfy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,

    /// When the requirement was first captured.
    pub created_at: DateTime<Utc>,

    /// Last time any field was modified.
    pub updated_at: DateTime<Utc>,

    /// Last time `status` moved into [`RequirementNativeStatus::Refined`].
    /// `None` for requirements that have never been refined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refined_at: Option<DateTime<Utc>>,

    /// Free-form identifier (email, agent name, ...) of the actor that
    /// last refined this requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refined_by: Option<String>,

    /// Free-form custom fields. Workflow YAML can read these via
    /// templating (e.g. `{{subject.custom.origin}}`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_fields: BTreeMap<String, JsonValue>,
}

fn default_kind() -> String {
    "requirement".to_string()
}

impl Frontmatter {
    /// Build a fresh requirement frontmatter with default-now timestamps.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            kind: default_kind(),
            title: title.into(),
            status: RequirementNativeStatus::Drafted,
            priority: None,
            labels: Vec::new(),
            parent_id: None,
            linked_tasks: Vec::new(),
            linked_workflows: Vec::new(),
            acceptance_criteria: Vec::new(),
            created_at: now,
            updated_at: now,
            refined_at: None,
            refined_by: None,
            custom_fields: BTreeMap::new(),
        }
    }
}

/// A parsed requirement file = its YAML frontmatter + the markdown body
/// after the closing `---`.
#[derive(Debug, Clone, PartialEq)]
pub struct RequirementFile {
    /// Parsed YAML frontmatter.
    pub frontmatter: Frontmatter,
    /// Markdown body — everything after the closing `---` line.
    pub body: String,
}

impl RequirementFile {
    /// Parse a full file content (frontmatter + body) into a
    /// [`RequirementFile`].
    pub fn parse(raw: &str) -> Result<Self, FrontmatterError> {
        let stripped = raw.strip_prefix('\u{FEFF}').unwrap_or(raw); // BOM tolerance
        let stripped = stripped.trim_start_matches('\n');

        let body_start = stripped.strip_prefix("---\n").ok_or_else(|| {
            FrontmatterError::Malformed(
                "expected `---\\n` at start of file (YAML frontmatter)".to_string(),
            )
        })?;

        // Find the closing `---` marker on its own line.
        let mut end_idx: Option<usize> = None;
        let mut offset = 0usize;
        for line in body_start.split_inclusive('\n') {
            let trimmed = line.trim_end_matches('\n');
            if trimmed == "---" {
                end_idx = Some(offset);
                break;
            }
            offset += line.len();
        }

        let end = end_idx.ok_or_else(|| {
            FrontmatterError::Malformed("missing closing `---` for YAML frontmatter".to_string())
        })?;

        let yaml_str = &body_start[..end];
        let after = &body_start[end..];
        let body = after.strip_prefix("---").unwrap_or(after);
        let body = body.strip_prefix('\n').unwrap_or(body);

        let frontmatter: Frontmatter =
            serde_yaml::from_str(yaml_str).map_err(|e| FrontmatterError::Yaml(e.to_string()))?;

        Ok(Self {
            frontmatter,
            body: body.to_string(),
        })
    }

    /// Render the file back to disk-friendly text.
    pub fn to_string(&self) -> Result<String, FrontmatterError> {
        let yaml = serde_yaml::to_string(&self.frontmatter)
            .map_err(|e| FrontmatterError::Yaml(e.to_string()))?;
        let yaml_trimmed = yaml.trim_end_matches('\n');
        Ok(format!("---\n{yaml_trimmed}\n---\n{}", self.body))
    }
}

/// Errors that can occur while parsing or serializing frontmatter.
#[derive(Debug, thiserror::Error)]
pub enum FrontmatterError {
    /// Structural problem with the markdown framing (missing fences, no
    /// closing marker, etc.).
    #[error("malformed frontmatter: {0}")]
    Malformed(String),
    /// The YAML between the fences didn't parse.
    #[error("yaml error: {0}")]
    Yaml(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SAMPLE: &str = r#"---
id: requirement:REQ-0001
kind: requirement
title: "Users must be able to log in with OAuth"
status: refined
priority: high
labels:
  - auth
  - p1
linked_tasks:
  - task:TASK-0042
  - task:TASK-0099
linked_workflows:
  - delivery
acceptance_criteria:
  - "Google + GitHub OAuth providers supported"
  - "Session tokens stored server-side, not in localStorage"
created_at: 2026-05-18T12:00:00Z
updated_at: 2026-05-18T13:30:00Z
refined_at: 2026-05-18T13:30:00Z
refined_by: alice@example.com
custom_fields:
  origin: stakeholder-interview-2026-q2
---

# REQ-0001: Users must be able to log in with OAuth

Body content here.
"#;

    #[test]
    fn parses_sample_file() {
        let parsed = RequirementFile::parse(SAMPLE).expect("sample parses");
        assert_eq!(parsed.frontmatter.id, "requirement:REQ-0001");
        assert_eq!(parsed.frontmatter.kind, "requirement");
        assert_eq!(parsed.frontmatter.status, RequirementNativeStatus::Refined);
        assert_eq!(parsed.frontmatter.priority.as_deref(), Some("high"));
        assert_eq!(parsed.frontmatter.labels, vec!["auth", "p1"]);
        assert_eq!(parsed.frontmatter.acceptance_criteria.len(), 2);
        assert_eq!(parsed.frontmatter.linked_tasks.len(), 2);
        assert_eq!(parsed.frontmatter.linked_workflows, vec!["delivery"]);
        assert_eq!(
            parsed.frontmatter.custom_fields.get("origin"),
            Some(&json!("stakeholder-interview-2026-q2"))
        );
        assert!(parsed.body.contains("# REQ-0001"));
    }

    #[test]
    fn round_trip_preserves_fields() {
        let parsed = RequirementFile::parse(SAMPLE).expect("parse");
        let rendered = parsed.to_string().expect("render");
        let reparsed = RequirementFile::parse(&rendered).expect("reparse");
        assert_eq!(parsed.frontmatter, reparsed.frontmatter);
    }

    #[test]
    fn missing_close_fence_errors() {
        let bad = "---\nid: requirement:REQ-1\ntitle: missing close\n";
        let err = RequirementFile::parse(bad).expect_err("must error");
        assert!(matches!(err, FrontmatterError::Malformed(_)));
    }

    #[test]
    fn missing_open_fence_errors() {
        let bad = "no fence at all\n";
        let err = RequirementFile::parse(bad).expect_err("must error");
        assert!(matches!(err, FrontmatterError::Malformed(_)));
    }

    #[test]
    fn bare_minimum_frontmatter_parses() {
        let minimal = "---\n\
id: requirement:REQ-9\n\
kind: requirement\n\
title: minimal\n\
status: drafted\n\
created_at: 2026-05-18T00:00:00Z\n\
updated_at: 2026-05-18T00:00:00Z\n\
---\n";
        let parsed = RequirementFile::parse(minimal).expect("minimal parses");
        assert_eq!(parsed.frontmatter.title, "minimal");
        assert!(parsed.frontmatter.labels.is_empty());
        assert!(parsed.frontmatter.linked_tasks.is_empty());
    }
}
