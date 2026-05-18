//! Contract tests for the requirements `SubjectBackend` implementation.
//!
//! Tests are filesystem-driven — each test sets up a `tempfile::TempDir`
//! as the requirements root, exercises one operation, and asserts the
//! on-disk + in-memory shape.

use std::collections::BTreeMap;
use std::time::SystemTime;

use animus_subject_protocol::{
    Subject, SubjectBackend, SubjectFilter, SubjectId, SubjectPatch, SubjectStatus,
};
use animus_subject_requirements::backend::{create_requirement, RequirementsBackend};
use animus_subject_requirements::config::RequirementsConfig;
use animus_subject_requirements::frontmatter::RequirementFile;
use animus_subject_requirements::status_map::{self, RequirementNativeStatus};
use serde_json::Value as JsonValue;
use tempfile::TempDir;

const FIXTURE: &str = include_str!("fixtures/REQ-0001.md");

async fn make_backend() -> (TempDir, RequirementsBackend) {
    let tmp = TempDir::new().expect("tempdir");
    let config = RequirementsConfig::new(tmp.path()).with_index_ttl_secs(1);
    let backend = RequirementsBackend::new(config).await.expect("backend");
    (tmp, backend)
}

async fn install_fixture(tmp: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = tmp.path().join(name);
    tokio::fs::write(&path, body).await.expect("write fixture");
    path
}

#[tokio::test]
async fn creates_requirement_assigns_sequential_id() {
    let (_tmp, backend) = make_backend().await;
    let first = create_requirement(&backend, "First", "First body")
        .await
        .expect("create first");
    let second = create_requirement(&backend, "Second", "Second body")
        .await
        .expect("create second");
    assert_eq!(first.id, "requirement:REQ-0001");
    assert_eq!(second.id, "requirement:REQ-0002");
}

#[tokio::test]
async fn parses_acceptance_criteria_as_list() {
    let (tmp, backend) = make_backend().await;
    install_fixture(&tmp, "REQ-0001.md", FIXTURE).await;
    let subject = backend
        .get(&SubjectId::new("requirement:REQ-0001"))
        .await
        .expect("get");
    let ac = subject
        .custom
        .get("acceptance_criteria")
        .expect("acceptance_criteria present");
    let arr = ac.as_array().expect("array");
    assert_eq!(arr.len(), 3);
    assert!(arr[0]
        .as_str()
        .unwrap()
        .starts_with("Google + GitHub OAuth"));
}

#[tokio::test]
async fn parses_linked_tasks_as_list() {
    let (tmp, backend) = make_backend().await;
    install_fixture(&tmp, "REQ-0001.md", FIXTURE).await;
    let subject = backend
        .get(&SubjectId::new("requirement:REQ-0001"))
        .await
        .expect("get");
    let arr = subject
        .custom
        .get("linked_tasks")
        .and_then(|v| v.as_array())
        .expect("linked_tasks array");
    let names: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["task:TASK-0042", "task:TASK-0099"]);
    // The Subject.children mirror should also be populated.
    let child_ids: Vec<String> = subject.children.iter().map(|c| c.0.clone()).collect();
    assert_eq!(child_ids, vec!["task:TASK-0042", "task:TASK-0099"]);
}

#[tokio::test]
async fn parses_linked_workflows_as_list() {
    let (tmp, backend) = make_backend().await;
    install_fixture(&tmp, "REQ-0001.md", FIXTURE).await;
    let subject = backend
        .get(&SubjectId::new("requirement:REQ-0001"))
        .await
        .expect("get");
    let arr = subject
        .custom
        .get("linked_workflows")
        .and_then(|v| v.as_array())
        .expect("linked_workflows array");
    let names: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names, vec!["delivery"]);
}

#[tokio::test]
async fn update_records_refined_at_when_moved_to_refined() {
    let (_tmp, backend) = make_backend().await;
    let created = create_requirement(&backend, "Drafted", "body")
        .await
        .expect("create");
    assert_eq!(created.status, RequirementNativeStatus::Drafted);
    assert!(created.refined_at.is_none());

    let subject_id = SubjectId::new(created.id.clone());
    let patch = SubjectPatch {
        // Drafted -> Refined: should stamp `refined_at`.
        status: Some(status_map::native_to_subject(
            RequirementNativeStatus::Refined,
        )),
        ..Default::default()
    };
    let updated_subject: Subject = backend.update(&subject_id, patch).await.expect("update");
    assert_eq!(updated_subject.status, SubjectStatus::InProgress);
    assert!(updated_subject
        .custom
        .get("refined_at")
        .and_then(|v| v.as_str())
        .is_some());
}

#[tokio::test]
async fn update_records_refined_by_when_provided() {
    let (_tmp, backend) = make_backend().await;
    let created = create_requirement(&backend, "Drafted", "body")
        .await
        .expect("create");
    let subject_id = SubjectId::new(created.id.clone());
    let mut custom = BTreeMap::new();
    custom.insert(
        "refined_by".to_string(),
        JsonValue::String("alice@example.com".to_string()),
    );
    let patch = SubjectPatch {
        status: Some(status_map::native_to_subject(
            RequirementNativeStatus::Refined,
        )),
        custom,
        ..Default::default()
    };
    let updated: Subject = backend.update(&subject_id, patch).await.expect("update");
    assert_eq!(
        updated.custom.get("refined_by").and_then(|v| v.as_str()),
        Some("alice@example.com")
    );
}

#[test]
fn status_map_drafted_to_ready() {
    assert_eq!(
        status_map::native_to_subject(RequirementNativeStatus::Drafted),
        SubjectStatus::Ready
    );
}

#[test]
fn status_map_refined_to_in_progress() {
    assert_eq!(
        status_map::native_to_subject(RequirementNativeStatus::Refined),
        SubjectStatus::InProgress
    );
}

#[test]
fn status_map_approved_to_done() {
    assert_eq!(
        status_map::native_to_subject(RequirementNativeStatus::Approved),
        SubjectStatus::Done
    );
}

#[test]
fn status_map_deprecated_to_cancelled() {
    assert_eq!(
        status_map::native_to_subject(RequirementNativeStatus::Deprecated),
        SubjectStatus::Cancelled
    );
}

#[tokio::test]
async fn index_json_rebuilds_when_file_mutated_externally() {
    let (tmp, backend) = make_backend().await;
    install_fixture(&tmp, "REQ-0001.md", FIXTURE).await;
    // Warm the index so _index.json exists.
    let first_page = backend
        .list(SubjectFilter::default())
        .await
        .expect("initial list");
    assert_eq!(first_page.subjects.len(), 1);
    assert_eq!(
        first_page.subjects[0].title,
        "Users must be able to log in with OAuth"
    );

    // Externally drop a new requirement and shift its mtime forward
    // so the staleness check trips even on filesystems with low mtime
    // resolution.
    let new_path = install_fixture(
        &tmp,
        "REQ-0002.md",
        &FIXTURE
            .replace("REQ-0001", "REQ-0002")
            .replace("OAuth", "MFA"),
    )
    .await;
    let future = SystemTime::now() + std::time::Duration::from_secs(120);
    filetime::set_file_mtime(&new_path, filetime::FileTime::from_system_time(future))
        .expect("touch mtime");

    let page = backend
        .list(SubjectFilter::default())
        .await
        .expect("list after external edit");
    assert_eq!(
        page.subjects.len(),
        2,
        "external edit must invalidate cache"
    );
    let ids: Vec<String> = page.subjects.iter().map(|s| s.id.0.clone()).collect();
    assert!(ids.iter().any(|id| id == "requirement:REQ-0001"));
    assert!(ids.iter().any(|id| id == "requirement:REQ-0002"));
}

#[tokio::test]
async fn archived_requirements_excluded_from_default_list() {
    let (tmp, backend) = make_backend().await;
    install_fixture(&tmp, "REQ-0001.md", FIXTURE).await;
    let archived_dir = tmp.path().join("archived");
    tokio::fs::create_dir_all(&archived_dir)
        .await
        .expect("mkdir archived");
    let archived = FIXTURE
        .replace("REQ-0001", "REQ-0003")
        .replace("OAuth", "Legacy SSO");
    tokio::fs::write(archived_dir.join("REQ-0003.md"), archived)
        .await
        .expect("write archived");

    let page = backend.list(SubjectFilter::default()).await.expect("list");
    let ids: Vec<String> = page.subjects.iter().map(|s| s.id.0.clone()).collect();
    assert_eq!(ids, vec!["requirement:REQ-0001"]);
    // But get() still works for archived items (they're parseable, just
    // not surfaced for dispatch).
    let archived_subject = backend
        .get(&SubjectId::new("requirement:REQ-0003"))
        .await
        .expect("archived still gettable");
    assert_eq!(
        archived_subject.title,
        "Users must be able to log in with Legacy SSO"
    );
}

#[tokio::test]
async fn list_round_trips_a_freshly_created_file() {
    let (_tmp, backend) = make_backend().await;
    let created = create_requirement(&backend, "Smoke", "smoke body")
        .await
        .expect("create");

    let page = backend.list(SubjectFilter::default()).await.expect("list");
    assert_eq!(page.subjects.len(), 1);
    let subject = &page.subjects[0];
    assert_eq!(subject.id.0, created.id);
    assert_eq!(subject.title, "Smoke");
    assert_eq!(subject.status, SubjectStatus::Ready);
    assert_eq!(subject.native_status.as_deref(), Some("drafted"));
}

#[tokio::test]
async fn frontmatter_to_file_roundtrip_preserves_body() {
    let parsed = RequirementFile::parse(FIXTURE).expect("parse fixture");
    let rendered = parsed.to_string().expect("render");
    let again = RequirementFile::parse(&rendered).expect("reparse");
    assert_eq!(parsed.frontmatter, again.frontmatter);
    assert!(again.body.contains("Open questions"));
}
