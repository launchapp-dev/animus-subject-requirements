//! `notify`-based file watcher that emits [`SubjectChangedEvent`]s.
//!
//! The watcher is started by [`RequirementsBackend::watch`](crate::backend::RequirementsBackend::watch).
//! It returns a [`futures::Stream`] of [`SubjectChangedEvent`]s that the
//! plugin runtime forwards as `subject/changed` notifications.

use std::path::PathBuf;
use std::sync::Arc;

use animus_subject_protocol::{ChangeKind, EventStream, SubjectChangedEvent};
use futures::stream::StreamExt;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::backend::cached_entry_to_subject;
use crate::store::{native_id_from_full, RequirementsStore};

/// Spawn a watcher on `<root>/` that emits a [`SubjectChangedEvent`] for
/// every `.md` mutation it sees. Returns `None` if the underlying
/// `notify::recommended_watcher` fails to initialize (e.g. on platforms
/// where the requested backend is unavailable).
pub fn spawn(store: Arc<RequirementsStore>) -> Option<EventStream> {
    let (tx_raw, mut rx_raw) = mpsc::channel::<notify::Result<Event>>(64);
    let mut watcher = match notify::recommended_watcher(move |res| {
        // notify's callback is sync, but we want the event on our async
        // channel — `blocking_send` is fine here because the channel is
        // bounded and the watcher thread has no other contention.
        let _ = tx_raw.blocking_send(res);
    }) {
        Ok(w) => w,
        Err(err) => {
            tracing::warn!(
                target: "animus_subject_requirements",
                ?err,
                "notify watcher failed to initialize; watch will be disabled"
            );
            return None;
        }
    };
    if let Err(err) = watcher.watch(&store.config().root, RecursiveMode::Recursive) {
        tracing::warn!(
            target: "animus_subject_requirements",
            ?err,
            root = %store.config().root.display(),
            "notify watcher failed to start; watch will be disabled"
        );
        return None;
    }

    let (tx_out, rx_out) = mpsc::channel::<SubjectChangedEvent>(64);
    // We have to keep the watcher alive for the lifetime of the stream;
    // moving it into the spawned task is the simplest way to tie its
    // lifetime to the consumer's.
    let store_clone = store.clone();
    tokio::spawn(async move {
        let _watcher = watcher;
        while let Some(res) = rx_raw.recv().await {
            let event = match res {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(
                        target: "animus_subject_requirements",
                        ?err,
                        "notify event error; continuing"
                    );
                    continue;
                }
            };
            for path in event.paths {
                if !is_requirement_file(&path) {
                    continue;
                }
                let change_kind = classify_event(&event.kind);
                if let Some(emit) = build_event(&store_clone, &path, change_kind).await {
                    if tx_out.send(emit).await.is_err() {
                        return; // consumer dropped the stream
                    }
                }
            }
        }
    });

    let stream = ReceiverStream::new(rx_out).map(|e| e);
    Some(Box::pin(stream))
}

fn is_requirement_file(path: &std::path::Path) -> bool {
    matches!(path.extension().and_then(|s| s.to_str()), Some("md"))
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| !n.starts_with('.') && n != "_index.json")
            .unwrap_or(false)
}

fn classify_event(kind: &EventKind) -> ChangeKind {
    match kind {
        EventKind::Create(_) => ChangeKind::Created,
        EventKind::Modify(_) => ChangeKind::Updated,
        EventKind::Remove(_) => ChangeKind::Deleted,
        _ => ChangeKind::Updated,
    }
}

async fn build_event(
    store: &Arc<RequirementsStore>,
    path: &PathBuf,
    change_kind: ChangeKind,
) -> Option<SubjectChangedEvent> {
    let stem = path.file_stem().and_then(|s| s.to_str())?.to_string();
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(s) => s,
        Err(_) => return None,
    };
    let parsed = crate::frontmatter::RequirementFile::parse(&raw).ok()?;
    let native = native_id_from_full(&parsed.frontmatter.id).unwrap_or(stem);
    let relative = path
        .strip_prefix(&store.config().root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let archived = relative.starts_with("archived");
    let entry = crate::store::CachedEntry {
        frontmatter: parsed.frontmatter,
        relative_path: relative,
        archived,
    };
    let subject = cached_entry_to_subject(&native, &entry);
    Some(SubjectChangedEvent {
        id: subject.id.clone(),
        change_kind,
        subject,
        previous_native_status: None,
        previous_dispatch_label: None,
    })
}
