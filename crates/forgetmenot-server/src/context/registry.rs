//! The live contexts.
//!
//! Each context has its own lock, so two events for one context are serialized
//! while events for different contexts are not. The map's own lock is held only
//! to find or insert a context's handle, never while a context is being worked
//! on, and the store lock is never taken inside a context lock.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};

use super::{ContextKey, ContextState, initial_active};
use crate::store::settings::Settings;

/// What a subagent's context starts with, which the store's settings decide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inheritance {
    /// The scopes its session had active when it was spawned.
    FromParent,
    /// Only the scopes every context has: global, its machine and its session.
    ImplicitOnly,
}

impl Inheritance {
    /// What the store's settings ask for.
    pub fn of(settings: &Settings) -> Self {
        match settings.subagents_inherit_scopes {
            true => Inheritance::FromParent,
            false => Inheritance::ImplicitOnly,
        }
    }
}

/// What went wrong loading or writing a snapshot of the registry.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("state file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("state file {path} is not a context snapshot: {source}")]
    Format {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// One context in a snapshot.
///
/// The key is spelled out as three fields rather than used as a map key,
/// because a JSON object key can only be a string and a context key is not one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextRecord {
    #[serde(flatten)]
    pub key: ContextKey,
    #[serde(flatten)]
    pub state: ContextState,
}

/// The whole registry, as written to the state file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub contexts: Vec<ContextRecord>,
}

/// Every context the server knows about.
pub struct ContextRegistry {
    contexts: Mutex<HashMap<ContextKey, Arc<Mutex<ContextState>>>>,
    /// Raised whenever a context is created or changed, so the snapshot task
    /// knows there is something to write.
    changed: Notify,
    /// How long the snapshot task waits for further changes; zero writes on
    /// every change.
    debounce: Duration,
    /// Contexts not seen for this long are dropped when the registry is loaded
    /// or snapshotted; unset keeps them forever.
    retention: Option<Duration>,
}

impl ContextRegistry {
    /// An empty registry.
    pub fn new(snapshot_debounce_ms: u64, context_retention_days: Option<u64>) -> Self {
        Self {
            contexts: Mutex::new(HashMap::new()),
            changed: Notify::new(),
            debounce: Duration::from_millis(snapshot_debounce_ms),
            retention: context_retention_days
                .map(|days| Duration::from_secs(days.saturating_mul(24 * 60 * 60))),
        }
    }

    /// The registry recorded at `path`, or an empty one when there is no file
    /// there yet. Contexts older than the retention window are dropped.
    pub async fn load_from(
        path: &Path,
        snapshot_debounce_ms: u64,
        context_retention_days: Option<u64>,
        now: DateTime<Utc>,
    ) -> Result<Self, SnapshotError> {
        let registry = Self::new(snapshot_debounce_ms, context_retention_days);
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(registry),
            Err(source) => {
                return Err(SnapshotError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let snapshot: RegistrySnapshot =
            serde_json::from_slice(&bytes).map_err(|source| SnapshotError::Format {
                path: path.to_path_buf(),
                source,
            })?;
        let mut contexts = registry.contexts.lock().await;
        for record in snapshot.contexts {
            if registry.is_expired(&record.state, now) {
                continue;
            }
            contexts.insert(record.key, Arc::new(Mutex::new(record.state)));
        }
        drop(contexts);
        Ok(registry)
    }

    /// Run `work` on one context's state, creating the context if this is the
    /// first event for it.
    ///
    /// A subagent's context starts from a copy of its session's active scopes,
    /// taken under the session's own lock so that a scope activated at the same
    /// moment is either fully in or fully out, unless `inheritance` says the
    /// store wants subagents to start from nothing. It always takes its
    /// parent's session directory, whatever `inheritance` says about scopes.
    ///
    /// The scopes are copied and the activations behind them are not: a
    /// forgetting count is per context, so an inherited scope is counted from
    /// the child's own first event.
    pub async fn with_context<R>(
        &self,
        key: &ContextKey,
        now: DateTime<Utc>,
        inheritance: Inheritance,
        work: impl FnOnce(&mut ContextState) -> R,
    ) -> R {
        let handle = self.handle(key, now, inheritance).await;
        let result = {
            let mut state = handle.lock().await;
            work(&mut state)
        };
        self.changed.notify_one();
        result
    }

    /// The handle of one context, created if absent.
    pub async fn handle(
        &self,
        key: &ContextKey,
        now: DateTime<Utc>,
        inheritance: Inheritance,
    ) -> Arc<Mutex<ContextState>> {
        if let Some(existing) = self.contexts.lock().await.get(key).cloned() {
            return existing;
        }
        let (active, parent, session_directory) = if key.is_subagent() {
            let parent_key = key.session_context();
            let parent = self.handle_main(&parent_key, now).await;
            // The map lock is not held here, so the parent's own events are not
            // blocked by a child being created. The scopes and the session
            // directory are read in the one pass under the parent's lock.
            let (active, session_directory) = {
                let parent = parent.lock().await;
                let active = match inheritance {
                    Inheritance::FromParent => parent.active.clone(),
                    // The parent is still the parent: what changes is only what
                    // the child starts with.
                    Inheritance::ImplicitOnly => initial_active(&key.machine, &key.session_id),
                };
                // The session directory is a fact about the session the
                // subagent runs in, not a scope, so it is inherited either way.
                (active, parent.session_directory.clone())
            };
            (active, Some(parent_key), session_directory)
        } else {
            (initial_active(&key.machine, &key.session_id), None, None)
        };
        let mut contexts = self.contexts.lock().await;
        let handle = contexts.entry(key.clone()).or_insert_with(|| {
            let mut state = ContextState::fresh(active, parent, now);
            state.session_directory = session_directory;
            Arc::new(Mutex::new(state))
        });
        self.changed.notify_one();
        handle.clone()
    }

    /// The handle of a session's main context, created if absent. Split out of
    /// [`ContextRegistry::handle`] so that creating a child is not recursive.
    async fn handle_main(&self, key: &ContextKey, now: DateTime<Utc>) -> Arc<Mutex<ContextState>> {
        let mut contexts = self.contexts.lock().await;
        contexts
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Mutex::new(ContextState::fresh(
                    initial_active(&key.machine, &key.session_id),
                    None,
                    now,
                )))
            })
            .clone()
    }

    /// Every context and its state, for the snapshot and for the contexts page.
    pub async fn snapshot(&self, now: DateTime<Utc>) -> RegistrySnapshot {
        let handles: Vec<(ContextKey, Arc<Mutex<ContextState>>)> = self
            .contexts
            .lock()
            .await
            .iter()
            .map(|(key, handle)| (key.clone(), handle.clone()))
            .collect();
        let mut contexts = Vec::with_capacity(handles.len());
        for (key, handle) in handles {
            let state = handle.lock().await.clone();
            if self.is_expired(&state, now) {
                continue;
            }
            contexts.push(ContextRecord { key, state });
        }
        // Sorted so that two snapshots of the same registry are the same bytes,
        // whatever order the hash map happened to be walked in.
        contexts.sort_by(|left, right| left.key.cmp(&right.key));
        RegistrySnapshot { contexts }
    }

    /// Write the registry to `path`, replacing it atomically: a snapshot is
    /// either the previous state or the new one, never a truncated file.
    pub async fn snapshot_to(&self, path: &Path, now: DateTime<Utc>) -> Result<(), SnapshotError> {
        let snapshot = self.snapshot(now).await;
        let bytes =
            serde_json::to_vec_pretty(&snapshot).map_err(|source| SnapshotError::Format {
                path: path.to_path_buf(),
                source,
            })?;
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || write_atomically(&path, &bytes))
            .await
            .map_err(|error| SnapshotError::Io {
                path: PathBuf::new(),
                source: std::io::Error::other(error),
            })?
    }

    /// Wait for the next change, then for the debounce window. Returns `false`
    /// when the registry will not change again because `cancel` fired.
    pub async fn wait_for_change(&self, cancel: &tokio_util::sync::CancellationToken) -> bool {
        tokio::select! {
            () = self.changed.notified() => {}
            () = cancel.cancelled() => return false,
        }
        if self.debounce.is_zero() {
            return true;
        }
        tokio::select! {
            () = tokio::time::sleep(self.debounce) => true,
            () = cancel.cancelled() => false,
        }
    }

    fn is_expired(&self, state: &ContextState, now: DateTime<Utc>) -> bool {
        let Some(retention) = self.retention else {
            return false;
        };
        let Ok(age) = (now - state.last_seen).to_std() else {
            // A last_seen in the future is not an age; keep the context.
            return false;
        };
        age > retention
    }
}

/// Replace `path` with `bytes` through a temporary file in the same directory,
/// so that the rename is atomic on the same filesystem.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), SnapshotError> {
    let io = |source: std::io::Error| SnapshotError::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).map_err(io)?;
    std::fs::rename(&temporary, path).map_err(io)
}
