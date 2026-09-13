//! Transaction branches: the name a transaction lives under, and what the
//! branch's own commits say about it.
//!
//! A transaction is a git branch and nothing else. There is no bookkeeping
//! anywhere else: the name carries the machine and the session, the commit that
//! opened the branch carries the owner's session key as a trailer and its time
//! is when the branch was opened, the newest commit's time is the last activity,
//! and the open branches are the refs under the prefix. A restart therefore
//! rediscovers every transaction from the repository, with no state file in it.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Prefix of every branch a transaction lives on.
///
/// Only branches under it are created, listed, written to, landed or deleted,
/// so `main` and any branch a person made by hand cannot be reached by these
/// operations.
pub const BRANCH_PREFIX: &str = "tx/";

/// The longest a branch name may be, so that a machine or session id nobody
/// meant as a name cannot produce a ref no tool can show.
pub const MAX_BRANCH_NAME: usize = 120;

/// The name of one transaction branch, checked to be one.
///
/// The form is `tx/<name>` with `<name>` a single segment of letters, digits,
/// `.`, `-` and `_`. One segment, so the name is two path segments in a URL and
/// one subsection in the git config, with no escaping anywhere.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct BranchName(String);

/// Why a name is not a transaction branch's name.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "a branch is named `{BRANCH_PREFIX}<name>`, where <name> is letters, digits, `.`, `-` or `_` \
     and at most {MAX_BRANCH_NAME} characters, not `{0}`"
)]
pub struct BranchNameError(String);

impl BranchName {
    /// The name of the branch a client asked about, or why it is not one.
    pub fn parse(name: &str) -> Result<Self, BranchNameError> {
        let invalid = || BranchNameError(name.to_string());
        let Some(segment) = name.strip_prefix(BRANCH_PREFIX) else {
            return Err(invalid());
        };
        if segment.is_empty() || name.len() > MAX_BRANCH_NAME {
            return Err(invalid());
        }
        if segment.starts_with('.') || segment.starts_with('-') || segment.ends_with('.') {
            return Err(invalid());
        }
        let allowed = segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        });
        if !allowed {
            return Err(invalid());
        }
        Ok(Self(name.to_string()))
    }

    /// The name a transaction of `session_key` takes, numbered `counter`.
    ///
    /// The session key is not a ref name: it holds a `/` and may hold anything
    /// else a harness put in a session id, so every character a ref name cannot
    /// carry becomes `-`. The counter keeps one session's branches apart.
    pub fn for_session(session_key: &str, counter: u32) -> Self {
        let stem: String = session_key
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        let stem = stem.trim_matches(['-', '.']).to_string();
        let stem = if stem.is_empty() {
            "session".to_string()
        } else {
            stem
        };
        // Trimmed so that a long session id cannot push the name past the
        // limit; the counter and the prefix always fit.
        let suffix = format!("-{counter}");
        let room = MAX_BRANCH_NAME - BRANCH_PREFIX.len() - suffix.len();
        let stem: String = stem.chars().take(room).collect();
        Self(format!("{BRANCH_PREFIX}{stem}{suffix}"))
    }

    /// The text form, which is the branch name git knows.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The part after the prefix, which names the branch's config subsection.
    pub fn segment(&self) -> &str {
        self.0
            .strip_prefix(BRANCH_PREFIX)
            .expect("a branch name carries the prefix")
    }

    /// The full reference this branch is.
    pub fn reference(&self) -> String {
        format!("refs/heads/{}", self.0)
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The trailer the commit that opens a branch carries, naming the session that
/// opened it. A trailer, because that is where git itself keeps this kind of
/// fact and `git log` shows it without any tool of ours.
pub const OWNER_TRAILER: &str = "Forgetmenot-Owner:";

/// What a branch's own commits say about it.
///
/// Every field is optional because a branch made by hand carries none of this,
/// and such a branch is still a branch that can be landed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchRecord {
    /// The session key that opened the branch, from the trailer of its first
    /// commit.
    pub owner: Option<String>,
    /// When the branch was opened, which is when its first commit was made.
    pub created: Option<DateTime<Utc>>,
    /// When the branch was last written to, which is when its newest commit was
    /// made; the retention window is measured from this.
    pub last_activity: Option<DateTime<Utc>>,
    /// The titles of the commits on the branch that changed a file, oldest
    /// first. The commit that opened the branch changes nothing and is not one
    /// of them, so this is the branch's writes.
    pub titles: Vec<String>,
}

impl BranchRecord {
    /// How many writes the branch holds, which is what it is ahead of `main` by.
    pub fn writes(&self) -> usize {
        self.titles.len()
    }
}

/// The message of the commit that opens a branch: what it is, and who opened it.
pub fn opening_message(branch: &BranchName, owner: &str) -> (String, String) {
    (
        format!("open the transaction {branch}"),
        format!("{OWNER_TRAILER} {owner}"),
    )
}

/// The owner a commit message names, if it carries the trailer.
pub fn owner_in_message(message: &str) -> Option<String> {
    message.lines().find_map(|line| {
        line.strip_prefix(OWNER_TRAILER)
            .map(|owner| owner.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a name check that lets through what is not a transaction
    /// branch: `main` itself, a branch outside the prefix, or a second path
    /// segment, each of which would let a land or a delete reach a ref no
    /// transaction owns.
    #[test]
    fn only_single_segment_names_under_the_prefix_are_branch_names() {
        assert!(BranchName::parse("tx/alpha-session-1-1").is_ok());
        assert!(BranchName::parse("tx/a_b.c-1").is_ok());
        assert!(BranchName::parse("main").is_err());
        assert!(BranchName::parse("tx/").is_err());
        assert!(BranchName::parse("tx/alpha/session").is_err());
        assert!(BranchName::parse("tx/../main").is_err());
        assert!(BranchName::parse("tx/-leading").is_err());
        assert!(BranchName::parse("refs/heads/tx/one").is_err());
        assert!(BranchName::parse(&format!("tx/{}", "a".repeat(MAX_BRANCH_NAME))).is_err());
    }

    /// Detects a generated name that keeps the session key's `/` or any other
    /// character a ref cannot hold, which would make every branch_create fail.
    #[test]
    fn a_generated_name_is_a_valid_branch_name_for_any_session_key() {
        for key in [
            "alpha/session-1",
            "alpha/session-1/agent-7",
            "a machine/a session:with punctuation",
            "///",
        ] {
            let name = BranchName::for_session(key, 3);
            assert_eq!(
                BranchName::parse(name.as_str()),
                Ok(name.clone()),
                "the name generated for {key:?} must be a branch name, got {name}"
            );
            assert!(
                name.as_str().ends_with("-3"),
                "the counter must be part of the name, got {name}"
            );
        }
    }

    /// Detects an owner trailer that cannot be read back out of the commit
    /// message, which would leave every branch without an owner and make the
    /// branch list unable to say whose transaction it is.
    #[test]
    fn the_owner_of_a_branch_is_read_back_from_its_opening_commit_message() {
        let branch = BranchName::for_session("alpha/session-1", 1);
        let (title, body) = opening_message(&branch, "alpha/session-1");
        let message = format!("{title}\n\n{body}\n");
        assert_eq!(
            owner_in_message(&message),
            Some("alpha/session-1".to_string())
        );
        assert_eq!(owner_in_message("a commit with no trailer\n"), None);
    }
}
