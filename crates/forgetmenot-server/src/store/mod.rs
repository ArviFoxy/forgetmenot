//! The store: a git repository of scope and memory files, and the parsed
//! catalog built from one revision of it.
//!
//! Paths inside the repository:
//!
//! ```text
//! config.yml
//! scopes/<id>.yaml
//! memories/<name>.md
//! memories/sessions/<machine>/<session-id>/<name>.md
//! ```

pub mod branch;
pub mod catalog;
pub mod frontmatter;
pub mod git;
pub mod memory;
pub mod scope;
pub mod settings;
pub mod validate;

use std::fmt;

use serde::{Deserialize, Serialize};

/// Directory prefix of every memory file.
pub const MEMORY_PREFIX: &str = "memories/";
/// Suffix of every memory file.
pub const MEMORY_SUFFIX: &str = ".md";
/// Directory prefix of every scope file.
pub const SCOPE_PREFIX: &str = "scopes/";
/// Suffix of every scope file.
pub const SCOPE_SUFFIX: &str = ".yaml";
/// Directory under `memories/` holding the per-session silos.
pub const SESSION_DIRECTORY: &str = "sessions";
/// Prefix of the memory id a scope's `message` is delivered as.
pub const SCOPE_MESSAGE_PREFIX: &str = "scopes/";

/// Identifier of a memory: its path under `memories/` without the `.md`
/// suffix, so `git-rules` or `sessions/alpha/session-1/notes`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(String);

impl MemoryId {
    /// A memory id from its text form.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The text form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The id of the memory stored at `path`, or `None` if `path` is not a
    /// memory file.
    pub fn from_repository_path(path: &str) -> Option<Self> {
        let relative = path.strip_prefix(MEMORY_PREFIX)?;
        let without_suffix = relative.strip_suffix(MEMORY_SUFFIX)?;
        if without_suffix.is_empty() {
            return None;
        }
        Some(Self(without_suffix.to_string()))
    }

    /// The path of this memory's file inside the repository.
    pub fn repository_path(&self) -> String {
        format!("{MEMORY_PREFIX}{}{MEMORY_SUFFIX}", self.0)
    }

    /// The last path segment, which the frontmatter `name` must equal.
    pub fn name(&self) -> &str {
        match self.0.rsplit_once('/') {
            Some((_, name)) => name,
            None => &self.0,
        }
    }

    /// Whether this memory lives in a per-session silo.
    pub fn is_session(&self) -> bool {
        self.session_silo().is_some()
    }

    /// The silo directory this memory lives in,
    /// `sessions/<machine>/<session-id>`, or `None` outside a silo.
    ///
    /// A path with fewer than four segments is not a memory in a silo: the
    /// machine, the session id and the memory name all have to be present.
    pub fn session_silo(&self) -> Option<&str> {
        let mut segments = self.0.split('/');
        if segments.next()? != SESSION_DIRECTORY {
            return None;
        }
        let machine = segments.next()?;
        let session = segments.next()?;
        if machine.is_empty() || session.is_empty() || segments.next().is_none() {
            return None;
        }
        let silo_length = SESSION_DIRECTORY.len() + 1 + machine.len() + 1 + session.len();
        Some(&self.0[..silo_length])
    }

    /// The session this memory belongs to as `<machine>/<session-id>`.
    pub fn session_key(&self) -> Option<&str> {
        let silo = self.session_silo()?;
        silo.strip_prefix(SESSION_DIRECTORY)
            .and_then(|rest| rest.strip_prefix('/'))
    }

    /// The id of a memory named `name` inside this memory's silo.
    pub fn sibling_in_silo(&self, name: &str) -> Option<Self> {
        let silo = self.session_silo()?;
        Some(Self(format!("{silo}/{name}")))
    }

    /// The id the message of `scope` is delivered under.
    ///
    /// The one place this id is built, so that the catalog, the renderer and the
    /// statistics all name a scope's message the same way.
    pub fn for_scope_message(scope: &ScopeId) -> Self {
        Self(format!("{SCOPE_MESSAGE_PREFIX}{scope}"))
    }

    /// The scope whose message this id names, or `None` for any other id.
    ///
    /// A scope id has no slash in it, so an id with one after the prefix names
    /// something deeper under `memories/scopes/` instead.
    pub fn scope_message_of(&self) -> Option<ScopeId> {
        let scope = self.0.strip_prefix(SCOPE_MESSAGE_PREFIX)?;
        if scope.is_empty() || scope.contains('/') {
            return None;
        }
        Some(ScopeId::new(scope))
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a scope id names, decided by its form alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Global,
    Machine,
    Session,
    File,
}

/// Identifier of a scope.
///
/// Most scopes have a file at `scopes/<id>.yaml` and an id matching
/// [`is_valid_scope_file_id`]. Three families are implicit and have no file:
/// `global`, `machine:<name>` and `session:<machine>/<session-id>`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeId(String);

impl ScopeId {
    /// A scope id from its text form.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The text form.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The scope every context starts in.
    pub fn global() -> Self {
        Self("global".to_string())
    }

    /// The scope of one machine.
    pub fn machine(machine: &str) -> Self {
        Self(format!("machine:{machine}"))
    }

    /// The scope of one session on one machine.
    pub fn session(machine: &str, session_id: &str) -> Self {
        Self(format!("session:{machine}/{session_id}"))
    }

    /// Whether this is a session scope.
    pub fn is_session(&self) -> bool {
        self.session_key().is_some()
    }

    /// The session as `<machine>/<session-id>`, or `None` for other scopes.
    pub fn session_key(&self) -> Option<&str> {
        let key = self.0.strip_prefix("session:")?;
        let (machine, session_id) = key.split_once('/')?;
        if machine.is_empty() || session_id.is_empty() || session_id.contains('/') {
            return None;
        }
        Some(key)
    }

    /// What this id names.
    ///
    /// An id that is not one of the three implicit forms is the id of a scope
    /// that could exist only by a file, a malformed `machine:a/b` among them.
    pub fn kind(&self) -> ScopeKind {
        if self.0 == "global" {
            return ScopeKind::Global;
        }
        if let Some(machine) = self.0.strip_prefix("machine:")
            && !machine.is_empty()
            && !machine.contains('/')
        {
            return ScopeKind::Machine;
        }
        if self.is_session() {
            return ScopeKind::Session;
        }
        ScopeKind::File
    }

    /// Whether this scope exists without a file: `global`, a well-formed
    /// `machine:<name>` or a well-formed `session:<machine>/<session-id>`.
    pub fn is_implicit(&self) -> bool {
        self.kind() != ScopeKind::File
    }

    /// The path of this scope's file inside the repository.
    pub fn repository_path(&self) -> String {
        format!("{SCOPE_PREFIX}{}{SCOPE_SUFFIX}", self.0)
    }

    /// The id of the scope stored at `path`, or `None` if `path` is not a
    /// scope file.
    pub fn from_repository_path(path: &str) -> Option<Self> {
        let relative = path.strip_prefix(SCOPE_PREFIX)?;
        let without_suffix = relative.strip_suffix(SCOPE_SUFFIX)?;
        if without_suffix.is_empty() || without_suffix.contains('/') {
            return None;
        }
        Some(Self(without_suffix.to_string()))
    }
}

impl fmt::Display for ScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Whether `id` matches the pattern required of a scope with a file,
/// `^[a-z0-9][a-z0-9-]*$`.
///
/// Written out rather than compiled as a regex because it runs on every scope
/// file of every catalog load and the pattern is fixed.
pub fn is_valid_scope_file_id(id: &str) -> bool {
    let mut characters = id.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    characters.all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a silo prefix computed with the wrong segment count, which
    /// would make the session silo rules apply to the wrong memories.
    #[test]
    fn session_memory_ids_expose_their_silo_and_session_key() {
        let inside = MemoryId::new("sessions/alpha/session-1/notes");
        assert_eq!(inside.session_silo(), Some("sessions/alpha/session-1"));
        assert_eq!(inside.session_key(), Some("alpha/session-1"));
        assert_eq!(inside.name(), "notes");
    }

    /// Detects treating any path under `sessions/` as a silo memory: a file
    /// directly in `sessions/<machine>/` names no session.
    #[test]
    fn a_path_with_too_few_segments_is_not_in_a_silo() {
        assert_eq!(MemoryId::new("sessions/alpha/notes").session_silo(), None);
        assert_eq!(MemoryId::new("notes").session_silo(), None);
    }

    /// Detects a round trip between a memory id and its file path that loses
    /// or duplicates the prefix or the suffix.
    #[test]
    fn memory_ids_round_trip_through_their_repository_path() {
        for id in ["git-rules", "sessions/alpha/session-1/notes"] {
            let id = MemoryId::new(id);
            assert_eq!(
                MemoryId::from_repository_path(&id.repository_path()).as_ref(),
                Some(&id)
            );
        }
    }

    /// Detects a scope message id that does not round trip, and an id of a
    /// memory deeper under `memories/scopes/` read as a scope's message: the
    /// renderer prints a message without its memory id and would then print a
    /// file memory that way too.
    ///
    /// Source: issue #7, where a scope's message is delivered as a critical
    /// memory of the scope whose id no memory file can have.
    #[test]
    fn a_scope_message_id_names_its_scope_and_nothing_else_does() {
        let scope = ScopeId::new("broad-except");
        let id = MemoryId::for_scope_message(&scope);
        assert_eq!(id.as_str(), "scopes/broad-except");
        assert_eq!(id.scope_message_of(), Some(scope));
        assert_eq!(MemoryId::new("scopes/alpha/notes").scope_message_of(), None);
        assert_eq!(MemoryId::new("scopes").scope_message_of(), None);
        assert_eq!(MemoryId::new("widget-naming").scope_message_of(), None);
    }

    /// Detects accepting a file-backed scope id that the id pattern forbids,
    /// which would let a scope file sit at a path no id can name.
    #[test]
    fn the_scope_id_pattern_rejects_ids_it_should() {
        assert!(is_valid_scope_file_id("widgets"));
        assert!(is_valid_scope_file_id("a1-b2"));
        assert!(!is_valid_scope_file_id(""));
        assert!(!is_valid_scope_file_id("-leading-dash"));
        assert!(!is_valid_scope_file_id("Upper"));
        assert!(!is_valid_scope_file_id("has_underscore"));
        assert!(!is_valid_scope_file_id("machine:alpha"));
    }

    /// Detects an implicit-scope test that accepts malformed forms, which
    /// would silence the unknown-scope validation for typos like
    /// `session:alpha` with no session id.
    #[test]
    fn only_well_formed_implicit_scopes_are_recognised() {
        assert!(ScopeId::global().is_implicit());
        assert!(ScopeId::machine("alpha").is_implicit());
        assert!(ScopeId::session("alpha", "session-1").is_implicit());
        assert!(!ScopeId::new("session:alpha").is_implicit());
        assert!(!ScopeId::new("machine:").is_implicit());
        assert!(!ScopeId::new("widgets").is_implicit());
    }

    /// Detects an id read as the wrong kind, which would put a scope in the
    /// wrong family of the index: a malformed prefix taken for a machine or a
    /// session names a scope that could exist only by a file.
    ///
    /// Source: the rule that the kind of a scope id follows from its form.
    #[test]
    fn a_scope_id_names_its_kind_by_its_form() {
        assert_eq!(ScopeId::global().kind(), ScopeKind::Global);
        assert_eq!(ScopeId::machine("alpha").kind(), ScopeKind::Machine);
        assert_eq!(
            ScopeId::session("alpha", "session-1").kind(),
            ScopeKind::Session
        );
        assert_eq!(ScopeId::new("widgets").kind(), ScopeKind::File);
        assert_eq!(ScopeId::new("machine:a/b").kind(), ScopeKind::File);
        assert_eq!(ScopeId::new("session:alpha").kind(), ScopeKind::File);
        assert_eq!(ScopeId::new("machine:").kind(), ScopeKind::File);
    }
}
