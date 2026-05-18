//! Mapping between requirement-native lifecycle states and the normalized
//! [`SubjectStatus`] enum the Animus daemon dispatches on.
//!
//! Requirements have a lifecycle distinct from tasks: they're drafted,
//! refined (often iteratively), approved, then either consumed by a
//! workflow or deprecated. This module collapses that lifecycle into the
//! five-bucket [`SubjectStatus`] taxonomy without losing the native value
//! (callers can still read [`Subject::native_status`](animus_subject_protocol::Subject::native_status)
//! to branch on `"drafted"`, `"refined"`, etc.).

use animus_subject_protocol::SubjectStatus;
use serde::{Deserialize, Serialize};

/// Native lifecycle states a requirement can be in.
///
/// The four-state model is intentionally simpler than Animus's in-tree
/// `RequirementStatus` enum (Draft / Refined / Planned / InProgress / Done
/// / PoReview / EmReview / NeedsRework / Approved / Implemented /
/// Deprecated). This plugin treats post-approval lifecycle (Planned,
/// InProgress, Implemented, Done) as the responsibility of the *task*
/// subject backend the requirement spawns — once a requirement is
/// `approved` it's ready to dispatch downstream and the workflow takes
/// over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequirementNativeStatus {
    /// Newly captured. Needs further refinement before it can be acted on.
    #[default]
    Drafted,
    /// Iteratively clarified. May go back and forth before approval.
    Refined,
    /// Approved for downstream dispatch. Ready to spawn tasks / kick off
    /// implementing workflows.
    Approved,
    /// Abandoned without implementation.
    Deprecated,
}

impl RequirementNativeStatus {
    /// The kebab-case wire string this status serializes to.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drafted => "drafted",
            Self::Refined => "refined",
            Self::Approved => "approved",
            Self::Deprecated => "deprecated",
        }
    }

    /// Parse a wire string into a status, accepting both kebab-case and
    /// PascalCase forms. Returns `None` for unknown values.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "drafted" | "Drafted" | "draft" | "Draft" => Some(Self::Drafted),
            "refined" | "Refined" => Some(Self::Refined),
            "approved" | "Approved" => Some(Self::Approved),
            "deprecated" | "Deprecated" => Some(Self::Deprecated),
            _ => None,
        }
    }

    /// Every variant, in lifecycle order. Used by `schema()` to surface the
    /// native status set.
    pub const ALL: &'static [RequirementNativeStatus] = &[
        Self::Drafted,
        Self::Refined,
        Self::Approved,
        Self::Deprecated,
    ];
}

/// Translate a requirement-native status to the normalized
/// [`SubjectStatus`] bucket.
///
/// Mapping rationale:
/// - `drafted` -> [`SubjectStatus::Ready`]: requirements eligible for
///   refinement workflows pull on this bucket.
/// - `refined` -> [`SubjectStatus::InProgress`]: ongoing iterative
///   clarification.
/// - `approved` -> [`SubjectStatus::Done`]: from the requirement's
///   perspective the work of capturing/clarifying is complete. The
///   downstream tasks the requirement spawns have their own lifecycle.
/// - `deprecated` -> [`SubjectStatus::Cancelled`]: abandoned.
pub fn native_to_subject(status: RequirementNativeStatus) -> SubjectStatus {
    match status {
        RequirementNativeStatus::Drafted => SubjectStatus::Ready,
        RequirementNativeStatus::Refined => SubjectStatus::InProgress,
        RequirementNativeStatus::Approved => SubjectStatus::Done,
        RequirementNativeStatus::Deprecated => SubjectStatus::Cancelled,
    }
}

/// Translate a normalized [`SubjectStatus`] back to the most natural
/// requirement-native status. Used when callers apply a status patch
/// expressed in [`SubjectStatus`] terms.
///
/// Note that `Blocked` has no natural inverse in the requirement
/// lifecycle, so we map it to `Refined` (the iterative state) on the
/// theory that a blocked requirement is one whose clarification is stuck.
pub fn subject_to_native(status: SubjectStatus) -> RequirementNativeStatus {
    match status {
        SubjectStatus::Ready => RequirementNativeStatus::Drafted,
        SubjectStatus::InProgress | SubjectStatus::Blocked => RequirementNativeStatus::Refined,
        SubjectStatus::Done => RequirementNativeStatus::Approved,
        SubjectStatus::Cancelled => RequirementNativeStatus::Deprecated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_kebab_and_pascal() {
        assert_eq!(
            RequirementNativeStatus::parse("drafted"),
            Some(RequirementNativeStatus::Drafted)
        );
        assert_eq!(
            RequirementNativeStatus::parse("Refined"),
            Some(RequirementNativeStatus::Refined)
        );
        assert_eq!(
            RequirementNativeStatus::parse("draft"),
            Some(RequirementNativeStatus::Drafted)
        );
        assert!(RequirementNativeStatus::parse("not-a-status").is_none());
    }

    #[test]
    fn as_str_round_trips_via_parse() {
        for status in RequirementNativeStatus::ALL {
            let s = status.as_str();
            assert_eq!(RequirementNativeStatus::parse(s), Some(*status));
        }
    }

    #[test]
    fn native_to_subject_maps_lifecycle() {
        assert_eq!(
            native_to_subject(RequirementNativeStatus::Drafted),
            SubjectStatus::Ready
        );
        assert_eq!(
            native_to_subject(RequirementNativeStatus::Refined),
            SubjectStatus::InProgress
        );
        assert_eq!(
            native_to_subject(RequirementNativeStatus::Approved),
            SubjectStatus::Done
        );
        assert_eq!(
            native_to_subject(RequirementNativeStatus::Deprecated),
            SubjectStatus::Cancelled
        );
    }

    #[test]
    fn subject_to_native_inverse() {
        assert_eq!(
            subject_to_native(SubjectStatus::Ready),
            RequirementNativeStatus::Drafted
        );
        assert_eq!(
            subject_to_native(SubjectStatus::InProgress),
            RequirementNativeStatus::Refined
        );
        assert_eq!(
            subject_to_native(SubjectStatus::Done),
            RequirementNativeStatus::Approved
        );
        assert_eq!(
            subject_to_native(SubjectStatus::Cancelled),
            RequirementNativeStatus::Deprecated
        );
        // Blocked has no native inverse; mapping to Refined is documented.
        assert_eq!(
            subject_to_native(SubjectStatus::Blocked),
            RequirementNativeStatus::Refined
        );
    }
}
