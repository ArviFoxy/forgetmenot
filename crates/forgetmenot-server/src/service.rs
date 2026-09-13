//! The shared store handle: one git repository, one catalog snapshot, one
//! write lock.
//!
//! Readers take a snapshot of the catalog once and use it for a whole request,
//! so every decision inside one request is made against one store version.
//! Writers serialize on an async mutex and commit compare-and-swap against the
//! head they read, so a commit made by hand between a read and a write makes
//! the write fail rather than discarding the hand commit.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use git2::Oid;

use crate::store::branch::{BranchName, BranchRecord};
use crate::store::catalog::{Catalog, LoadError};
use crate::store::git::{CommitOutcome, FileChange, FileConflict, GitError, GitRepo, LandAttempt};
use crate::store::validate::{self, ValidationError};
use crate::store::{MemoryId, ScopeId};

/// The longest a commit message's title line may be, the width `git log
/// --oneline` shows without wrapping.
pub const MAX_MESSAGE_TITLE: usize = 72;

/// How many times a land redoes its merge against a `main` that moved under it
/// before giving up and telling the caller to try again.
pub const LAND_ATTEMPTS: usize = 5;

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
    /// The write named a branch that is not open, so there is nothing to commit
    /// to; the write is not quietly redirected to `main`.
    #[error("there is no branch `{0}`")]
    NoSuchBranch(BranchName),
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
    /// The current snapshot of `main`. Swapped under a short write lock when the
    /// head moves; readers of the previous snapshot are never blocked.
    catalog: RwLock<Arc<Catalog>>,
    /// One snapshot per open branch, each at that branch's head. A write on a
    /// branch validates against its own catalog, and rebuilding it on every
    /// write would re-read the whole tree each time.
    branch_catalogs: RwLock<HashMap<BranchName, Arc<Catalog>>>,
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
            branch_catalogs: RwLock::new(HashMap::new()),
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

    // -----------------------------------------------------------------------
    // Transactions, which are branches
    //
    // Nothing on a branch is served to any context: the catalog contexts are
    // answered from follows `main` alone, and a branch's own catalog exists only
    // to check and validate the writes made on it.
    // -----------------------------------------------------------------------

    /// Open a branch for `owner`, at the current head of `main`.
    ///
    /// The name is `tx/<owner>-<n>` with the lowest `n` that is free, so a
    /// session that opens a second transaction gets a second branch rather than
    /// taking over the first.
    pub async fn open_branch(&self, owner: &str) -> Result<BranchName, StoreError> {
        let owner = owner.to_string();
        let branch = self
            .with_repository(move |repository| {
                let head = repository.head_oid()?;
                for counter in 1..=u32::MAX {
                    let candidate = BranchName::for_session(&owner, counter);
                    if repository.branch_head(&candidate)?.is_some() {
                        continue;
                    }
                    match repository.create_branch(&candidate, head, &owner) {
                        Ok(()) => {
                            return Ok(candidate);
                        }
                        // Another caller took this name between the two calls;
                        // the next number is tried rather than failing.
                        Err(GitError::BranchExists { .. }) => continue,
                        Err(error) => return Err(error),
                    }
                }
                Err(GitError::BranchExists { name: owner })
            })
            .await?;
        Ok(branch)
    }

    /// The catalog of one branch's head, or `None` when there is no such
    /// branch.
    pub async fn branch_snapshot(
        &self,
        branch: &BranchName,
    ) -> Result<Option<Arc<Catalog>>, StoreError> {
        let known = self
            .branch_catalogs
            .read()
            .expect("the catalog lock is not poisoned")
            .get(branch)
            .cloned();
        let known_head = known.as_ref().map(|catalog| catalog.head);
        let branch_for_task = branch.clone();
        let loaded = self
            .with_repository(move |repository| {
                let Some(head) = repository.branch_head(&branch_for_task)? else {
                    return Ok(BranchLoad::Gone);
                };
                if known_head == Some(head) {
                    return Ok(BranchLoad::Unchanged);
                }
                Ok(BranchLoad::Loaded(Box::new(
                    Catalog::load_at(repository, head).map_err(load_error_as_git)?,
                )))
            })
            .await?;

        match loaded {
            BranchLoad::Gone => {
                self.forget_branch_catalog(branch);
                Ok(None)
            }
            BranchLoad::Unchanged => Ok(known),
            BranchLoad::Loaded(catalog) => {
                let catalog = Arc::new(*catalog);
                self.branch_catalogs
                    .write()
                    .expect("the catalog lock is not poisoned")
                    .insert(branch.clone(), catalog.clone());
                Ok(Some(catalog))
            }
        }
    }

    /// Every open branch, with its record and how far it is from `main`.
    pub async fn branch_statuses(&self) -> Result<Vec<BranchStatus>, StoreError> {
        let statuses = self
            .with_repository(move |repository| {
                let main_head = repository.head_oid()?;
                let mut statuses = Vec::new();
                for name in repository.open_branches()? {
                    let Some(head) = repository.branch_head(&name)? else {
                        continue;
                    };
                    let (_, behind) = repository.ahead_behind(head, main_head)?;
                    let record = repository.branch_record(head, main_head)?;
                    statuses.push(BranchStatus {
                        // The writes on the branch, not every commit on it: the
                        // commit that opened it wrote nothing.
                        ahead: record.writes(),
                        record,
                        name,
                        behind,
                    });
                }
                Ok(statuses)
            })
            .await?;
        Ok(statuses)
    }

    /// What one branch would change, against the merge base with `main`, or
    /// `None` when there is no such branch.
    pub async fn branch_changes(
        &self,
        branch: &BranchName,
    ) -> Result<Option<BranchChanges>, StoreError> {
        let branch = branch.clone();
        let changes = self
            .with_repository(move |repository| {
                let main_head = repository.head_oid()?;
                let Some(head) = repository.branch_head(&branch)? else {
                    return Ok(None);
                };
                let base = repository.merge_base(head, main_head)?;
                let (_, behind) = repository.ahead_behind(head, main_head)?;
                Ok(Some(BranchChanges {
                    ahead: repository.branch_record(head, main_head)?.writes(),
                    behind,
                    files: repository.changes_between(base, head)?,
                }))
            })
            .await?;
        Ok(changes)
    }

    /// Delete one branch, whatever it holds. Reports whether there was a branch
    /// to delete.
    pub async fn abandon_branch(&self, branch: &BranchName) -> Result<bool, StoreError> {
        let name = branch.clone();
        let existed = self
            .with_repository(move |repository| repository.delete_branch(&name))
            .await?;
        self.forget_branch_catalog(branch);
        Ok(existed)
    }

    /// Delete every branch whose last activity is older than `retention`.
    ///
    /// A branch with no record is left alone: it is not this server's
    /// transaction and nothing says when it was last touched.
    pub async fn prune_idle_branches(
        &self,
        now: DateTime<Utc>,
        retention: Duration,
    ) -> Result<Vec<BranchName>, StoreError> {
        let idle: Vec<BranchName> = self
            .branch_statuses()
            .await?
            .into_iter()
            .filter(|status| is_idle(&status.record, now, retention))
            .map(|status| status.name)
            .collect();
        for branch in &idle {
            self.abandon_branch(branch).await?;
        }
        Ok(idle)
    }

    /// Write a set of documents to one branch as one commit.
    ///
    /// The same contract as [`Store::commit_documents`], except that
    /// `expected_versions` is checked against the branch's own head and nothing
    /// on `main` or in the working tree changes.
    pub async fn commit_documents_on_branch(
        &self,
        branch: &BranchName,
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
        for attempt in 0..2 {
            let Some(snapshot) = self.branch_snapshot(branch).await? else {
                return Err(WriteError::NoSuchBranch(branch.clone()));
            };
            for (path, expected) in &expected_versions {
                let current = version_at(&snapshot, path);
                if current != *expected {
                    return Err(WriteError::VersionMismatch {
                        path: path.clone(),
                        current,
                    });
                }
            }

            let expected_head = snapshot.head;
            let name = branch.clone();
            let author = author.to_string();
            let title = message_title.to_string();
            let body = message_body.to_string();
            let files = files.clone();
            let outcome = self
                .with_repository(move |repository| {
                    let outcome = repository.commit_files_on_branch(
                        &name,
                        &author,
                        &title,
                        &body,
                        files,
                        expected_head,
                    )?;
                    Ok(outcome)
                })
                .await;

            match outcome {
                Ok(CommitOutcome {
                    commit_oid,
                    blob_oids,
                }) => {
                    // Leaves the branch's snapshot at the commit just made.
                    self.branch_snapshot(branch).await?;
                    return Ok(WriteOutcome {
                        commit_oid,
                        blob_oids,
                    });
                }
                Err(StoreError::Git(GitError::HeadMoved { .. })) if attempt == 0 => continue,
                Err(StoreError::Git(GitError::HeadMoved { .. })) => {
                    return Err(WriteError::Conflict);
                }
                Err(StoreError::Git(GitError::NoSuchBranch { .. })) => {
                    return Err(WriteError::NoSuchBranch(branch.clone()));
                }
                Err(error) => return Err(WriteError::Store(error)),
            }
        }
        Err(WriteError::Conflict)
    }

    /// Squash one branch onto `main` as a single commit.
    ///
    /// The merge is redone against the new head when `main` moves between the
    /// merge and the reference move, up to [`LAND_ATTEMPTS`] times; after that
    /// the caller is told the store is too busy rather than being made to wait.
    /// A merge that conflicts, or a merged store the validator refuses, leaves
    /// `main` and the branch exactly as they were.
    pub async fn land_branch(
        &self,
        branch: &BranchName,
        author: &str,
        message_title: &str,
    ) -> Result<Oid, LandError> {
        if !is_valid_message_title(message_title) {
            return Err(LandError::BadMessage);
        }

        let _serialized = self.write_lock.lock().await;
        for _ in 0..LAND_ATTEMPTS {
            let name = branch.clone();
            let author = author.to_string();
            let title = message_title.to_string();
            let step = self
                .with_repository(move |repository| {
                    let prepared = match repository.prepare_land(&name, &author, &title)? {
                        LandAttempt::Conflicts(conflicts) => {
                            return Ok(LandStep::Conflicts(conflicts));
                        }
                        LandAttempt::Prepared(prepared) => prepared,
                    };
                    // The commit object exists but no reference names it yet, so
                    // the merged store can be read and validated before anything
                    // is published, and refusing it changes nothing.
                    let catalog = Catalog::load_at(repository, prepared.commit_oid)
                        .map_err(load_error_as_git)?;
                    let report = validate::validate(&catalog);
                    if report.has_errors() {
                        return Ok(LandStep::Invalid(report.errors().to_vec()));
                    }
                    match repository.finish_land(&name, &prepared) {
                        Ok(()) => Ok(LandStep::Landed(prepared.commit_oid)),
                        Err(GitError::HeadMoved { .. }) => Ok(LandStep::MainMoved),
                        Err(error) => Err(error),
                    }
                })
                .await;

            match step {
                Ok(LandStep::Landed(commit_oid)) => {
                    self.forget_branch_catalog(branch);
                    // Leaves the shared snapshot at the commit just landed, so a
                    // reader that arrives before the next head check sees it.
                    self.snapshot().await.map_err(LandError::Store)?;
                    return Ok(commit_oid);
                }
                Ok(LandStep::Conflicts(conflicts)) => {
                    return Err(LandError::Conflicts(conflicts));
                }
                Ok(LandStep::Invalid(errors)) => return Err(LandError::Invalid(errors)),
                Ok(LandStep::MainMoved) => continue,
                Err(StoreError::Git(GitError::NoSuchBranch { .. })) => {
                    return Err(LandError::NoSuchBranch(branch.clone()));
                }
                Err(StoreError::Git(GitError::NothingToLand { .. })) => {
                    return Err(LandError::NothingToLand(branch.clone()));
                }
                Err(error) => return Err(LandError::Store(error)),
            }
        }
        Err(LandError::Busy)
    }

    /// Run one piece of git work on a blocking thread, under the repository
    /// lock.
    async fn with_repository<Answer, Work>(&self, work: Work) -> Result<Answer, StoreError>
    where
        Work: FnOnce(&GitRepo) -> Result<Answer, GitError> + Send + 'static,
        Answer: Send + 'static,
    {
        let repository = self.repository.clone();
        let answer = tokio::task::spawn_blocking(move || {
            let repository = repository.lock().expect("the store lock is not poisoned");
            work(&repository)
        })
        .await?;
        Ok(answer?)
    }

    fn forget_branch_catalog(&self, branch: &BranchName) {
        self.branch_catalogs
            .write()
            .expect("the catalog lock is not poisoned")
            .remove(branch);
    }
}

/// What a read of a branch's catalog found.
enum BranchLoad {
    /// The branch is not there any more.
    Gone,
    /// The branch head is the one the cached catalog was built from.
    Unchanged,
    /// Boxed because a catalog is far larger than the other two answers and this
    /// enum only carries it from one task back to the caller.
    Loaded(Box<Catalog>),
}

/// What one attempt at landing produced.
enum LandStep {
    Landed(Oid),
    Conflicts(Vec<FileConflict>),
    Invalid(Vec<ValidationError>),
    /// `main` moved between the merge and the reference move, so the merge is
    /// made again against the new head.
    MainMoved,
}

/// One open branch, as the branch list reports it.
#[derive(Clone, Debug)]
pub struct BranchStatus {
    pub name: BranchName,
    pub record: BranchRecord,
    /// Commits the branch has that `main` does not.
    pub ahead: usize,
    /// Commits `main` has that the branch does not.
    pub behind: usize,
}

/// What one branch would change if it landed.
#[derive(Clone, Debug)]
pub struct BranchChanges {
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<FileChange>,
}

/// Why a branch did not land.
#[derive(Debug, thiserror::Error)]
pub enum LandError {
    #[error(
        "the commit message must be one non-empty line of at most {MAX_MESSAGE_TITLE} characters"
    )]
    BadMessage,
    #[error("there is no branch `{0}`")]
    NoSuchBranch(BranchName),
    #[error("the branch `{0}` has no commits to land")]
    NothingToLand(BranchName),
    /// The branch and `main` changed the same lines of the same files; the
    /// branch is left as it is so that the caller can resolve them on it.
    #[error("{} file(s) changed on both the branch and main", .0.len())]
    Conflicts(Vec<FileConflict>),
    /// The merged store breaks the store's own rules, so it is not published.
    #[error("the merged store is not valid: {} problem(s)", .0.len())]
    Invalid(Vec<ValidationError>),
    /// `main` moved under every attempt, so nothing was landed and the caller
    /// may simply try again.
    #[error("the store is being written concurrently; the branch was not landed")]
    Busy,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// A catalog that could not be read inside a git task, as a git failure, since
/// that is the only error such a task can report.
fn load_error_as_git(error: LoadError) -> GitError {
    match error {
        LoadError::Git(error) => error,
        // A single trigger pattern that does not compile is left out of the
        // catalog and reported by the validator; this is the whole index
        // failing to build, which no caller can act on.
        LoadError::Triggers(error) => GitError::Git(git2::Error::from_str(&format!(
            "the trigger index could not be compiled: {error}"
        ))),
    }
}

/// Whether a branch has been idle for longer than `retention`.
fn is_idle(record: &BranchRecord, now: DateTime<Utc>, retention: Duration) -> bool {
    let Some(last) = record.last_activity.or(record.created) else {
        return false;
    };
    match (now - last).to_std() {
        Ok(age) => age > retention,
        // A record stamped in the future is not an age; the branch is kept.
        Err(_) => false,
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

    /// Detects a retention window that deletes a branch somebody is still
    /// working on, or that never deletes anything: an idle transaction has to go
    /// and a fresh one has to stay, or open branches pile up for ever, or vanish
    /// under the agent that opened them.
    #[tokio::test]
    async fn only_a_branch_idle_for_longer_than_the_window_is_pruned() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let store = Store::open(directory.path())
            .await
            .expect("the store opens");
        let branch = store
            .open_branch("alpha/session-1")
            .await
            .expect("the branch opens");
        let window = Duration::from_secs(24 * 60 * 60);

        let pruned = store
            .prune_idle_branches(Utc::now(), window)
            .await
            .expect("pruning runs");
        assert!(
            pruned.is_empty(),
            "a branch opened just now is not idle, got {pruned:?}"
        );
        assert!(
            store
                .branch_snapshot(&branch)
                .await
                .expect("the branch is readable")
                .is_some(),
            "a branch that is not idle must still be there"
        );

        let later = Utc::now() + chrono::Duration::days(2);
        let pruned = store
            .prune_idle_branches(later, window)
            .await
            .expect("pruning runs");
        assert_eq!(
            pruned,
            vec![branch.clone()],
            "a branch idle for longer than the window must be deleted"
        );
        assert!(
            store
                .branch_snapshot(&branch)
                .await
                .expect("the branch is readable")
                .is_none(),
            "a pruned branch must be gone"
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
