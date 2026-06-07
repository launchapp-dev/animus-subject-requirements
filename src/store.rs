//! Read/write logic for requirement records on disk + `_index.json` caching.
//!
//! The store is the only place that touches the filesystem. The
//! [`RequirementsBackend`](crate::backend::RequirementsBackend) delegates
//! all CRUD ops here and stays focused on protocol shape.
//!
//! # Index cache
//!
//! Listing requirements means walking the entire `<root>/` directory
//! every call. To keep that cheap we maintain `_index.json` — a flat
//! cache of every requirement's frontmatter. The cache is refreshed
//! when:
//!
//! 1. The file is older than `index_ttl_secs` (config-driven).
//! 2. Any requirement file's `mtime` is newer than the index file's
//!    `mtime` (catches external edits like a colleague pushing a
//!    requirement via git).
//!
//! On miss the store walks the directory, parses each `*.md` file, and
//! rewrites `_index.json` atomically. Concurrent rebuilds are guarded by
//! an in-process [`tokio::sync::Mutex`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::RwLock;

use crate::config::RequirementsConfig;
use crate::frontmatter::{Frontmatter, RequirementFile};
use crate::id_gen;
use crate::status_map::RequirementNativeStatus;

const ARCHIVED_DIR: &str = "archived";
const INDEX_FILENAME: &str = "_index.json";

/// Errors produced by the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Requested requirement file is not present on disk.
    #[error("requirement not found: {0}")]
    NotFound(String),

    /// Filesystem I/O failed.
    #[error("io error at {path}: {source}")]
    Io {
        /// Path the operation was attempted on.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Frontmatter parsing failed for an existing file.
    #[error("parse error at {path}: {source}")]
    Parse {
        /// Path the parse failed on.
        path: PathBuf,
        /// Underlying parse error.
        #[source]
        source: crate::frontmatter::FrontmatterError,
    },

    /// JSON ser/de failed (e.g. malformed `_index.json`).
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Caller passed a malformed subject id.
    #[error("invalid id: {0}")]
    InvalidId(String),
}

/// Cached index of requirement frontmatter, persisted as `_index.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedIndex {
    /// Generation timestamp.
    pub generated_at: chrono::DateTime<Utc>,
    /// Native id (`REQ-NNNN`) -> entry.
    pub entries: BTreeMap<String, CachedEntry>,
}

/// One row in the cached index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedEntry {
    /// Frontmatter as read from disk (sans body).
    pub frontmatter: Frontmatter,
    /// Disk path relative to the requirements root.
    pub relative_path: String,
    /// Whether the file lives under `archived/`.
    #[serde(default)]
    pub archived: bool,
}

/// The store. Holds config + an in-process lock for index rebuilds.
#[derive(Debug)]
pub struct RequirementsStore {
    config: RequirementsConfig,
    rebuild_lock: Mutex<()>,
    last_loaded: RwLock<Option<SystemTime>>,
}

impl RequirementsStore {
    /// Construct a new store. Ensures the root directory exists.
    pub async fn new(config: RequirementsConfig) -> Result<Self, StoreError> {
        tokio::fs::create_dir_all(&config.root)
            .await
            .map_err(|e| StoreError::Io {
                path: config.root.clone(),
                source: e,
            })?;
        Ok(Self {
            config,
            rebuild_lock: Mutex::new(()),
            last_loaded: RwLock::new(None),
        })
    }

    /// Borrow the configuration the store was constructed with.
    pub fn config(&self) -> &RequirementsConfig {
        &self.config
    }

    /// Filesystem path for a given native id, preferring the top-level
    /// directory but falling back to `archived/` if the file moved.
    pub fn path_for(&self, native_id: &str) -> PathBuf {
        let primary = self.config.root.join(format!("{native_id}.md"));
        if primary.exists() {
            return primary;
        }
        let archived = self
            .config
            .root
            .join(ARCHIVED_DIR)
            .join(format!("{native_id}.md"));
        if archived.exists() {
            return archived;
        }
        primary
    }

    /// Read one requirement by native id (`REQ-NNNN`, no `requirement:`
    /// prefix). Returns [`StoreError::NotFound`] if the file is missing.
    pub async fn read(&self, native_id: &str) -> Result<RequirementFile, StoreError> {
        let path = self.path_for(native_id);
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound(native_id.to_string()))
            }
            Err(e) => {
                return Err(StoreError::Io {
                    path: path.clone(),
                    source: e,
                });
            }
        };
        RequirementFile::parse(&raw).map_err(|source| StoreError::Parse { path, source })
    }

    /// Permanently remove a requirement file by native id. Returns
    /// [`StoreError::NotFound`] if the file does not exist (in either the
    /// root or `archived/`). Callers should follow up with
    /// [`Self::rebuild_index`] to invalidate the cache.
    ///
    /// Rejects native ids that are not pure filename fragments — anything
    /// containing path separators (`/`, `\`), `..`, or an absolute root —
    /// to prevent crafted ids escaping the requirements directory.
    pub async fn delete(&self, native_id: &str) -> Result<PathBuf, StoreError> {
        if native_id.is_empty()
            || native_id.contains('/')
            || native_id.contains('\\')
            || native_id.contains("..")
            || Path::new(native_id).is_absolute()
            || std::path::Path::new(native_id).components().count() != 1
        {
            return Err(StoreError::InvalidId(native_id.to_string()));
        }
        let path = self.path_for(native_id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StoreError::NotFound(native_id.to_string()))
            }
            Err(e) => Err(StoreError::Io {
                path: path.clone(),
                source: e,
            }),
        }
    }

    /// Atomically write a requirement file. New files go under the root
    /// directory; archived requirements stay in `archived/`.
    pub async fn write(&self, file: &RequirementFile) -> Result<PathBuf, StoreError> {
        let native_id = native_id_from_full(&file.frontmatter.id)
            .ok_or_else(|| StoreError::InvalidId(file.frontmatter.id.clone()))?;
        // Preserve the archived/ location if the file was previously archived.
        let existing = self.path_for(&native_id);
        let target = if existing.exists() {
            existing
        } else {
            self.config.root.join(format!("{native_id}.md"))
        };
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StoreError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
        }
        let body = file.to_string().map_err(|source| StoreError::Parse {
            path: target.clone(),
            source,
        })?;
        atomic_write(&target, &body).await?;
        Ok(target)
    }

    /// Walk the requirements directory and return every entry. Honors
    /// the index cache when fresh; otherwise rebuilds.
    pub async fn load_index(&self) -> Result<CachedIndex, StoreError> {
        let cached = self.read_index_if_fresh().await?;
        if let Some(idx) = cached {
            return Ok(idx);
        }
        // Take the rebuild lock so concurrent callers don't all scan.
        let _g = self.rebuild_lock.lock().await;
        // Re-check after acquiring the lock (another waiter may have
        // rebuilt while we waited).
        if let Some(idx) = self.read_index_if_fresh().await? {
            return Ok(idx);
        }
        let rebuilt = self.rebuild_index().await?;
        Ok(rebuilt)
    }

    /// Force a fresh rebuild of `_index.json`.
    pub async fn rebuild_index(&self) -> Result<CachedIndex, StoreError> {
        let mut entries: BTreeMap<String, CachedEntry> = BTreeMap::new();
        scan_dir(&self.config.root, &self.config.root, false, &mut entries).await?;
        let archived_dir = self.config.root.join(ARCHIVED_DIR);
        if archived_dir.exists() {
            scan_dir(&archived_dir, &self.config.root, true, &mut entries).await?;
        }
        let idx = CachedIndex {
            generated_at: Utc::now(),
            entries,
        };
        self.write_index(&idx).await?;
        let mut last = self.last_loaded.write().await;
        *last = Some(SystemTime::now());
        Ok(idx)
    }

    async fn read_index_if_fresh(&self) -> Result<Option<CachedIndex>, StoreError> {
        let index_path = self.config.root.join(INDEX_FILENAME);
        let idx_meta = match tokio::fs::metadata(&index_path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(StoreError::Io {
                    path: index_path,
                    source: e,
                })
            }
        };
        let idx_mtime = idx_meta.modified().ok();
        let now = SystemTime::now();
        let ttl = Duration::from_secs(self.config.index_ttl_secs);
        if let Some(mtime) = idx_mtime {
            if let Ok(age) = now.duration_since(mtime) {
                if age > ttl {
                    return Ok(None);
                }
            }
        }
        // Check for any newer .md files (covers external edits).
        let stale = any_md_newer_than(&self.config.root, idx_mtime).await?;
        if stale {
            return Ok(None);
        }
        let raw = tokio::fs::read_to_string(&index_path)
            .await
            .map_err(|e| StoreError::Io {
                path: index_path.clone(),
                source: e,
            })?;
        let idx: CachedIndex = serde_json::from_str(&raw)?;
        Ok(Some(idx))
    }

    async fn write_index(&self, idx: &CachedIndex) -> Result<(), StoreError> {
        let path = self.config.root.join(INDEX_FILENAME);
        let body = serde_json::to_string_pretty(idx)?;
        atomic_write(&path, &body).await
    }

    /// Path to the `_index.json` file (mainly for tests).
    pub fn index_path(&self) -> PathBuf {
        self.config.root.join(INDEX_FILENAME)
    }
}

/// Strip the `requirement:` prefix from a full subject id and return the
/// native portion (e.g. `requirement:REQ-0001` -> `REQ-0001`).
pub fn native_id_from_full(full: &str) -> Option<String> {
    full.strip_prefix("requirement:").map(str::to_string)
}

/// Native id (`REQ-0001`) -> fully-qualified subject id.
pub fn full_id_from_native(native: &str) -> String {
    format!("requirement:{native}")
}

/// Generate the next sequential id. Uses [`id_gen::next_id`] but reads
/// existing ids from the cached index for efficiency.
pub fn next_id(prefix: &str, index: &CachedIndex) -> String {
    id_gen::next_id(prefix, index.entries.keys())
}

async fn scan_dir(
    dir: &Path,
    root: &Path,
    archived: bool,
    out: &mut BTreeMap<String, CachedEntry>,
) -> Result<(), StoreError> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(StoreError::Io {
                path: dir.to_path_buf(),
                source: e,
            })
        }
    };
    while let Some(entry) = rd.next_entry().await.map_err(|e| StoreError::Io {
        path: dir.to_path_buf(),
        source: e,
    })? {
        let path = entry.path();
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if file_name == ARCHIVED_DIR {
            continue;
        }
        if file_name == INDEX_FILENAME {
            continue;
        }
        let ft = entry.file_type().await.map_err(|e| StoreError::Io {
            path: path.clone(),
            source: e,
        })?;
        if ft.is_dir() {
            continue;
        }
        if !file_name.ends_with(".md") {
            continue;
        }
        let raw = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| StoreError::Io {
                path: path.clone(),
                source: e,
            })?;
        let parsed = RequirementFile::parse(&raw).map_err(|source| StoreError::Parse {
            path: path.clone(),
            source,
        })?;
        let native_id = native_id_from_full(&parsed.frontmatter.id)
            .ok_or_else(|| StoreError::InvalidId(parsed.frontmatter.id.clone()))?;
        let relative_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        out.insert(
            native_id,
            CachedEntry {
                frontmatter: parsed.frontmatter,
                relative_path,
                archived,
            },
        );
    }
    Ok(())
}

async fn any_md_newer_than(root: &Path, threshold: Option<SystemTime>) -> Result<bool, StoreError> {
    let threshold = match threshold {
        Some(t) => t,
        None => return Ok(true),
    };
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut rd = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(StoreError::Io {
                    path: dir,
                    source: e,
                })
            }
        };
        while let Some(entry) = rd.next_entry().await.map_err(|e| StoreError::Io {
            path: dir.clone(),
            source: e,
        })? {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let ft = entry.file_type().await.map_err(|e| StoreError::Io {
                path: path.clone(),
                source: e,
            })?;
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if name == INDEX_FILENAME {
                continue;
            }
            if !name.ends_with(".md") {
                continue;
            }
            let meta = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if let Ok(mtime) = meta.modified() {
                if mtime > threshold {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

async fn atomic_write(target: &Path, contents: &str) -> Result<(), StoreError> {
    let tmp = match target.file_name().and_then(|n| n.to_str()) {
        Some(name) => target.with_file_name(format!(".{name}.tmp")),
        None => target.with_extension("tmp"),
    };
    if let Some(parent) = tmp.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| StoreError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
    }
    tokio::fs::write(&tmp, contents)
        .await
        .map_err(|e| StoreError::Io {
            path: tmp.clone(),
            source: e,
        })?;
    tokio::fs::rename(&tmp, target)
        .await
        .map_err(|e| StoreError::Io {
            path: target.to_path_buf(),
            source: e,
        })?;
    Ok(())
}

/// Decide whether a requirement should record `refined_at`/`refined_by`
/// based on its old and new statuses.
///
/// We record the timestamp when the requirement *enters* the `Refined`
/// state from any other state. Re-refining (`Refined` -> `Refined`) is
/// also stamped so callers can track the most recent refinement.
pub fn should_stamp_refinement(
    previous: RequirementNativeStatus,
    next: RequirementNativeStatus,
) -> bool {
    next == RequirementNativeStatus::Refined
        && (previous != RequirementNativeStatus::Refined
            || previous == RequirementNativeStatus::Refined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_id_round_trip() {
        let full = full_id_from_native("REQ-0001");
        assert_eq!(full, "requirement:REQ-0001");
        assert_eq!(native_id_from_full(&full).as_deref(), Some("REQ-0001"));
        assert!(native_id_from_full("task:REQ-0001").is_none());
    }

    #[test]
    fn should_stamp_refinement_into_refined() {
        assert!(should_stamp_refinement(
            RequirementNativeStatus::Drafted,
            RequirementNativeStatus::Refined
        ));
        assert!(should_stamp_refinement(
            RequirementNativeStatus::Refined,
            RequirementNativeStatus::Refined
        ));
        assert!(!should_stamp_refinement(
            RequirementNativeStatus::Drafted,
            RequirementNativeStatus::Approved
        ));
        assert!(!should_stamp_refinement(
            RequirementNativeStatus::Drafted,
            RequirementNativeStatus::Drafted
        ));
    }
}
