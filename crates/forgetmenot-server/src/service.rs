//! The shared store handle: one git repository, one catalog snapshot, one
//! write lock.
//!
//! Readers take a snapshot of the catalog once and use it for a whole request,
//! so every decision inside one request is made against one store version.
//! Writers serialize on an async mutex and commit compare-and-swap against the
//! head they read, so a commit made by hand between a read and a write makes
//! the write fail rather than discarding the hand commit.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use git2::Oid;

use crate::store::catalog::{Catalog, LoadError};
use crate::store::git::{CommitOutcome, GitError, GitRepo};
use crate::store::{MemoryId, ScopeId};

/// The longest a commit message's title line may be, the width `git log
/// --oneline` shows without wrapping.
pub const MAX_MESSAGE_TITLE: usize = 72;

/// A failure of a store read.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Load(#[from] LoadError),
    /// The blocking task the git work runs in did not finish, which happens
    /// only if it panicked or the runtime is shutting down.
    #[error("the store task did not finish: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// A failure of a store write.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The commit message's title line is missing, multi-line or too long.
    #[error(
        "the commit message must be one non-empty line of at most {MAX_MESSAGE_TITLE} characters"
    )]
    BadMessage,
    /// The document changed since the caller read it. `current` is the version
    /// it has now, absent when the document no longer exists.
    #[error("{path} has changed since it was read")]
    VersionMismatch { path: String, current: Option<Oid> },
    /// The store moved under the write twice in a row.
    #[error("the store is being written concurrently; the write was not made")]
    Conflict,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Git(#[from] GitError),
}

impl From<LoadError> for WriteError {
    fn from(error: LoadError) -> Self {
        WriteError::Store(StoreError::Load(error))
    }
}

impl From<tokio::task::JoinError> for WriteError {
    fn from(error: tokio::task::JoinError) -> Self {
        WriteError::Store(StoreError::Task(error))
    }
}

/// What a successful write produced.
#[derive(Clone, Debug)]
pub struct WriteOutcome {
    pub commit_oid: Oid,
    /// The new version of each written file.
    pub blob_oids: BTreeMap<String, Oid>,
}

/// The store handle every request shares.
pub struct Store {
    /// `GitRepo` is `Send` but not `Sync`, so every use goes through this lock
    /// inside a blocking task.
    repository: Arc<Mutex<GitRepo>>,
    /// The current snapshot. Swapped under a short write lock when the head
    /// moves; readers of the previous snapshot are never blocked.
    catalog: RwLock<Arc<Catalog>>,
    /// Serializes read head, validate, commit: the compare-and-swap in
    /// `commit_files` needs the head it read to still be the head.
    write_lock: tokio::sync::Mutex<()>,
}

impl Store {
    /// Open the store at `path`, creating it if there is nothing there yet, and
    /// build the first catalog.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let path = path.to_path_buf();
        let (repository, catalog) = tokio::task::spawn_blocking(move || {
            let repository = GitRepo::open_or_init(&path)?;
            let catalog = Catalog::load(&repository)?;
            Ok::<_, StoreError>((repository, catalog))
        })
        .await??;
        Ok(Self {
            repository: Arc::new(Mutex::new(repository)),
            catalog: RwLock::new(Arc::new(catalog)),
            write_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// The catalog of the store's current head.
    ///
    /// Reading the head is cheap; the catalog is rebuilt only when the head has
    /// moved, which is how a commit made outside the server is picked up.
    pub async fn snapshot(&self) -> Result<Arc<Catalog>, StoreError> {
        let current = self.current_snapshot();
        let repository = self.repository.clone();
        let known_head = current.head;
        let rebuilt = tokio::task::spawn_blocking(move || {
            let repository = repository.lock().expect("the store lock is not poisoned");
            let head = repository.head_oid()?;
            if head == known_head {
                return Ok::<_, StoreError>(None);
            }
            Ok(Some(Catalog::load_at(&repository, head)?))
        })
        .await??;

        match rebuilt {
            None => Ok(current),
            Some(catalog) => {
                let catalog = Arc::new(catalog);
                *self
                    .catalog
                    .write()
                    .expect("the catalog lock is not poisoned") = catalog.clone();
                Ok(catalog)
            }
        }
    }

    /// The snapshot as it is, without asking git anything.
    pub fn current_snapshot(&self) -> Arc<Catalog> {
        self.catalog
            .read()
            .expect("the catalog lock is not poisoned")
            .clone()
    }

    /// Write a set of documents as one commit.
    ///
    /// `files` pairs a repository path with its new bytes, or `None` to delete
    /// it. `expected_versions` is the version of each path the caller read,
    /// `None` meaning the caller expects the path not to exist; a path whose
    /// current version differs is reported as [`WriteError::VersionMismatch`]
    /// and nothing is written.
    pub async fn commit_documents(
        &self,
        author: &str,
        message_title: &str,
        message_body: &str,
        files: Vec<(String, Option<Vec<u8>>)>,
        expected_versions: Vec<(String, Option<Oid>)>,
    ) -> Result<WriteOutcome, WriteError> {
        if !is_valid_message_title(message_title) {
            return Err(WriteError::BadMessage);
        }

        let _serialized = self.write_lock.lock().await;
        // Two attempts: the second runs against the head the first one lost to,
        // so a single hand commit landing in the middle costs a retry, not an
        // error the caller has to handle.
        for attempt in 0..2 {
            let snapshot = self.snapshot().await?;
            for (path, expected) in &expected_versions {
                let current = version_at(&snapshot, path);
                if current != *expected {
                    return Err(WriteError::VersionMismatch {
                        path: path.clone(),
                        current,
                    });
                }
            }

            let repository = self.repository.clone();
            let author = author.to_string();
            let title = message_title.to_string();
            let body = message_body.to_string();
            let files = files.clone();
            let expected_head = snapshot.head;
            let outcome = tokio::task::spawn_blocking(move || {
                let repository = repository.lock().expect("the store lock is not poisoned");
                repository.commit_files(&author, &title, &body, files, Some(expected_head))
            })
            .await?;

            match outcome {
                Ok(CommitOutcome {
                    commit_oid,
                    blob_oids,
                }) => {
                    // Leaves the shared snapshot at the commit just made, so a
                    // reader that arrives before the next head check sees it.
                    self.snapshot().await?;
                    return Ok(WriteOutcome {
                        commit_oid,
                        blob_oids,
                    });
                }
                Err(GitError::HeadMoved { .. }) if attempt == 0 => continue,
                Err(GitError::HeadMoved { .. }) => return Err(WriteError::Conflict),
                Err(error) => return Err(WriteError::Git(error)),
            }
        }
        Err(WriteError::Conflict)
    }
}

/// The version of the document at `path` in `catalog`, or `None` when the
/// catalog holds no document there.
pub fn version_at(catalog: &Catalog, path: &str) -> Option<Oid> {
    if let Some(id) = MemoryId::from_repository_path(path) {
        return catalog.memory(&id).map(|memory| memory.version);
    }
    if let Some(id) = ScopeId::from_repository_path(path) {
        return catalog.scope(&id).map(|scope| scope.version);
    }
    None
}

/// Whether `title` is a usable commit title: one non-empty line within the
/// length `git log --oneline` shows.
pub fn is_valid_message_title(title: &str) -> bool {
    !title.trim().is_empty()
        && title.chars().count() <= MAX_MESSAGE_TITLE
        && !title.contains('\n')
        && !title.contains('\r')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryId;

    fn memory_file(name: &str, body: &str) -> Vec<u8> {
        format!("---\nname: {name}\ndescription: the index entry for {name}\n---\n{body}\n")
            .into_bytes()
    }

    /// Detects a message title check that lets through what the commit history
    /// cannot show: an empty title, a title spanning lines, or one past the
    /// length limit. Expectation source: the plan's write contract.
    #[test]
    fn message_titles_the_history_cannot_show_are_rejected() {
        assert!(is_valid_message_title("record the widget rule"));
        assert!(!is_valid_message_title(""));
        assert!(!is_valid_message_title("   "));
        assert!(!is_valid_message_title("first line\nsecond line"));
        assert!(is_valid_message_title(&"a".repeat(MAX_MESSAGE_TITLE)));
        assert!(!is_valid_message_title(&"a".repeat(MAX_MESSAGE_TITLE + 1)));
    }

    /// Detects a write path that commits against the head it cached rather than
    /// the current one: a commit made outside the server between two writes
    /// would then fail with a conflict the caller cannot resolve.
    #[tokio::test]
    async fn a_commit_made_outside_the_server_does_not_fail_the_next_write() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let store = Store::open(directory.path())
            .await
            .expect("the store opens");
        let first = store.snapshot().await.expect("a snapshot");

        // A second handle on the same repository, which is what a person
        // running `git commit` in the store is.
        let by_hand = GitRepo::open_or_init(directory.path()).expect("a second handle opens");
        let head = by_hand.head_oid().expect("the store has a head");
        by_hand
            .commit_files(
                "person",
                "add a memory by hand",
                "",
                vec![(
                    "memories/by-hand.md".to_string(),
                    Some(memory_file("by-hand", "written by hand")),
                )],
                Some(head),
            )
            .expect("the hand commit lands");

        let outcome = store
            .commit_documents(
                "test",
                "add a memory through the server",
                "",
                vec![(
                    "memories/by-server.md".to_string(),
                    Some(memory_file("by-server", "written by the server")),
                )],
                vec![("memories/by-server.md".to_string(), None)],
            )
            .await
            .expect("the write succeeds against the moved head");

        let after = store.snapshot().await.expect("a snapshot");
        assert_ne!(
            after.head, first.head,
            "the snapshot must advance to the commit just made"
        );
        assert!(
            after.memory(&MemoryId::new("by-hand")).is_some(),
            "the hand commit must survive the server's write"
        );
        assert_eq!(
            after
                .memory(&MemoryId::new("by-server"))
                .map(|memory| memory.version),
            outcome.blob_oids.get("memories/by-server.md").copied(),
            "the write must report the version the store now holds"
        );
    }

    /// Detects a write that ignores the version the caller read, which is the
    /// check that stops two editors from overwriting each other.
    #[tokio::test]
    async fn a_write_from_a_stale_version_is_refused() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let store = Store::open(directory.path())
            .await
            .expect("the store opens");
        store
            .commit_documents(
                "test",
                "add a memory",
                "",
                vec![(
                    "memories/rule.md".to_string(),
                    Some(memory_file("rule", "first text")),
                )],
                vec![("memories/rule.md".to_string(), None)],
            )
            .await
            .expect("the first write succeeds");

        let error = store
            .commit_documents(
                "test",
                "edit a memory from a version it no longer has",
                "",
                vec![(
                    "memories/rule.md".to_string(),
                    Some(memory_file("rule", "second text")),
                )],
                vec![("memories/rule.md".to_string(), None)],
            )
            .await
            .expect_err("a write from a stale version must be refused");

        assert!(
            matches!(error, WriteError::VersionMismatch { ref path, current: Some(_) } if path == "memories/rule.md"),
            "the refusal must name the path and the version it has now, got {error:?}"
        );
    }
}
