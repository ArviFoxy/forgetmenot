//! The branch operations: open a transaction, see what it holds, land it as one
//! commit, or drop it.
//!
//! A transaction is a git branch, so these are the four things an agent already
//! knows how to do with one. Nothing on a branch is served to any context: what
//! a context is delivered follows `main`, and a branch reaches `main` only by
//! landing, as a single commit whose title the lander writes.

use git2::Oid;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::service::LandError;
use crate::store::MemoryId;
use crate::store::branch::BranchName;

use super::{ConflictedFile, OperationError, ValidationMessage, iso8601};

/// A branch that was just opened.
#[derive(Clone, Debug, Serialize)]
pub struct BranchOpened {
    pub branch: BranchName,
}

/// One open branch, as the branch list reports it.
#[derive(Clone, Debug, Serialize)]
pub struct BranchRow {
    pub branch: BranchName,
    /// The session key that opened it; absent for a branch made by hand.
    pub owner: Option<String>,
    /// Commits the branch has that `main` does not.
    pub ahead: usize,
    /// Commits `main` has that the branch does not.
    pub behind: usize,
    /// ISO 8601, absent when nothing was recorded.
    pub created: Option<String>,
    pub last_activity: Option<String>,
}

/// One file a branch changes.
#[derive(Clone, Debug, Serialize)]
pub struct BranchFileDiff {
    pub path: String,
    /// `added`, `modified` or `deleted`.
    pub status: String,
    /// Unified diff of this file against the merge base with `main`.
    pub diff: String,
}

/// What a branch would change if it landed.
#[derive(Clone, Debug, Serialize)]
pub struct BranchDiff {
    pub branch: BranchName,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<BranchFileDiff>,
}

/// What a land produced.
#[derive(Clone, Debug, Serialize)]
pub struct BranchLanded {
    pub branch: BranchName,
    /// The one commit the branch became, on `main`.
    pub commit_oid: String,
}

/// A branch that was dropped with whatever it held.
#[derive(Clone, Debug, Serialize)]
pub struct BranchAbandoned {
    pub branch: BranchName,
}

/// Which session is opening a transaction.
#[derive(Clone, Debug, Deserialize)]
pub struct BranchCreateRequest {
    pub session_key: String,
}

/// The one commit a land makes.
#[derive(Clone, Debug, Deserialize)]
pub struct LandRequest {
    /// The commit's title line: what all the writes on the branch did.
    pub message: String,
    pub author: String,
}

/// Open a branch for one session, at the current head of `main`.
pub async fn branch_create(
    state: &AppState,
    session_key: &str,
) -> Result<BranchOpened, OperationError> {
    let branch = state.store.open_branch(session_key).await?;
    Ok(BranchOpened { branch })
}

/// Every open branch, with its owner and how far it is from `main`.
pub async fn branch_list(state: &AppState) -> Result<Vec<BranchRow>, OperationError> {
    Ok(state
        .store
        .branch_statuses()
        .await?
        .into_iter()
        .map(|status| BranchRow {
            branch: status.name,
            owner: status.record.owner,
            ahead: status.ahead,
            behind: status.behind,
            created: status.record.created.map(iso8601),
            last_activity: status.record.last_activity.map(iso8601),
        })
        .collect())
}

/// What one branch would change, per file, against the merge base with `main`.
pub async fn branch_diff(
    state: &AppState,
    branch: &BranchName,
) -> Result<BranchDiff, OperationError> {
    let changes = state
        .store
        .branch_changes(branch)
        .await?
        .ok_or_else(|| OperationError::UnknownBranch(branch.clone()))?;
    Ok(BranchDiff {
        branch: branch.clone(),
        ahead: changes.ahead,
        behind: changes.behind,
        files: changes
            .files
            .into_iter()
            .map(|change| BranchFileDiff {
                path: change.path,
                status: change.status.as_str().to_string(),
                diff: change.diff,
            })
            .collect(),
    })
}

/// Land one branch: every write on it becomes one commit on `main`.
///
/// A file the branch and `main` both changed incompatibly is reported as a
/// conflict with the three versions of it, and the branch is left as it is: the
/// caller resolves by writing the file it wants on the branch and landing again.
pub async fn branch_land(
    state: &AppState,
    branch: &BranchName,
    request: &LandRequest,
) -> Result<BranchLanded, OperationError> {
    match state
        .store
        .land_branch(branch, &request.author, &request.message)
        .await
    {
        Ok(commit_oid) => Ok(BranchLanded {
            branch: branch.clone(),
            commit_oid: commit_oid.to_string(),
        }),
        Err(LandError::BadMessage) => Err(OperationError::invalid(
            branch.as_str(),
            &LandError::BadMessage.to_string(),
        )),
        Err(LandError::NoSuchBranch(branch)) => Err(OperationError::UnknownBranch(branch)),
        Err(error @ LandError::NothingToLand(_)) => {
            Err(OperationError::invalid(branch.as_str(), &error.to_string()))
        }
        Err(LandError::Conflicts(conflicts)) => Err(OperationError::MergeConflicts {
            conflicts: conflicts.iter().map(ConflictedFile::of).collect(),
        }),
        Err(LandError::Invalid(errors)) => Err(OperationError::Invalid {
            errors: errors.iter().map(ValidationMessage::of).collect(),
        }),
        Err(LandError::Busy) => Err(OperationError::Busy),
        Err(LandError::Store(error)) => Err(OperationError::Store(error)),
    }
}

/// The memories one landed commit changed against `main` as it was before the
/// land, which is what the landing session holds once the branch has landed.
///
/// A file in the commit that is no memory, a scope or the settings, is left out:
/// only memories are delivered to a context. A commit whose files cannot be read
/// yields nothing, because the land itself has happened and the most this costs
/// is a memory delivered back to the session that wrote it.
pub async fn landed_memories(state: &AppState, landed: &BranchLanded) -> Vec<MemoryId> {
    let commit_oid = match Oid::from_str(&landed.commit_oid) {
        Ok(commit_oid) => commit_oid,
        Err(error) => {
            tracing::warn!(
                commit = %landed.commit_oid,
                %error,
                "the landed commit is not a revision, so its files are not read"
            );
            return Vec::new();
        }
    };
    match state.store.commit_changed_paths(commit_oid).await {
        Ok(paths) => paths
            .iter()
            .filter_map(|path| MemoryId::from_repository_path(path))
            .collect(),
        Err(error) => {
            tracing::warn!(
                commit = %commit_oid,
                %error,
                "the files a landed commit changed could not be read, so the landing session may \
                 be delivered its own writes"
            );
            Vec::new()
        }
    }
}

/// Delete one branch with whatever it holds; `main` is untouched.
pub async fn branch_abandon(
    state: &AppState,
    branch: &BranchName,
) -> Result<BranchAbandoned, OperationError> {
    if state.store.abandon_branch(branch).await? {
        Ok(BranchAbandoned {
            branch: branch.clone(),
        })
    } else {
        Err(OperationError::UnknownBranch(branch.clone()))
    }
}
