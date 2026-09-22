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

use super::{ContextKey, ContextState, initial_active, migrate};
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

/// What a context is made from, at the moment it is first seen.
///
/// It is read only when the context is created; everything that finds the
/// context already there carries on with the state it has.
#[derive(Clone, Debug)]
pub struct Creation {
    /// What a subagent's context starts with, which the store's settings decide.
    pub inheritance: Inheritance,
    /// The context a subagent inherits from and is recorded under. `None` is
    /// the session's own context, which is where a subagent whose spawner is
    /// not known sits.
    pub parent: Option<ContextKey>,
}

impl Creation {
    /// A context created by a caller that names no spawner: the MCP tools and
    /// the API reach a context by its key alone, so a subagent they create
    /// inherits from the session it runs in.
    pub fn in_session(settings: &Settings) -> Self {
        Self {
            inheritance: Inheritance::of(settings),
            parent: None,
        }
    }

    /// A context created by the event that spawned it, which says whether the
    /// spawner was another subagent or the session itself.
    pub fn spawned_by(settings: &Settings, parent: Option<ContextKey>) -> Self {
        Self {
            inheritance: Inheritance::of(settings),
            parent,
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
    /// The shape the records are in, which is what says whether a build reading
    /// this file has to repair them: see [`migrate`]. A file written before the
    /// field existed carries none, which is version 0.
    #[serde(default)]
    pub version: u32,
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
    /// there yet. A file from an earlier shape is brought up to the current one
    /// by [`migrate::to_current`] before it is read, and contexts older than the
    /// retention window are dropped.
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
        let mut snapshot: RegistrySnapshot =
            serde_json::from_slice(&bytes).map_err(|source| SnapshotError::Format {
                path: path.to_path_buf(),
                source,
            })?;
        migrate::to_current(&mut snapshot);
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
    /// A subagent's context starts from a copy of the active scopes of the
    /// context `creation` names as its parent, taken under that context's own
    /// lock so that a scope activated at the same moment is either fully in or
    /// fully out, unless the inheritance says the store wants subagents to
    /// start from nothing. It always takes its parent's session directory,
    /// whatever the inheritance says about scopes.
    ///
    /// The scopes are copied and the activations behind them are not: a
    /// forgetting count is per context, so an inherited scope is counted from
    /// the child's own first event.
    pub async fn with_context<R>(
        &self,
        key: &ContextKey,
        now: DateTime<Utc>,
        creation: Creation,
        work: impl FnOnce(&mut ContextState) -> R,
    ) -> R {
        let handle = self.handle(key, now, creation).await;
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
        creation: Creation,
    ) -> Arc<Mutex<ContextState>> {
        if let Some(existing) = self.contexts.lock().await.get(key).cloned() {
            return existing;
        }
        let (active, parent, session_directory) = if key.is_subagent() {
            let parent_key = creation.parent.unwrap_or_else(|| key.session_context());
            let parent = self.handle_parent(&parent_key, now).await;
            // The map lock is not held here, so the parent's own events are not
            // blocked by a child being created. The scopes and the session
            // directory are read in the one pass under the parent's lock.
            let (active, session_directory) = {
                let parent = parent.lock().await;
                let active = match creation.inheritance {
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

    /// The handle of the context a child is created from, created itself if
    /// absent. Split out of [`ContextRegistry::handle`] so that creating a
    /// child is not recursive.
    ///
    /// A parent that is not there yet is a spawner whose own context was
    /// dropped or never recorded, so it is created from the implicit scopes and
    /// recorded under its session, which is as far as a key alone says.
    async fn handle_parent(
        &self,
        key: &ContextKey,
        now: DateTime<Utc>,
    ) -> Arc<Mutex<ContextState>> {
        let mut contexts = self.contexts.lock().await;
        contexts
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Mutex::new(ContextState::fresh(
                    initial_active(&key.machine, &key.session_id),
                    key.is_subagent().then(|| key.session_context()),
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
        RegistrySnapshot {
            version: migrate::CURRENT_VERSION,
            contexts,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ScopeId;

    /// The directory `claude` was started in, which is what these tests follow
    /// from the session that reported it to the places that must still have it.
    const STARTED_IN: &str = "/start/project";

    /// A context created by a caller that names no spawner, under a store that
    /// asks for a subagent to start with its parent's scopes.
    fn under_a_session() -> Creation {
        Creation {
            inheritance: Inheritance::FromParent,
            parent: None,
        }
    }

    /// The same, for a subagent `parent` spawned.
    fn spawned_by(parent: &ContextKey) -> Creation {
        Creation {
            inheritance: Inheritance::FromParent,
            parent: Some(parent.clone()),
        }
    }

    /// Detects a subagent given a session directory of its own, or none at all:
    /// a subagent runs in its parent's session, so the directory `claude` was
    /// started in is the same for both, whatever the subagent's shell is doing
    /// and whatever the store says a subagent starts with in the way of scopes.
    #[tokio::test]
    async fn a_subagent_starts_with_the_session_directory_of_the_session_it_runs_in() {
        let registry = ContextRegistry::new(0, None);
        let now = Utc::now();
        let session = ContextKey::main("alpha", "session-1");
        registry
            .with_context(&session, now, under_a_session(), |state| {
                state.session_directory = Some(STARTED_IN.to_string());
                state.active.insert(ScopeId::new("widgets"));
            })
            .await;

        let child = ContextKey::subagent("alpha", "session-1", "agent-7");
        let (directory, active) = registry
            .with_context(
                &child,
                now,
                Creation {
                    inheritance: Inheritance::ImplicitOnly,
                    parent: None,
                },
                |state| (state.session_directory.clone(), state.active.clone()),
            )
            .await;

        assert_eq!(
            directory.as_deref(),
            Some(STARTED_IN),
            "the subagent runs in the session that began in {STARTED_IN}"
        );
        assert!(
            !active.contains(&ScopeId::new("widgets")),
            "this store starts a subagent from the implicit scopes, so the directory cannot \
             have come with the scopes, got {active:?}"
        );
    }

    /// Detects a session directory that is not written to the snapshot: the
    /// server would come back up having forgotten where every running session
    /// began, and their directory triggers would stop firing.
    #[tokio::test]
    async fn a_snapshot_keeps_the_directory_each_session_was_started_in() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let path = directory.path().join("contexts.json");
        let now = Utc::now();
        let key = ContextKey::main("alpha", "session-1");
        let registry = ContextRegistry::new(0, None);
        registry
            .with_context(&key, now, under_a_session(), |state| {
                state.session_directory = Some(STARTED_IN.to_string());
            })
            .await;

        registry
            .snapshot_to(&path, now)
            .await
            .expect("the snapshot is written");
        let loaded = ContextRegistry::load_from(&path, 0, None, now)
            .await
            .expect("the snapshot is read back");

        let started_in = loaded
            .with_context(&key, now, under_a_session(), |state| {
                state.session_directory.clone()
            })
            .await;
        assert_eq!(
            started_in.as_deref(),
            Some(STARTED_IN),
            "a session picked up after a restart began where it began"
        );
    }

    /// Detects a subagent created from its session rather than from the agent
    /// that spawned it: a subagent that turns a scope on for the work it is
    /// about to delegate would hand its own subagent a context without that
    /// scope, and every agent below the first would work from the session's
    /// scopes however deep it sits.
    ///
    /// The session and its child are each in a scope the other is not, so the
    /// grandchild's set says which of the two it was created from: both scopes
    /// means the child, and `paint` alone means the session. The session's
    /// second child is created with no spawner named and must still be the
    /// session's, so a registry that gave every subagent the last parent it saw
    /// is caught as well.
    #[tokio::test]
    async fn a_subagent_inherits_from_the_agent_that_spawned_it_rather_than_from_its_session() {
        let registry = ContextRegistry::new(0, None);
        let now = Utc::now();
        let paint = ScopeId::new("paint");
        let lathe = ScopeId::new("lathe");
        let session = ContextKey::main("alpha", "session-1");
        registry
            .with_context(&session, now, under_a_session(), |state| {
                state.active.insert(paint.clone());
            })
            .await;
        let child = ContextKey::subagent("alpha", "session-1", "agent-1");
        registry
            .with_context(&child, now, under_a_session(), |state| {
                state.active.insert(lathe.clone());
            })
            .await;

        let grandchild = ContextKey::subagent("alpha", "session-1", "agent-1-1");
        let (inherited, recorded) = registry
            .with_context(&grandchild, now, spawned_by(&child), |state| {
                (state.active.clone(), state.parent.clone())
            })
            .await;
        let sibling = ContextKey::subagent("alpha", "session-1", "agent-2");
        let by_the_session = registry
            .with_context(&sibling, now, under_a_session(), |state| {
                state.active.clone()
            })
            .await;

        assert!(
            inherited.contains(&paint) && inherited.contains(&lathe),
            "the grandchild starts with the scopes the agent that spawned it was working in, \
             got {inherited:?}"
        );
        assert_eq!(
            recorded,
            Some(child),
            "the context a subagent inherited from is the one recorded as its parent"
        );
        assert!(
            by_the_session.contains(&paint) && !by_the_session.contains(&lathe),
            "a subagent the session spawned starts with the session's scopes and not with \
             another subagent's, got {by_the_session:?}"
        );
    }

    /// Detects a context created by a caller that names no spawner being left
    /// without a parent, or given one it never had: the MCP tools and the API
    /// reach a context by its key alone, and a key says no more than which
    /// session the subagent runs in.
    #[tokio::test]
    async fn a_subagent_first_seen_by_a_call_that_names_no_spawner_is_recorded_under_its_session() {
        let registry = ContextRegistry::new(0, None);
        let now = Utc::now();
        let key = ContextKey::subagent("alpha", "session-1", "agent-1");

        let parent = registry
            .with_context(
                &key,
                now,
                Creation::in_session(&Settings::default()),
                |state| state.parent.clone(),
            )
            .await;

        assert_eq!(
            parent,
            Some(ContextKey::main("alpha", "session-1")),
            "the session the subagent runs in is all its key says, so it is the parent"
        );
    }
}
