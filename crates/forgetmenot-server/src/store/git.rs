//! The store's git repository.
//!
//! `GitRepo` is `Send` but not `Sync`; callers keep it behind a
//! `std::sync::Mutex` so that read HEAD, build the commit and move the branch
//! reference happen without another writer in between. The reference move is
//! itself a compare-and-swap, so a commit a person makes by hand between two
//! server operations makes the server's update fail instead of discarding it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use git2::build::CheckoutBuilder;
use git2::{
    BlameOptions, BranchType, Delta, DiffDelta, DiffFormat, DiffOptions, ErrorCode, Index,
    IndexEntry, IndexTime, ObjectType, Oid, Repository, RepositoryInitOptions, Signature, Sort,
    Tree,
};

use super::branch::{BranchName, BranchRecord, opening_message, owner_in_message};

/// The branch a new store is created on.
pub const DEFAULT_BRANCH: &str = "main";

/// The domain used for the synthetic author email addresses.
const AUTHOR_EMAIL_DOMAIN: &str = "forgetmenot";

/// The file mode recorded for every file the server writes.
const REGULAR_FILE_MODE: u32 = 0o100_644;

/// Something that stopped a store operation.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    /// The branch no longer points where the caller expected, so the caller's
    /// decisions were made against a store version that is no longer current.
    #[error("the store moved on; its head is now {current}")]
    HeadMoved { current: Oid },
    /// Committing would discard an edit made in the working tree.
    #[error("the working tree has uncommitted changes to {path}")]
    DirtyWorkingTree { path: String },
    #[error("the store has no working tree")]
    NoWorkingTree,
    /// The repository exists but nothing has ever been committed to it, so
    /// there is no revision to read.
    #[error("the store has no commits yet")]
    NoHistory,
    #[error("the store contains a path that is not valid UTF-8")]
    NonUtf8Path,
    /// A branch of that name is already open, so creating it again would take
    /// over someone else's transaction.
    #[error("the branch `{name}` already exists")]
    BranchExists { name: String },
    #[error("there is no branch `{name}`")]
    NoSuchBranch { name: String },
    /// The branch holds no commit `main` does not already have, so there is
    /// nothing to squash into one.
    #[error("the branch `{name}` has no commits to land")]
    NothingToLand { name: String },
}

/// A set of file writes: each path with its new bytes, or `None` to delete it.
pub type FileWrites = Vec<(String, Option<Vec<u8>>)>;

/// One file in a revision of the store.
#[derive(Clone, Debug)]
pub struct TreeEntry {
    /// Path inside the repository, with `/` separators.
    pub path: String,
    /// The blob's object id, used as the version of a document.
    pub blob_oid: Oid,
    pub bytes: Vec<u8>,
}

/// One commit in a file's history.
#[derive(Clone, Debug)]
pub struct CommitSummary {
    pub oid: Oid,
    pub time: DateTime<Utc>,
    pub author: String,
    /// The first line of the commit message.
    pub title: String,
}

/// One line of a file with the commit that last changed it.
#[derive(Clone, Debug)]
pub struct BlameLine {
    /// The line's number in the file, counting from one.
    pub line: usize,
    /// The commit that last changed the line.
    pub oid: String,
    pub time: DateTime<Utc>,
    pub author: String,
    /// The line itself, without its newline.
    pub text: String,
}

/// What a successful [`GitRepo::commit_files`] produced.
#[derive(Clone, Debug)]
pub struct CommitOutcome {
    pub commit_oid: Oid,
    /// The new blob id of each written file, which becomes its version.
    pub blob_oids: BTreeMap<String, Oid>,
}

/// The store's git repository.
pub struct GitRepo {
    repository: Repository,
    /// The full name of the branch HEAD points at, moved by every commit.
    branch_reference: String,
}

impl GitRepo {
    /// Open an existing store without changing anything on disk.
    ///
    /// Read-only callers use this so that a mistyped path, or a directory of
    /// memory files nobody has committed, is an error rather than a new empty
    /// store that looks valid.
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let store = Self::with_branch_from_head(Repository::open(path)?)?;
        if store.has_history()? {
            Ok(store)
        } else {
            Err(GitError::NoHistory)
        }
    }

    /// Open the store at `path`, creating it with an empty initial commit if
    /// there is no repository or no commit there yet.
    ///
    /// An existing repository keeps its branch and its history.
    pub fn open_or_init(path: &Path) -> Result<Self, GitError> {
        let repository = match Repository::open(path) {
            Ok(repository) => repository,
            Err(_) => {
                let mut options = RepositoryInitOptions::new();
                options.initial_head(DEFAULT_BRANCH).mkpath(true);
                Repository::init_opts(path, &options)?
            }
        };
        let store = Self::with_branch_from_head(repository)?;
        if !store.has_history()? {
            store.write_initial_commit()?;
        } else if store.repository.workdir().is_some() {
            // A store that was never checked out would otherwise look like a
            // working tree in which every file has been deleted by hand. A safe
            // checkout fills in missing files and never overwrites an edit.
            let mut checkout = CheckoutBuilder::new();
            checkout.safe().recreate_missing(true);
            store.repository.checkout_head(Some(&mut checkout))?;
        }
        Ok(store)
    }

    fn with_branch_from_head(repository: Repository) -> Result<Self, GitError> {
        // HEAD is symbolic whether or not the branch has a commit yet, so this
        // finds the branch to move even in a repository with no history.
        let branch_reference = repository
            .find_reference("HEAD")?
            .symbolic_target()?
            .map(str::to_string)
            .unwrap_or_else(|| format!("refs/heads/{DEFAULT_BRANCH}"));
        Ok(Self {
            repository,
            branch_reference,
        })
    }

    /// Whether the branch has a commit, asked by resolving it: libgit2's
    /// `is_empty` compares HEAD against the configured default branch name and
    /// so reports a fresh repository on `main` as non-empty.
    fn has_history(&self) -> Result<bool, GitError> {
        match self.repository.head() {
            Ok(head) => Ok(head.peel_to_commit().is_ok()),
            Err(error) if matches!(error.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The commit the branch points at.
    pub fn head_oid(&self) -> Result<Oid, GitError> {
        Ok(self.repository.head()?.peel_to_commit()?.id())
    }

    /// The directory the working tree lives in.
    pub fn workdir(&self) -> Result<&Path, GitError> {
        self.repository.workdir().ok_or(GitError::NoWorkingTree)
    }

    /// Every file in the revision `commit_oid`, with its bytes.
    pub fn read_tree(&self, commit_oid: Oid) -> Result<Vec<TreeEntry>, GitError> {
        let tree = self.repository.find_commit(commit_oid)?.tree()?;
        let mut entries = Vec::new();
        self.collect_tree(&tree, "", &mut entries)?;
        Ok(entries)
    }

    /// Commit a set of file writes and deletions as one commit.
    ///
    /// `files` pairs a repository path with its new bytes, or with `None` to
    /// delete it. `expected_head` is the commit the caller read; if the branch
    /// has moved since, nothing is committed and [`GitError::HeadMoved`] names
    /// the current head.
    pub fn commit_files(
        &self,
        author_name: &str,
        message_title: &str,
        message_body: &str,
        files: Vec<(String, Option<Vec<u8>>)>,
        expected_head: Option<Oid>,
    ) -> Result<CommitOutcome, GitError> {
        let parent_commit = self.repository.head()?.peel_to_commit()?;
        let current_head = parent_commit.id();
        if let Some(expected) = expected_head
            && expected != current_head
        {
            return Err(GitError::HeadMoved {
                current: current_head,
            });
        }
        let parent_tree = parent_commit.tree()?;

        // Checked before anything is written: a file that differs from the
        // committed version holds an edit made by hand, and committing over it
        // would lose work that was never recorded anywhere.
        for (path, _) in &files {
            self.refuse_if_edited_by_hand(&parent_tree, path)?;
        }

        let (tree_oid, blob_oids) = self.tree_with(&parent_tree, &files)?;
        // The commit object is written without moving any reference, so a
        // failed compare-and-swap below leaves the branch exactly as it was.
        let commit_oid = self.write_commit(
            author_name,
            message_title,
            message_body,
            tree_oid,
            &[&parent_commit],
        )?;

        self.repository
            .reference_matching(
                &self.branch_reference,
                commit_oid,
                true,
                current_head,
                &format!("forgetmenot: {message_title}"),
            )
            .map_err(|error| self.head_moved_or(error))?;

        self.update_working_tree(&files)?;
        Ok(CommitOutcome {
            commit_oid,
            blob_oids,
        })
    }

    /// The commits that changed `path`, newest first.
    ///
    /// A commit is included when the file's blob differs from the blob in its
    /// first parent, which is what `git log -- path` reports.
    pub fn log_for_path(&self, path: &str) -> Result<Vec<CommitSummary>, GitError> {
        let mut walk = self.repository.revwalk()?;
        // Topological order puts every commit before its parents, so a linear
        // history comes out newest first without depending on commit clocks.
        walk.set_sorting(Sort::TOPOLOGICAL)?;
        walk.push_head()?;

        let mut summaries = Vec::new();
        for oid in walk {
            let commit = self.repository.find_commit(oid?)?;
            let in_commit = blob_oid_at(&commit.tree()?, path);
            let in_parent = match commit.parent(0) {
                Ok(parent) => blob_oid_at(&parent.tree()?, path),
                Err(_) => None,
            };
            if in_commit != in_parent {
                summaries.push(CommitSummary {
                    oid: commit.id(),
                    time: commit_time(&commit),
                    author: commit.author().name().unwrap_or_default().to_string(),
                    title: commit
                        .message()
                        .unwrap_or_default()
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                });
            }
        }
        Ok(summaries)
    }

    /// Every line of `path` at the head of the branch HEAD points at, with the
    /// commit that last changed it, in line order.
    ///
    /// The whole file as it is stored, frontmatter included, so a line number
    /// here is the line number in the file. The text of each line comes from the
    /// head revision, and the commit, its time and its author are the ones
    /// [`GitRepo::log_for_path`] reports for that commit.
    pub fn blame(&self, path: &str) -> Result<Vec<BlameLine>, GitError> {
        let head = self.head_oid()?;
        let mut options = BlameOptions::new();
        options.newest_commit(head);
        let blame = self
            .repository
            .blame_file(Path::new(path), Some(&mut options))?;
        let content = self.blob_at(head, path)?.unwrap_or_default();
        let text = String::from_utf8_lossy(&content);
        let file_lines: Vec<&str> = text.lines().collect();

        let mut lines = Vec::with_capacity(file_lines.len());
        for hunk in blame.iter() {
            let commit = self.repository.find_commit(hunk.final_commit_id())?;
            let oid = commit.id().to_string();
            let time = commit_time(&commit);
            let author = commit.author().name().unwrap_or_default().to_string();
            for offset in 0..hunk.lines_in_hunk() {
                let number = hunk.final_start_line() + offset;
                lines.push(BlameLine {
                    line: number,
                    oid: oid.clone(),
                    time,
                    author: author.clone(),
                    // A blame counts lines from one, and a hunk that reaches
                    // past the file's last line has no text to show.
                    text: file_lines
                        .get(number - 1)
                        .copied()
                        .unwrap_or_default()
                        .to_string(),
                });
            }
        }
        lines.sort_by_key(|line| line.line);
        Ok(lines)
    }

    /// The content of `path` in the revision `commit_oid`.
    pub fn blob_at(&self, commit_oid: Oid, path: &str) -> Result<Option<Vec<u8>>, GitError> {
        let tree = self.repository.find_commit(commit_oid)?.tree()?;
        match blob_oid_at(&tree, path) {
            Some(blob_oid) => Ok(Some(
                self.repository.find_blob(blob_oid)?.content().to_vec(),
            )),
            None => Ok(None),
        }
    }

    /// The content of the blob `blob_oid`, or `None` when the repository holds no
    /// blob of that id.
    ///
    /// The blob is addressed by its own id rather than through a revision, which
    /// is how a version recorded as delivered is read back: the commit it came
    /// from is not recorded, and the blob outlives the commit anyway.
    pub fn read_blob(&self, blob_oid: Oid) -> Result<Option<Vec<u8>>, GitError> {
        match self.repository.find_blob(blob_oid) {
            Ok(blob) => Ok(Some(blob.content().to_vec())),
            Err(error) if matches!(error.code(), ErrorCode::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// A unified diff of `path` between `commit_oid` and its first parent. The
    /// root commit is diffed against an empty tree.
    pub fn diff_for_path(&self, commit_oid: Oid, path: &str) -> Result<String, GitError> {
        let commit = self.repository.find_commit(commit_oid)?;
        let new_tree = commit.tree()?;
        let old_tree = match commit.parent(0) {
            Ok(parent) => Some(parent.tree()?),
            Err(_) => None,
        };
        let mut options = DiffOptions::new();
        options.pathspec(path);
        let diff = self.repository.diff_tree_to_tree(
            old_tree.as_ref(),
            Some(&new_tree),
            Some(&mut options),
        )?;

        let mut text = String::new();
        diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
            if matches!(line.origin(), '+' | '-' | ' ') {
                text.push(line.origin());
            }
            text.push_str(&String::from_utf8_lossy(line.content()));
            true
        })?;
        Ok(text)
    }

    // -----------------------------------------------------------------------
    // Transaction branches
    //
    // A transaction is a branch under `tx/`. Nothing here touches the working
    // tree: the checkout follows `main`, so a write on a branch leaves the
    // files on disk alone and a later write to `main` still sees a clean tree.
    // -----------------------------------------------------------------------

    /// The commit `branch` points at, or `None` when there is no such branch.
    pub fn branch_head(&self, branch: &BranchName) -> Result<Option<Oid>, GitError> {
        match self
            .repository
            .find_branch(branch.as_str(), BranchType::Local)
        {
            Ok(found) => Ok(Some(found.get().peel_to_commit()?.id())),
            Err(error) if matches!(error.code(), ErrorCode::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Every transaction branch the repository holds, by name.
    ///
    /// Branches outside the prefix are left out, so `main` and a person's own
    /// branches are not transactions and cannot be listed, landed or deleted.
    pub fn open_branches(&self) -> Result<Vec<BranchName>, GitError> {
        let mut names = Vec::new();
        for found in self.repository.branches(Some(BranchType::Local))? {
            let (found, _) = found?;
            let Some(name) = found.name()? else {
                continue;
            };
            if let Ok(branch) = BranchName::parse(name) {
                names.push(branch);
            }
        }
        names.sort();
        Ok(names)
    }

    /// Create `branch` at `from` with the commit that opens it, failing when a
    /// branch of that name exists.
    ///
    /// The opening commit changes nothing: its tree is `from`'s tree. It is there
    /// so that who opened the branch and when are recorded where everything else
    /// about the branch is, which is the branch itself.
    pub fn create_branch(
        &self,
        branch: &BranchName,
        from: Oid,
        owner: &str,
    ) -> Result<(), GitError> {
        let parent = self.repository.find_commit(from)?;
        let (title, body) = opening_message(branch, owner);
        let opening = self.write_commit(owner, &title, &body, parent.tree()?.id(), &[&parent])?;
        let commit = self.repository.find_commit(opening)?;
        match self.repository.branch(branch.as_str(), &commit, false) {
            Ok(_) => Ok(()),
            Err(error) if matches!(error.code(), ErrorCode::Exists) => {
                Err(GitError::BranchExists {
                    name: branch.to_string(),
                })
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Delete `branch`. Reports whether there was a branch to delete.
    pub fn delete_branch(&self, branch: &BranchName) -> Result<bool, GitError> {
        match self
            .repository
            .find_branch(branch.as_str(), BranchType::Local)
        {
            Ok(mut found) => {
                found.delete()?;
                Ok(true)
            }
            Err(error) if matches!(error.code(), ErrorCode::NotFound) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Commit a set of file writes and deletions to `branch` as one commit.
    ///
    /// `expected_head` is the branch head the caller read; if the branch has
    /// moved since, nothing is committed and [`GitError::HeadMoved`] names
    /// where it is now. The working tree is not touched, because it follows
    /// `main`.
    ///
    /// The commit brings `main` into the branch as well when `main` has moved on
    /// and the two merge, taking the written files from the write. That is what
    /// makes overwriting a file the way to resolve a conflict: afterwards the
    /// branch holds `main`'s content and the caller's, so the land has nothing
    /// left to merge in that file. When a file the write does not touch cannot be
    /// merged, the write is committed on the branch alone and the land reports
    /// that file, because picking a side there is not this write's decision.
    pub fn commit_files_on_branch(
        &self,
        branch: &BranchName,
        author_name: &str,
        message_title: &str,
        message_body: &str,
        files: Vec<(String, Option<Vec<u8>>)>,
        expected_head: Oid,
    ) -> Result<CommitOutcome, GitError> {
        let current_head = self
            .branch_head(branch)?
            .ok_or_else(|| GitError::NoSuchBranch {
                name: branch.to_string(),
            })?;
        if current_head != expected_head {
            return Err(GitError::HeadMoved {
                current: current_head,
            });
        }
        let parent_commit = self.repository.find_commit(current_head)?;
        let main_head = self.head_oid()?;
        let main_commit = self.repository.find_commit(main_head)?;
        let written: BTreeSet<String> = files.iter().map(|(path, _)| path.clone()).collect();
        let synced = if main_head == current_head
            || self
                .repository
                .graph_descendant_of(current_head, main_head)?
        {
            // The branch already holds every commit `main` has.
            None
        } else {
            self.merged_index(&parent_commit, &main_commit, &written)?
        };

        let (tree_oid, blob_oids, parents) = match synced {
            Some(mut index) => {
                let blob_oids = self.apply_files(&mut index, &files)?;
                let tree_oid = index.write_tree_to(&self.repository)?;
                (tree_oid, blob_oids, vec![&parent_commit, &main_commit])
            }
            None => {
                let (tree_oid, blob_oids) = self.tree_with(&parent_commit.tree()?, &files)?;
                (tree_oid, blob_oids, vec![&parent_commit])
            }
        };
        let commit_oid =
            self.write_commit(author_name, message_title, message_body, tree_oid, &parents)?;
        self.repository
            .reference_matching(
                &branch.reference(),
                commit_oid,
                true,
                current_head,
                &format!("forgetmenot: {message_title}"),
            )
            .map_err(|error| self.head_moved_or_branch(branch, error))?;
        Ok(CommitOutcome {
            commit_oid,
            blob_oids,
        })
    }

    /// How many commits `head` has that `upstream` does not, and the other way
    /// round.
    pub fn ahead_behind(&self, head: Oid, upstream: Oid) -> Result<(usize, usize), GitError> {
        Ok(self.repository.graph_ahead_behind(head, upstream)?)
    }

    /// The commit `head` and `upstream` last had in common.
    pub fn merge_base(&self, head: Oid, upstream: Oid) -> Result<Oid, GitError> {
        Ok(self.repository.merge_base(head, upstream)?)
    }

    /// The revision one commit was made against: its first parent.
    ///
    /// Every commit this server writes to `main`, a land included, has exactly
    /// one parent, so this is the store as it was before that commit.
    pub fn first_parent(&self, commit_oid: Oid) -> Result<Oid, GitError> {
        Ok(self.repository.find_commit(commit_oid)?.parent_id(0)?)
    }

    /// What changed in each file between two revisions, with the file's diff.
    pub fn changes_between(&self, base: Oid, head: Oid) -> Result<Vec<FileChange>, GitError> {
        let base_tree = self.repository.find_commit(base)?.tree()?;
        let head_tree = self.repository.find_commit(head)?.tree()?;
        let diff = self
            .repository
            .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), None)?;

        let mut changes: Vec<FileChange> = Vec::new();
        let mut position = BTreeMap::new();
        for delta in diff.deltas() {
            let path = delta_path(&delta).ok_or(GitError::NonUtf8Path)?;
            position.insert(path.clone(), changes.len());
            changes.push(FileChange {
                path,
                status: ChangeStatus::of(delta.status()),
                diff: String::new(),
            });
        }
        // The patch is printed once and each line appended to the file it
        // belongs to, so one read of the diff produces every file's text.
        diff.print(DiffFormat::Patch, |delta, _hunk, line| {
            if let Some(path) = delta_path(&delta)
                && let Some(index) = position.get(&path)
            {
                let text = &mut changes[*index].diff;
                if matches!(line.origin(), '+' | '-' | ' ') {
                    text.push(line.origin());
                }
                text.push_str(&String::from_utf8_lossy(line.content()));
            }
            true
        })?;
        Ok(changes)
    }

    /// What the commits `head` has and `upstream` does not say about the branch:
    /// who opened it, when, when it was last written to, and the title of every
    /// commit that changed a file, oldest first.
    ///
    /// Everything comes from the commits themselves, so a restart rediscovers it
    /// and there is no state file that could disagree with the refs.
    pub fn branch_record(&self, head: Oid, upstream: Oid) -> Result<BranchRecord, GitError> {
        let mut walk = self.repository.revwalk()?;
        // Oldest first, so the first commit seen is the one that opened the
        // branch and the last one seen is its newest.
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;
        walk.push(head)?;
        walk.hide(upstream)?;

        let mut record = BranchRecord::default();
        for oid in walk {
            let commit = self.repository.find_commit(oid?)?;
            let message = commit.message().unwrap_or_default().to_string();
            if record.created.is_none() {
                record.created = Some(commit_time(&commit));
                record.owner = owner_in_message(&message);
            }
            record.last_activity = Some(commit_time(&commit));
            // A commit whose tree is its parent's tree wrote nothing: the commit
            // that opens a branch is one, and it is not one of the writes.
            if self.changes_a_file(&commit)? {
                record
                    .titles
                    .push(message.lines().next().unwrap_or_default().to_string());
            }
        }
        Ok(record)
    }

    /// Whether `commit` differs from its first parent, which is what "this
    /// commit wrote something" means.
    fn changes_a_file(&self, commit: &git2::Commit<'_>) -> Result<bool, GitError> {
        match commit.parent(0) {
            Ok(parent) => Ok(parent.tree()?.id() != commit.tree()?.id()),
            Err(_) => Ok(true),
        }
    }

    /// Merge `branch` into the current `main` and write the squashed commit,
    /// without moving any reference.
    ///
    /// The merge is the three-way merge of the branch head into the head of
    /// `main` against their merge base, so an edit made on `main` since the
    /// branch was opened is merged rather than lost. A file the two changed
    /// incompatibly is reported as a conflict and nothing is written.
    ///
    /// The commit object exists after this returns but no reference names it,
    /// so a caller that refuses it leaves the store exactly as it was; git
    /// collects the unreferenced object.
    pub fn prepare_land(
        &self,
        branch: &BranchName,
        author_name: &str,
        message_title: &str,
    ) -> Result<LandAttempt, GitError> {
        let main_head = self.head_oid()?;
        let branch_head = self
            .branch_head(branch)?
            .ok_or_else(|| GitError::NoSuchBranch {
                name: branch.to_string(),
            })?;
        let main_commit = self.repository.find_commit(main_head)?;
        let branch_commit = self.repository.find_commit(branch_head)?;
        let titles = self.branch_record(branch_head, main_head)?.titles;
        if titles.is_empty() {
            return Err(GitError::NothingToLand {
                name: branch.to_string(),
            });
        }

        let merged = self
            .repository
            .merge_commits(&main_commit, &branch_commit, None)?;
        if merged.has_conflicts() {
            return Ok(LandAttempt::Conflicts(self.conflicts_of(&merged)?));
        }
        // `merge_commits` builds its own index, so writing the tree from it
        // cannot disturb the repository's index or the working tree.
        let mut merged = merged;
        let tree_oid = merged.write_tree_to(&self.repository)?;

        let body = land_message_body(branch, author_name, &titles);
        let commit_oid = self.write_commit(
            author_name,
            message_title,
            &body,
            tree_oid,
            // One parent, so the history stays the line `git log` shows without
            // a graph: the branch's own commits are squashed into this one.
            &[&main_commit],
        )?;

        let main_tree = main_commit.tree()?;
        let merged_tree = self.repository.find_tree(tree_oid)?;
        Ok(LandAttempt::Prepared(PreparedLand {
            commit_oid,
            main_head,
            changed: self.files_between(&main_tree, &merged_tree)?,
        }))
    }

    /// Move `main` to a prepared squash commit and delete the branch.
    ///
    /// The reference move is a compare-and-swap against the head the merge was
    /// made against, so a commit that landed in between makes this fail and the
    /// caller merge again.
    pub fn finish_land(
        &self,
        branch: &BranchName,
        prepared: &PreparedLand,
    ) -> Result<(), GitError> {
        // The same check a direct commit makes, for the files this land would
        // write: an edit made by hand in the working tree is work that was
        // never recorded and must not be written over.
        let main_tree = self.repository.find_commit(prepared.main_head)?.tree()?;
        for (path, _) in &prepared.changed {
            self.refuse_if_edited_by_hand(&main_tree, path)?;
        }
        self.repository
            .reference_matching(
                &self.branch_reference,
                prepared.commit_oid,
                true,
                prepared.main_head,
                &format!("forgetmenot: land {branch}"),
            )
            .map_err(|error| self.head_moved_or(error))?;
        self.update_working_tree(&prepared.changed)?;
        self.delete_branch(branch)?;
        Ok(())
    }

    /// The index that would hold the result of writing `files` over
    /// `parent_tree`, as a tree, with each written file's new blob id.
    ///
    /// An index of its own, not the repository's, so that building the tree
    /// cannot disturb what is staged in the working tree.
    fn tree_with(
        &self,
        parent_tree: &Tree<'_>,
        files: &[(String, Option<Vec<u8>>)],
    ) -> Result<(Oid, BTreeMap<String, Oid>), GitError> {
        let mut tree_index = Index::new()?;
        tree_index.read_tree(parent_tree)?;
        let blob_oids = self.apply_files(&mut tree_index, files)?;
        let tree_oid = tree_index.write_tree_to(&self.repository)?;
        Ok((tree_oid, blob_oids))
    }

    /// Write `files` into `index`, reporting each written file's new blob id.
    fn apply_files(
        &self,
        index: &mut Index,
        files: &[(String, Option<Vec<u8>>)],
    ) -> Result<BTreeMap<String, Oid>, GitError> {
        let mut blob_oids = BTreeMap::new();
        for (path, content) in files {
            match content {
                Some(bytes) => {
                    let blob_oid = self.repository.blob(bytes)?;
                    index.add(&index_entry(path, blob_oid, bytes.len()))?;
                    blob_oids.insert(path.clone(), blob_oid);
                }
                None => {
                    if index.get_path(Path::new(path), 0).is_some() {
                        index.remove_path(Path::new(path))?;
                    }
                }
            }
        }
        Ok(blob_oids)
    }

    /// The index of `branch` with `main` merged into it, or `None` when a file
    /// the write does not touch cannot be merged.
    ///
    /// A conflict in a file the write overwrites is dropped: the write supplies
    /// that file's whole content, which is the caller resolving it.
    fn merged_index(
        &self,
        branch_commit: &git2::Commit<'_>,
        main_commit: &git2::Commit<'_>,
        written: &BTreeSet<String>,
    ) -> Result<Option<Index>, GitError> {
        let mut index = self
            .repository
            .merge_commits(branch_commit, main_commit, None)?;
        if !index.has_conflicts() {
            return Ok(Some(index));
        }
        let mut conflicted = Vec::new();
        for conflict in index.conflicts()? {
            conflicted.push(conflict_path(&conflict?)?);
        }
        if conflicted.iter().any(|path| !written.contains(path)) {
            return Ok(None);
        }
        for path in conflicted {
            index.conflict_remove(Path::new(&path))?;
        }
        Ok(Some(index))
    }

    /// Write a commit object without moving any reference, so that a caller
    /// that refuses it leaves every branch exactly as it was.
    fn write_commit(
        &self,
        author_name: &str,
        message_title: &str,
        message_body: &str,
        tree_oid: Oid,
        parents: &[&git2::Commit<'_>],
    ) -> Result<Oid, GitError> {
        let tree = self.repository.find_tree(tree_oid)?;
        let signature = signature_for(author_name)?;
        let message = compose_message(message_title, message_body);
        Ok(self
            .repository
            .commit(None, &signature, &signature, &message, &tree, parents)?)
    }

    /// The files that differ between two trees, as the write list that turns
    /// the first into the second.
    fn files_between(&self, from: &Tree<'_>, to: &Tree<'_>) -> Result<FileWrites, GitError> {
        let diff = self
            .repository
            .diff_tree_to_tree(Some(from), Some(to), None)?;
        let mut files = Vec::new();
        for delta in diff.deltas() {
            let path = delta_path(&delta).ok_or(GitError::NonUtf8Path)?;
            let content = match blob_oid_at(to, &path) {
                Some(blob_oid) => Some(self.repository.find_blob(blob_oid)?.content().to_vec()),
                None => None,
            };
            files.push((path, content));
        }
        Ok(files)
    }

    /// The files a merge could not resolve, with the three versions of each.
    fn conflicts_of(&self, merged: &Index) -> Result<Vec<FileConflict>, GitError> {
        let mut conflicts = Vec::new();
        for conflict in merged.conflicts()? {
            let conflict = conflict?;
            let content = |entry: &Option<IndexEntry>| -> Result<Option<Vec<u8>>, GitError> {
                match entry {
                    Some(entry) => Ok(Some(
                        self.repository.find_blob(entry.id)?.content().to_vec(),
                    )),
                    None => Ok(None),
                }
            };
            conflicts.push(FileConflict {
                path: conflict_path(&conflict)?,
                base: content(&conflict.ancestor)?,
                ours: content(&conflict.our)?,
                theirs: content(&conflict.their)?,
            });
        }
        Ok(conflicts)
    }

    /// Turn libgit2's "the reference moved" code into [`GitError::HeadMoved`]
    /// carrying where the branch is now.
    fn head_moved_or_branch(&self, branch: &BranchName, error: git2::Error) -> GitError {
        if error.code() == ErrorCode::Modified {
            match self.branch_head(branch) {
                Ok(Some(current)) => GitError::HeadMoved { current },
                Ok(None) => GitError::NoSuchBranch {
                    name: branch.to_string(),
                },
                Err(other) => other,
            }
        } else {
            GitError::Git(error)
        }
    }

    /// The initial commit of a store with no history: an empty tree, so that
    /// every later commit has a parent and HEAD always resolves.
    fn write_initial_commit(&self) -> Result<Oid, GitError> {
        let tree_oid = Index::new()?.write_tree_to(&self.repository)?;
        let tree = self.repository.find_tree(tree_oid)?;
        let signature = signature_for(AUTHOR_EMAIL_DOMAIN)?;
        let commit_oid = self.repository.commit(
            Some(&self.branch_reference),
            &signature,
            &signature,
            "initialize store\n",
            &tree,
            &[],
        )?;
        self.repository.set_head(&self.branch_reference)?;
        Ok(commit_oid)
    }

    fn collect_tree(
        &self,
        tree: &Tree<'_>,
        prefix: &str,
        entries: &mut Vec<TreeEntry>,
    ) -> Result<(), GitError> {
        for entry in tree.iter() {
            let name = entry.name().map_err(|_| GitError::NonUtf8Path)?;
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            match entry.kind() {
                Some(ObjectType::Tree) => {
                    let subtree = self.repository.find_tree(entry.id())?;
                    self.collect_tree(&subtree, &path, entries)?;
                }
                Some(ObjectType::Blob) => {
                    let blob = self.repository.find_blob(entry.id())?;
                    entries.push(TreeEntry {
                        path,
                        blob_oid: entry.id(),
                        bytes: blob.content().to_vec(),
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Refuse when the file on disk differs from the committed version, which
    /// is exactly the condition "the working tree has uncommitted changes to
    /// this path": a modification, a deletion, or a new untracked file.
    fn refuse_if_edited_by_hand(&self, head_tree: &Tree<'_>, path: &str) -> Result<(), GitError> {
        let workdir = self.workdir()?;
        let committed = match blob_oid_at(head_tree, path) {
            Some(blob_oid) => Some(self.repository.find_blob(blob_oid)?.content().to_vec()),
            None => None,
        };
        let on_disk = match std::fs::read(workdir.join(path)) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if committed == on_disk {
            Ok(())
        } else {
            Err(GitError::DirtyWorkingTree {
                path: path.to_string(),
            })
        }
    }

    /// Bring the working tree and the index up to the commit just made, so
    /// that a person's next hand edit starts from the latest content.
    fn update_working_tree(&self, files: &[(String, Option<Vec<u8>>)]) -> Result<(), GitError> {
        let workdir = self.workdir()?.to_path_buf();
        let mut index = self.repository.index()?;
        for (path, content) in files {
            let absolute = workdir.join(path);
            match content {
                Some(bytes) => {
                    if let Some(parent) = absolute.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&absolute, bytes)?;
                    index.add_path(Path::new(path))?;
                }
                None => {
                    match std::fs::remove_file(&absolute) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                    if index.get_path(Path::new(path), 0).is_some() {
                        index.remove_path(Path::new(path))?;
                    }
                }
            }
        }
        index.write()?;
        Ok(())
    }

    /// Turn libgit2's "the reference moved" code into [`GitError::HeadMoved`]
    /// carrying where it moved to.
    fn head_moved_or(&self, error: git2::Error) -> GitError {
        if error.code() == ErrorCode::Modified {
            match self.head_oid() {
                Ok(current) => GitError::HeadMoved { current },
                Err(other) => other,
            }
        } else {
            GitError::Git(error)
        }
    }
}

/// What one revision did to one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
}

impl ChangeStatus {
    /// The word the API reports, which the frontend and the model read.
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeStatus::Added => "added",
            ChangeStatus::Modified => "modified",
            ChangeStatus::Deleted => "deleted",
        }
    }

    fn of(status: Delta) -> Self {
        match status {
            Delta::Added | Delta::Copied | Delta::Untracked => ChangeStatus::Added,
            Delta::Deleted => ChangeStatus::Deleted,
            // A rename or a type change is a file that is there and differs,
            // which is what a reader of the diff needs to know.
            _ => ChangeStatus::Modified,
        }
    }
}

/// One file's change between two revisions.
#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub status: ChangeStatus,
    /// Unified diff of this file alone.
    pub diff: String,
}

/// One file two branches changed in a way git cannot merge.
///
/// Each side is absent when that side has no such file: a file added on one
/// branch has no base, and a file one side deleted has no text there.
#[derive(Clone, Debug)]
pub struct FileConflict {
    pub path: String,
    pub base: Option<Vec<u8>>,
    /// The version on the branch the merge was made against, which is `main`.
    pub ours: Option<Vec<u8>>,
    /// The version on the transaction branch.
    pub theirs: Option<Vec<u8>>,
}

/// A squash commit that is written but which no reference names yet.
#[derive(Clone, Debug)]
pub struct PreparedLand {
    pub commit_oid: Oid,
    /// The head of `main` the merge was made against, which
    /// [`GitRepo::finish_land`] compares and swaps on.
    pub main_head: Oid,
    /// The files the land changes, as the write list that brings the working
    /// tree up to the merged revision.
    pub changed: FileWrites,
}

/// What one attempt at landing a branch produced.
#[derive(Clone, Debug)]
pub enum LandAttempt {
    Prepared(PreparedLand),
    Conflicts(Vec<FileConflict>),
}

/// The body of a squash commit: the branch it came from, who landed it, and the
/// title of every commit it holds, so the one commit still says what was in it.
fn land_message_body(branch: &BranchName, author: &str, titles: &[String]) -> String {
    let mut body = format!("branch: {branch}\nauthor: {author}\nsquashed:");
    for title in titles {
        body.push_str(&format!("\n- {title}"));
    }
    body
}

/// The path a merge conflict is about, from whichever of its three sides has a
/// file there.
fn conflict_path(conflict: &git2::IndexConflict) -> Result<String, GitError> {
    [&conflict.our, &conflict.their, &conflict.ancestor]
        .into_iter()
        .flatten()
        .map(|entry| String::from_utf8_lossy(&entry.path).into_owned())
        .next()
        .ok_or(GitError::NonUtf8Path)
}

/// The path a diff entry is about: where the file is now, or where it was when
/// the change removed it.
fn delta_path(delta: &DiffDelta<'_>) -> Option<String> {
    delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())
        .and_then(|path| path.to_str())
        .map(str::to_string)
}

// git2::Repository is Send; naming it here documents that callers may move a
// GitRepo into a blocking task and guard it with a mutex.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<GitRepo>();
};

/// The blob id of `path` in `tree`, or `None` if the tree has no such file.
fn blob_oid_at(tree: &Tree<'_>, path: &str) -> Option<Oid> {
    let entry = tree.get_path(Path::new(path)).ok()?;
    if entry.kind() == Some(ObjectType::Blob) {
        Some(entry.id())
    } else {
        None
    }
}

/// The index entry for a file the server writes. Timestamps and ownership are
/// left at zero: the tree only needs the path, the mode and the blob.
fn index_entry(path: &str, blob_oid: Oid, size: usize) -> IndexEntry {
    IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: REGULAR_FILE_MODE,
        uid: 0,
        gid: 0,
        file_size: u32::try_from(size).unwrap_or(u32::MAX),
        id: blob_oid,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    }
}

/// The signature recorded as both author and committer.
///
/// Authors here are session keys, `wiki`, or a name the frontend sends, none of
/// which is an email address, so one is synthesised. Characters git forbids in
/// a signature are replaced rather than rejected, because refusing a commit
/// over a space in a name would be worse than normalising it.
fn signature_for(author_name: &str) -> Result<Signature<'static>, git2::Error> {
    let name = sanitize_signature_part(author_name);
    let name = if name.is_empty() {
        AUTHOR_EMAIL_DOMAIN.to_string()
    } else {
        name
    };
    let local_part: String = name
        .chars()
        .map(|character| {
            if character.is_whitespace() {
                '-'
            } else {
                character
            }
        })
        .collect();
    Signature::now(&name, &format!("{local_part}@{AUTHOR_EMAIL_DOMAIN}"))
}

/// Drop the characters git refuses in a signature and trim the result.
fn sanitize_signature_part(text: &str) -> String {
    text.chars()
        .filter(|character| !matches!(character, '<' | '>' | '\n' | '\r' | '\0'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// A commit message: the title line, then a blank line and the body if there
/// is one.
fn compose_message(title: &str, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        format!("{title}\n")
    } else {
        format!("{title}\n\n{body}\n")
    }
}

fn commit_time(commit: &git2::Commit<'_>) -> DateTime<Utc> {
    Utc.timestamp_opt(commit.time().seconds(), 0)
        .single()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a commit message built without the blank line that separates
    /// the title from the body, which would fold the body into the title that
    /// the history page shows.
    #[test]
    fn a_message_body_is_separated_from_the_title_by_a_blank_line() {
        let message = compose_message("record the rule", "memory: widgets\nauthor: user");
        let mut lines = message.lines();
        assert_eq!(lines.next(), Some("record the rule"));
        assert_eq!(lines.next(), Some(""));
        assert_eq!(lines.next(), Some("memory: widgets"));
    }

    /// Detects a title-only message that grows a trailing blank line, which
    /// would show as an empty body in the history.
    #[test]
    fn a_message_without_a_body_is_just_the_title() {
        assert_eq!(compose_message("record the rule", ""), "record the rule\n");
    }

    /// Detects an author name that git refuses reaching the signature, which
    /// would fail every commit made by a frontend user with a space or an
    /// angle bracket in their name.
    #[test]
    fn an_author_name_with_forbidden_characters_still_makes_a_signature() {
        let signature = signature_for("Ada <Lovelace>").unwrap();
        assert_eq!(signature.name().unwrap(), "Ada Lovelace");
        assert_eq!(signature.email().unwrap(), "Ada-Lovelace@forgetmenot");
    }

    /// Detects an empty author name producing an invalid signature and failing
    /// the commit.
    #[test]
    fn an_empty_author_name_falls_back_to_a_usable_signature() {
        let signature = signature_for("   ").unwrap();
        assert_eq!(signature.name().unwrap(), "forgetmenot");
    }
}
