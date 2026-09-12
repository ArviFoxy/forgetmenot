//! The store's git repository.
//!
//! `GitRepo` is `Send` but not `Sync`; callers keep it behind a
//! `std::sync::Mutex` so that read HEAD, build the commit and move the branch
//! reference happen without another writer in between. The reference move is
//! itself a compare-and-swap, so a commit a person makes by hand between two
//! server operations makes the server's update fail instead of discarding it.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use git2::build::CheckoutBuilder;
use git2::{
    DiffFormat, DiffOptions, ErrorCode, Index, IndexEntry, IndexTime, ObjectType, Oid, Repository,
    RepositoryInitOptions, Signature, Sort, Tree,
};

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
}

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

        // An index of its own, not the repository's, so that building the tree
        // cannot disturb what is staged in the working tree.
        let mut tree_index = Index::new()?;
        tree_index.read_tree(&parent_tree)?;
        let mut blob_oids = BTreeMap::new();
        for (path, content) in &files {
            match content {
                Some(bytes) => {
                    let blob_oid = self.repository.blob(bytes)?;
                    tree_index.add(&index_entry(path, blob_oid, bytes.len()))?;
                    blob_oids.insert(path.clone(), blob_oid);
                }
                None => {
                    if tree_index.get_path(Path::new(path), 0).is_some() {
                        tree_index.remove_path(Path::new(path))?;
                    }
                }
            }
        }
        let tree_oid = tree_index.write_tree_to(&self.repository)?;
        let tree = self.repository.find_tree(tree_oid)?;

        let signature = signature_for(author_name)?;
        let message = compose_message(message_title, message_body);
        // The commit object is written without moving any reference, so a
        // failed compare-and-swap below leaves the branch exactly as it was.
        let commit_oid = self.repository.commit(
            None,
            &signature,
            &signature,
            &message,
            &tree,
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
