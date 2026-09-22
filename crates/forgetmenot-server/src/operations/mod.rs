//! The operations on memories, scopes and sessions.
//!
//! This is the one place where "an edit is a commit" and "a session's scopes
//! are a set of flags" are implemented. The JSON API and the MCP tools are thin
//! adapters over the functions here, so a rule enforced here holds for both:
//! there is no second write path that could validate less or forget to stamp
//! `modified`.
//!
//! Writes are optimistic: the caller sends the version it read, the store
//! commits compare-and-swap against it, and a caller whose version is no longer
//! current gets the current document back rather than overwriting it.
//!
//! Every write takes an optional branch. Without one it commits to `main` and
//! takes effect at once; with one it commits to that branch, is checked and
//! validated against that branch, and reaches no context until the branch is
//! landed. The branch operations themselves are in [`branches`].

pub mod branches;
pub mod settings;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, SubsecRound, Utc};
use git2::Oid;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::context::registry::{ContextRecord, ContextRegistry, Inheritance, RegistrySnapshot};
use crate::context::{ContextKey, ContextState, PreviousTexts, compute_needs};
use crate::render::{self, Delivery};
use crate::service::{self, StoreError, WriteError, is_valid_message_title};
use crate::stats::{StatsError, ToolCallRecord, tokens_of};
use crate::store::branch::{BranchName, BranchNameError};
use crate::store::catalog::{Catalog, MemoryEntry, ScopeEntry};
use crate::store::frontmatter::FrontmatterError;
use crate::store::git::{CommitSummary, FileConflict, GitError, GitRepo};
use crate::store::memory::{
    MemoryDocument, MemoryFrontmatter, MemoryKind, MemoryMetadata, MemorySource, rewrite_links,
};
use crate::store::scope::{Forget, ScopeDocument, Trigger, TriggerField};
use crate::store::validate::{
    self, Candidate, CrossDocumentRules, ValidationError, ValidationWarning, WriteMode,
};
use crate::store::{MemoryId, ScopeId, ScopeKind};

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------

/// Why an operation did not happen.
///
/// The three failures a caller acts on are separate variants: a document that
/// is not there, a version that is no longer current (which carries the
/// document as it is now, so an editor can show both), and a document the store
/// rules refuse (which carries one message per problem).
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error("{0} does not exist")]
    NotFound(String),

    #[error("{what} has changed since it was read")]
    Conflict {
        what: String,
        current: CurrentDocument,
    },

    #[error("the write was refused: {} problem(s) in the document", errors.len())]
    Invalid { errors: Vec<ValidationMessage> },

    /// The store moved under the write twice in a row, so the caller may simply
    /// try again; nothing was written.
    #[error("the store is being written by someone else; the write was not made")]
    Busy,

    #[error("`{0}` is not a scope in this store")]
    UnknownScope(ScopeId),

    /// The call to turn scopes on or off carried an empty list, so it named
    /// nothing to change and cannot be read as a change of anything.
    #[error("the list names no scope to change")]
    EmptyScopeList,

    #[error("`{0}` is always on for a session and cannot be turned off")]
    ImplicitScope(ScopeId),

    #[error("`{0}` is implicit and has no file, so there is nothing to delete")]
    ImplicitScopeHasNoFile(ScopeId),

    /// The scope named exists but keeps nothing in a file, so the document the
    /// caller asked for is not there. Asking is reasonable, which is why this
    /// is answered the way a missing document is and not the way a request the
    /// server cannot read is.
    #[error("the scope `{0}` has no file to read")]
    ScopeHasNoFile(ScopeId),

    #[error("no session `{0}` has been seen by this server")]
    UnknownSession(String),

    #[error("no context `{0}` has been seen by this server")]
    UnknownContext(String),

    /// The call named a branch that is not open. A write is never quietly
    /// redirected to `main`: the caller meant the transaction.
    #[error("there is no branch `{0}`")]
    UnknownBranch(BranchName),

    #[error(transparent)]
    BadBranchName(#[from] BranchNameError),

    /// The branch and `main` changed the same lines of the same files, so the
    /// land was refused; the branch is left as it is, for the caller to
    /// overwrite the file on it and land again.
    #[error("the branch cannot be landed: {} file(s) changed on both sides", conflicts.len())]
    MergeConflicts { conflicts: Vec<ConflictedFile> },

    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Stats(#[from] StatsError),
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("the document could not be written out: {0}")]
    Render(#[from] FrontmatterError),
}

impl OperationError {
    fn missing_memory(id: &MemoryId) -> Self {
        OperationError::NotFound(format!("the memory `{id}`"))
    }

    fn missing_scope(id: &ScopeId) -> Self {
        OperationError::NotFound(format!("the scope `{id}`"))
    }

    /// One refusal about one path, used for the problems that are not
    /// [`ValidationError`]s: a missing or unusable commit message, or a
    /// `base_version` that is not a version.
    pub(crate) fn invalid(path: &str, message: &str) -> Self {
        OperationError::Invalid {
            errors: vec![ValidationMessage {
                path: path.to_string(),
                message: message.to_string(),
            }],
        }
    }
}

/// One problem with a document, as the review page and the edit form show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationMessage {
    /// The file the problem is in.
    pub path: String,
    pub message: String,
}

impl ValidationMessage {
    pub(crate) fn of(error: &ValidationError) -> Self {
        Self {
            path: error.path().to_string(),
            message: error.to_string(),
        }
    }

    pub(crate) fn of_warning(warning: &ValidationWarning) -> Self {
        Self {
            path: warning.path().to_string(),
            message: warning.to_string(),
        }
    }
}

/// One file a land could not merge, with the three versions of it a resolution
/// needs.
#[derive(Clone, Debug, Serialize)]
pub struct ConflictedFile {
    pub path: String,
    /// The text where the two sides last agreed; absent when the file was added
    /// on both sides.
    pub base: Option<String>,
    /// The text on `main`; absent when `main` deleted the file.
    pub ours: Option<String>,
    /// The text on the branch; absent when the branch deleted the file.
    pub theirs: Option<String>,
}

impl ConflictedFile {
    pub(crate) fn of(conflict: &FileConflict) -> Self {
        let text = |bytes: &Option<Vec<u8>>| {
            bytes
                .as_ref()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        };
        Self {
            path: conflict.path.clone(),
            base: text(&conflict.base),
            ours: text(&conflict.ours),
            theirs: text(&conflict.theirs),
        }
    }
}

/// The document a conflict carries: whichever kind the caller was writing.
///
/// Serialized without a tag, because the caller knows which kind it asked for
/// and the frontend reads the body of a 409 as the document it was editing.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum CurrentDocument {
    /// Every variant is boxed: every operation's failure carries this enum, and
    /// a whole document is far larger than the answer it travels with.
    Memory(Box<MemoryDoc>),
    Scope(Box<ScopeDoc>),
    Settings(Box<settings::SettingsDoc>),
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// One line of the memory index.
#[derive(Clone, Debug, Serialize)]
pub struct MemorySummary {
    pub id: MemoryId,
    pub name: String,
    pub title: String,
    pub description: String,
    pub kind: MemoryKind,
    pub scope: ScopeId,
    pub source: MemorySource,
    /// ISO 8601, or absent when the file carries no `modified` stamp.
    pub modified: Option<String>,
    /// The blob id of the file, which a write sends back as `base_version`.
    pub version: String,
}

/// One commit, as the history pages show it.
#[derive(Clone, Debug, Serialize)]
pub struct Commit {
    pub oid: String,
    /// ISO 8601.
    pub time: String,
    pub author: String,
    /// The commit message's first line.
    pub title: String,
}

impl Commit {
    fn of(summary: &CommitSummary) -> Self {
        Self {
            oid: summary.oid.to_string(),
            time: iso8601(summary.time),
            author: summary.author.clone(),
            title: summary.title.clone(),
        }
    }
}

/// A whole memory.
#[derive(Clone, Debug, Serialize)]
pub struct MemoryDoc {
    pub id: MemoryId,
    pub name: String,
    /// The body's first level-1 heading, else the name; derived, not writable.
    pub title: String,
    pub description: String,
    pub kind: MemoryKind,
    pub scope: ScopeId,
    pub source: MemorySource,
    /// The `metadata` keys this server does not interpret, Claude Code's own
    /// `type` among them, as the file carries them.
    pub metadata: serde_json::Map<String, serde_json::Value>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub author: Option<String>,
    pub body: String,
    pub version: String,
    /// The memories this one links to, resolved against the store.
    pub links: Vec<MemoryId>,
    /// The memories that link to this one.
    pub backlinks: Vec<MemoryId>,
    /// The newest commit that changed this file.
    pub last_commit: Option<Commit>,
}

/// A whole scope.
#[derive(Clone, Debug, Serialize)]
pub struct ScopeDoc {
    pub id: ScopeId,
    /// The text this scope delivers whenever it is active, `null` when it
    /// delivers none.
    pub message: Option<String>,
    pub implies: Vec<ScopeId>,
    pub triggers: Vec<Trigger>,
    /// When the scope turns itself off in a context, `null` when it stays on
    /// until the agent turns it off.
    pub forget: Option<Forget>,
    pub version: String,
}

impl ScopeDoc {
    fn of(entry: &ScopeEntry) -> Self {
        Self {
            id: entry.id.clone(),
            message: entry.document.message.clone(),
            implies: entry.document.implies.clone(),
            triggers: entry.document.triggers.clone(),
            forget: entry.document.forget,
            version: entry.version.to_string(),
        }
    }
}

/// One scope that exists, in the index.
#[derive(Clone, Debug, Serialize)]
pub struct ScopeRow {
    pub id: ScopeId,
    pub kind: ScopeKind,
    /// What to call a session, from its context, the same name the contexts
    /// page shows; absent for the other kinds and for a session with no live
    /// context.
    pub name: Option<String>,
    /// The scope's file, for the `file` kind: what a client edits and deletes.
    pub file: Option<ScopeDoc>,
}

impl ScopeRow {
    /// The row of a scope with no file, whose kind its id decides.
    fn of(id: ScopeId) -> Self {
        Self {
            kind: id.kind(),
            id,
            name: None,
            file: None,
        }
    }
}

/// One line of a memory's file with the commit that last changed it.
#[derive(Clone, Debug, Serialize)]
pub struct BlameLine {
    /// The line's number in the file, counting from one.
    pub line: usize,
    /// The commit that last changed the line.
    pub oid: String,
    /// ISO 8601.
    pub time: String,
    pub author: String,
    /// The line itself, without its newline.
    pub text: String,
}

impl BlameLine {
    fn of(line: &crate::store::git::BlameLine) -> Self {
        Self {
            line: line.line,
            oid: line.oid.clone(),
            time: iso8601(line.time),
            author: line.author.clone(),
            text: line.text.clone(),
        }
    }
}

/// One memory's file, line by line, with the commit each line came from.
#[derive(Clone, Debug, Serialize)]
pub struct MemoryBlame {
    pub id: MemoryId,
    /// The blob id of the file the lines were read from.
    pub version: String,
    pub lines: Vec<BlameLine>,
}

/// One commit of one document, with the file as it was and what changed.
#[derive(Clone, Debug, Serialize)]
pub struct HistoryEntry {
    pub commit: Commit,
    /// The file's whole content at that commit.
    pub content: String,
    /// Unified diff against the commit's first parent.
    pub diff: String,
}

/// One page of the store's commits, newest first.
#[derive(Clone, Debug, Serialize)]
pub struct StoreHistory {
    pub commits: Vec<Commit>,
    /// The `before` that reads the page after this one, `null` when the history
    /// ends here.
    pub next_before: Option<String>,
}

/// One file a commit changed.
#[derive(Clone, Debug, Serialize)]
pub struct CommitFile {
    pub path: String,
    /// `added`, `modified` or `deleted`.
    pub status: String,
    /// Unified diff of this file alone.
    pub diff: String,
    /// The memory the file carries, `null` when it carries none. Resolved here so
    /// that a reader links to the document without reading paths.
    pub memory_id: Option<MemoryId>,
    /// The scope the file carries, `null` when it carries none.
    pub scope_id: Option<ScopeId>,
}

/// One commit with every file it changed.
#[derive(Clone, Debug, Serialize)]
pub struct CommitFiles {
    pub commit: Commit,
    pub files: Vec<CommitFile>,
}

/// One live context.
#[derive(Clone, Debug, Serialize)]
pub struct ContextRow {
    /// The session key, the same form the MCP tools take.
    pub key: String,
    /// What to call this context in a list, derived by [`context_name`]. Empty
    /// when nothing is known about the context but its key.
    pub name: String,
    /// The name the user gave the session with `/rename`, absent when the
    /// session was never named.
    pub title: Option<String>,
    /// The user's first prompt in the session, as the client cut it.
    pub first_prompt: Option<String>,
    /// The key of the context this one runs inside, for a subagent, in the
    /// same form as `key`. Absent for a session's own context.
    pub parent: Option<String>,
    /// The task a subagent was given, absent for a session's own context.
    pub task: Option<String>,
    /// The kind of subagent this is, for example `general-purpose`, absent for
    /// a session's own context.
    pub agent_type: Option<String>,
    pub active_scopes: Vec<ScopeId>,
    pub delivered_count: usize,
    /// ISO 8601.
    pub last_seen: String,
}

/// Which text of a context is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    /// What the context's next hook event would deliver.
    Due,
    /// Everything the context's active scopes hold, as if nothing had been
    /// delivered into it yet.
    All,
}

/// The text a context would be given, rendered without delivering it.
#[derive(Clone, Debug, Serialize)]
pub struct ContextPrompt {
    /// The session key, the same form the MCP tools take.
    pub key: String,
    /// What to call this context in a heading, derived by [`context_name`].
    pub name: String,
    pub mode: PromptMode,
    /// The rendered text, empty when the mode has nothing to deliver.
    pub text: String,
    pub bytes: u64,
    /// What the text costs the model, as [`tokens_of`] counts it from the
    /// store's `characters_per_token`.
    pub tokens: u64,
}

/// What the review page reports.
#[derive(Clone, Debug, Serialize)]
pub struct ReviewReport {
    pub errors: Vec<ValidationMessage>,
    /// What is worth seeing but does not make the store invalid: a file that is
    /// not a memory, a memory with no index entry, a memory whose file names
    /// several scopes.
    pub warnings: Vec<ValidationMessage>,
    /// Critical memories whose scope is `global`: they are delivered in full
    /// to every session on every machine, which is worth a second look.
    pub global_only_critical: Vec<MemoryId>,
}

/// What a successful write produced.
#[derive(Clone, Debug, Serialize)]
pub struct WriteOutcome {
    pub commit_oid: String,
    /// The new version of the document, which the editor keeps as its
    /// `base_version` for the next write; absent when the write removed the
    /// file, which leaves no version to write against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// One trigger that matched in a trigger test.
#[derive(Clone, Debug, Serialize)]
pub struct TriggerMatch {
    pub scope_id: ScopeId,
    pub field: TriggerField,
    pub pattern: String,
}

/// The result of a trigger test.
#[derive(Clone, Debug, Serialize)]
pub struct TriggerTestResult {
    pub fired: Vec<TriggerMatch>,
}

/// A text to match against the store's triggers.
#[derive(Clone, Debug, Deserialize)]
pub struct TriggerTestRequest {
    /// Which of the hook's texts the text stands for, or `any` to match it
    /// against every trigger in the store.
    pub field: TriggerField,
    pub text: String,
    /// The machine the text is supposed to come from, which decides whether a
    /// machine-qualified trigger applies.
    pub machine: String,
}

/// A pattern to check on its own, before it is saved anywhere.
#[derive(Clone, Debug, Deserialize)]
pub struct PatternRequest {
    pub pattern: String,
}

/// Whether a pattern compiles, with the reason it does not when it does not.
#[derive(Clone, Debug, Serialize)]
pub struct PatternValidity {
    pub ok: bool,
    /// The `regex` crate's own message, absent when the pattern compiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The machines the server knows of.
#[derive(Clone, Debug, Serialize)]
pub struct MachineList {
    pub machines: Vec<String>,
}

/// The scopes of one context, and the ones it could turn on.
#[derive(Clone, Debug, Serialize)]
pub struct SessionScopes {
    pub active: Vec<ScopeId>,
    /// File-backed scopes that are not active; the implicit scopes are not
    /// offered because they are on by construction.
    pub available: Vec<ScopeId>,
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Which memories an index request wants.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MemoryFilter {
    pub scope: Option<ScopeId>,
    pub kind: Option<MemoryKind>,
}

impl MemoryFilter {
    fn matches(&self, entry: &MemoryEntry) -> bool {
        if let Some(kind) = self.kind
            && entry.kind() != kind
        {
            return false;
        }
        if let Some(scope) = &self.scope
            && entry.scope() != scope
        {
            return false;
        }
        true
    }
}

/// A write of one memory.
#[derive(Clone, Debug, Deserialize)]
pub struct MemoryWriteRequest {
    pub description: String,
    pub kind: MemoryKind,
    pub scope: ScopeId,
    pub source: MemorySource,
    /// The `metadata` keys this server does not interpret, which replace the
    /// ones the memory carries. Absent leaves them as they are, so a caller
    /// that knows nothing about them cannot delete them by not sending them.
    #[serde(default)]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    pub body: String,
    /// The version the caller read. Required when replacing a document, absent
    /// when creating one.
    #[serde(default)]
    pub base_version: Option<String>,
    /// Who to record as the commit's author: a session key, `wiki`, or a name
    /// the frontend sends.
    pub author: String,
    /// The commit's title line.
    pub message: String,
}

/// A memory to create, which needs the id the new file goes to.
#[derive(Clone, Debug, Deserialize)]
pub struct MemoryCreateRequest {
    pub id: MemoryId,
    #[serde(flatten)]
    pub write: MemoryWriteRequest,
}

/// One snippet of one memory's body to replace.
#[derive(Clone, Debug, Deserialize)]
pub struct ReplaceTextRequest {
    /// The text to replace, matched exactly and taken from the body as it is.
    pub old_string: String,
    pub new_string: String,
    /// Whether every occurrence is replaced; without it a snippet that appears
    /// more than once is refused.
    #[serde(default)]
    pub replace_all: bool,
    /// The version the caller read; the current one when absent.
    #[serde(default)]
    pub base_version: Option<String>,
    pub author: String,
    pub message: String,
}

/// The frontmatter fields of one memory to set. A field that is absent is left
/// exactly as it is, and the body is never touched.
#[derive(Clone, Debug, Deserialize)]
pub struct SetFieldsRequest {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub kind: Option<MemoryKind>,
    #[serde(default)]
    pub scope: Option<ScopeId>,
    #[serde(default)]
    pub source: Option<MemorySource>,
    /// The `metadata` keys this server does not interpret, merged into the ones
    /// the memory carries: a key given is added or replaced, a key given as
    /// `null` is removed, and a key not given keeps its value.
    #[serde(default)]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    /// The version the caller read; the current one when absent.
    #[serde(default)]
    pub base_version: Option<String>,
    pub author: String,
    pub message: String,
}

impl SetFieldsRequest {
    /// Whether the request would change no field, which is a write worth
    /// refusing rather than a commit that says nothing.
    fn sets_nothing(&self) -> bool {
        self.description.is_none()
            && self.kind.is_none()
            && self.scope.is_none()
            && self.source.is_none()
            && self.metadata.as_ref().is_none_or(serde_json::Map::is_empty)
    }
}

/// Where one memory is moved to.
#[derive(Clone, Debug, Deserialize)]
pub struct RenameRequest {
    pub to: MemoryId,
    /// The version the caller read; the current one when absent.
    #[serde(default)]
    pub base_version: Option<String>,
    pub author: String,
    pub message: String,
}

/// A deletion, which removes the file and so carries no content.
///
/// One type for memories and scopes: a deletion says which version it removes
/// and nothing about what kind of document that version is.
#[derive(Clone, Debug, Deserialize)]
pub struct DeleteRequest {
    pub base_version: String,
    pub author: String,
    pub message: String,
}

/// A write of one scope.
#[derive(Clone, Debug, Deserialize)]
pub struct ScopeWriteRequest {
    pub implies: Vec<ScopeId>,
    pub triggers: Vec<Trigger>,
    /// The scope's own `message`, the text it delivers whenever it is active.
    /// Named apart from `message`, which is this write's commit title.
    #[serde(default)]
    pub scope_message: Option<String>,
    /// When the scope turns itself off in a context; absent when it stays on
    /// until the agent turns it off.
    #[serde(default)]
    pub forget: Option<Forget>,
    #[serde(default)]
    pub base_version: Option<String>,
    pub author: String,
    pub message: String,
}

/// A scope to create, which needs the id the new file goes to.
#[derive(Clone, Debug, Deserialize)]
pub struct ScopeCreateRequest {
    pub id: ScopeId,
    #[serde(flatten)]
    pub write: ScopeWriteRequest,
}

/// Which kind of document a history request is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Memory,
    Scope,
}

impl DocumentKind {
    /// The file the document with this id lives in.
    fn repository_path(self, id: &str) -> String {
        match self {
            DocumentKind::Memory => MemoryId::new(id).repository_path(),
            DocumentKind::Scope => ScopeId::new(id).repository_path(),
        }
    }

    fn describe(self, id: &str) -> String {
        match self {
            DocumentKind::Memory => format!("the memory `{id}`"),
            DocumentKind::Scope => format!("the scope `{id}`"),
        }
    }
}

/// When a write to this target answers the rules that read other files.
///
/// A write to a branch answers them at the land, where the merged tree is whole;
/// a write to `main` answers them now, because the commit it makes is a revision
/// sessions read.
fn cross_document_rules(branch: Option<&BranchName>) -> CrossDocumentRules {
    match branch {
        Some(_) => CrossDocumentRules::DeferredToLanding,
        None => CrossDocumentRules::CheckedNow,
    }
}

// ---------------------------------------------------------------------------
// Memories
// ---------------------------------------------------------------------------

/// The memory index, ordered by id.
pub fn memory_index(catalog: &Catalog, filter: &MemoryFilter) -> Vec<MemorySummary> {
    catalog
        .memories()
        .filter(|entry| filter.matches(entry))
        .map(memory_summary)
        .collect()
}

/// One whole memory.
///
/// With a context key the memory is recorded as held by that context at the
/// version fetched, because that is what fetching it is: the model has the body,
/// and delivering it again at the next event would repeat what it just read.
/// The API passes `None`; the `memory_get` tool passes the calling session.
pub async fn memory_get(
    state: &AppState,
    id: &MemoryId,
    context: Option<&ContextKey>,
) -> Result<MemoryDoc, OperationError> {
    let catalog = state.store.snapshot().await?;
    let entry = catalog
        .memory(id)
        .ok_or_else(|| OperationError::missing_memory(id))?;
    let document = memory_doc(state, &catalog, entry).await?;

    if let Some(key) = context {
        let now = state.clock.now();
        state
            .contexts
            .with_context(key, now, Inheritance::of(catalog.settings()), |context| {
                // No hook event accompanies a fetch, so the context size at this
                // moment is unknown; staleness is judged again from the next
                // event that does carry one.
                context.note_shown(entry, None);
                context.last_seen = now;
            })
            .await;
    }
    Ok(document)
}

/// Record in `key`'s context that it wrote `ids` itself: a memory in the catalog
/// after the write is noted at that version, one that is gone is forgotten, so
/// neither is delivered back to its own writer.
///
/// The writer holds what it wrote, so repeating it at the next event would only
/// send back the text the session just composed. Every other context, including
/// a subagent of the writing session, keeps its own record and is delivered the
/// change as usual.
pub async fn note_own_writes(state: &AppState, key: &ContextKey, ids: &[MemoryId]) {
    // The catalog of `main` after the commit, so the version noted is the one
    // every other context is compared against.
    let catalog = match state.store.snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(
                %key,
                %error,
                "the store could not be read after a write, so the writing session may be \
                 delivered its own write"
            );
            return;
        }
    };
    let now = state.clock.now();
    state
        .contexts
        .with_context(key, now, Inheritance::of(catalog.settings()), |context| {
            for id in ids {
                match catalog.memory(id) {
                    Some(entry) => context.note_shown(entry, None),
                    // Gone from the store, so there is nothing to withdraw from
                    // the context that removed it.
                    None => {
                        context.delivered.remove(id);
                    }
                }
            }
            context.last_seen = now;
        })
        .await;
}

/// Write one memory: create it or replace the version the caller read.
pub async fn memory_put(
    state: &AppState,
    id: &MemoryId,
    request: &MemoryWriteRequest,
    mode: WriteMode,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    let catalog = target_catalog(state, branch).await?;
    let path = id.repository_path();
    let existing = catalog.memory(id);

    let expected = match mode {
        WriteMode::Create => {
            // A create that finds the id taken is the same situation as a stale
            // version: the caller's picture of the store is out of date, and the
            // answer is the document that is there.
            if let Some(entry) = existing {
                return Err(memory_conflict(state, &catalog, entry).await);
            }
            None
        }
        WriteMode::Update => {
            let Some(entry) = existing else {
                return Err(OperationError::missing_memory(id));
            };
            let Some(base_version) = request.base_version.as_deref() else {
                return Err(missing_base_version(&path, entry.version));
            };
            match Oid::from_str(base_version) {
                Ok(version) if version == entry.version => Some(version),
                Ok(_) => return Err(memory_conflict(state, &catalog, entry).await),
                Err(_) => return Err(OperationError::invalid(&path, UNREADABLE_BASE_VERSION)),
            }
        }
    };

    // Stamped to the second: these timestamps are read by people in a text file,
    // and the nanoseconds a clock offers say nothing about when an edit was made.
    let now = state.clock.now().trunc_subsecs(0);
    let mut document = match existing {
        // Started from the file as it is, so keys neither this server nor the
        // form knows about survive the write.
        Some(entry) => entry.document.clone(),
        None => empty_memory(id, now),
    };
    document.frontmatter.description = Some(request.description.clone());
    document.frontmatter.modified = Some(now);
    document.frontmatter.metadata.kind = Some(request.kind);
    document.frontmatter.metadata.scope = Some(request.scope.clone());
    document.frontmatter.metadata.source = Some(request.source.clone());
    document.frontmatter.metadata.author = Some(request.author.clone());
    if let Some(metadata) = &request.metadata {
        document
            .frontmatter
            .metadata
            .replace_extra(metadata)
            .map_err(|error| OperationError::invalid(&path, &error.to_string()))?;
    }
    document.body = with_final_newline(&request.body);

    commit_memory(
        state,
        &catalog,
        document,
        mode,
        expected,
        &request.author,
        &request.message,
        branch,
    )
    .await
}

/// Delete one memory: the file leaves the store in one commit, so it stops
/// being delivered anywhere, and the history keeps every version it had.
///
/// A memory other memories link to may be deleted: the links that no longer
/// resolve are what the review report lists, and refusing the deletion would
/// only keep a memory nobody wants in force.
pub async fn memory_delete(
    state: &AppState,
    id: &MemoryId,
    request: &DeleteRequest,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    let catalog = target_catalog(state, branch).await?;
    let path = id.repository_path();
    let Some(entry) = catalog.memory(id) else {
        return Err(OperationError::missing_memory(id));
    };
    let expected = match Oid::from_str(&request.base_version) {
        Ok(version) if version == entry.version => version,
        Ok(_) => return Err(memory_conflict(state, &catalog, entry).await),
        Err(_) => return Err(OperationError::invalid(&path, UNREADABLE_BASE_VERSION)),
    };

    if !is_valid_message_title(&request.message) {
        return Err(OperationError::invalid(
            &path,
            &WriteError::BadMessage.to_string(),
        ));
    }

    let outcome = commit_to(
        state,
        branch,
        &request.author,
        &request.message,
        &commit_body("memory", id.as_str(), &request.author),
        // No content: the commit removes the file.
        vec![(path.clone(), None)],
        vec![(path.clone(), Some(expected))],
    )
    .await;

    match outcome {
        Ok(outcome) => Ok(write_outcome(&outcome, &path)),
        Err(WriteError::VersionMismatch { .. }) => {
            // The store moved between the version check above and the commit,
            // so the answer is the document as it is now.
            let catalog = target_catalog(state, branch).await?;
            match catalog.memory(id) {
                Some(entry) => Err(memory_conflict(state, &catalog, entry).await),
                None => Err(OperationError::missing_memory(id)),
            }
        }
        Err(error) => Err(write_error(&path, error)),
    }
}

/// Replace one exact snippet of one memory's body.
///
/// The snippet is matched exactly, not as a pattern: a caller that read the body
/// can name a line of it without sending the whole document back, which is what
/// a model editing one rule of a long memory wants. A snippet that is not there,
/// or that is there more than once without `replace_all`, is refused rather than
/// guessed at. Nothing but the body, `modified` and `author` changes.
pub async fn memory_replace_text(
    state: &AppState,
    id: &MemoryId,
    request: &ReplaceTextRequest,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    let catalog = target_catalog(state, branch).await?;
    let path = id.repository_path();
    let Some(entry) = catalog.memory(id) else {
        return Err(OperationError::missing_memory(id));
    };
    let expected =
        expected_memory_version(state, &catalog, entry, request.base_version.as_deref()).await?;

    if request.old_string.is_empty() {
        return Err(OperationError::invalid(&path, EMPTY_SNIPPET));
    }
    let body = &entry.document.body;
    let occurrences = body.matches(&request.old_string).count();
    match occurrences {
        0 => return Err(OperationError::invalid(&path, SNIPPET_NOT_FOUND)),
        1 => {}
        several if !request.replace_all => {
            return Err(OperationError::invalid(
                &path,
                &format!(
                    "old_string appears {several} times in this memory's body; \
                     send replace_all to replace every one of them"
                ),
            ));
        }
        _ => {}
    }
    let rewritten = if request.replace_all {
        body.replace(&request.old_string, &request.new_string)
    } else {
        body.replacen(&request.old_string, &request.new_string, 1)
    };

    let now = state.clock.now().trunc_subsecs(0);
    let mut document = entry.document.clone();
    document.body = with_final_newline(&rewritten);
    document.frontmatter.modified = Some(now);
    document.frontmatter.metadata.author = Some(request.author.clone());
    commit_memory(
        state,
        &catalog,
        document,
        WriteMode::Update,
        Some(expected),
        &request.author,
        &request.message,
        branch,
    )
    .await
}

/// Set some of one memory's frontmatter fields, leaving the body alone.
///
/// Only the fields the request carries change, so a caller that wants to move a
/// memory to another scope does not have to send the body back and cannot
/// truncate it by forgetting to.
pub async fn memory_set_fields(
    state: &AppState,
    id: &MemoryId,
    request: &SetFieldsRequest,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    let catalog = target_catalog(state, branch).await?;
    let path = id.repository_path();
    let Some(entry) = catalog.memory(id) else {
        return Err(OperationError::missing_memory(id));
    };
    let expected =
        expected_memory_version(state, &catalog, entry, request.base_version.as_deref()).await?;
    if request.sets_nothing() {
        return Err(OperationError::invalid(&path, NO_FIELDS_TO_SET));
    }

    let now = state.clock.now().trunc_subsecs(0);
    // Started from the file as it is, and the body is not touched at all, so the
    // bytes after the frontmatter are the same bytes.
    let mut document = entry.document.clone();
    if let Some(description) = &request.description {
        document.frontmatter.description = Some(description.clone());
    }
    if let Some(kind) = request.kind {
        document.frontmatter.metadata.kind = Some(kind);
    }
    if let Some(scope) = &request.scope {
        document.frontmatter.metadata.scope = Some(scope.clone());
    }
    if let Some(source) = &request.source {
        document.frontmatter.metadata.source = Some(source.clone());
    }
    if let Some(metadata) = &request.metadata {
        document
            .frontmatter
            .metadata
            .merge_extra(metadata)
            .map_err(|error| OperationError::invalid(&path, &error.to_string()))?;
    }
    document.frontmatter.modified = Some(now);
    document.frontmatter.metadata.author = Some(request.author.clone());
    commit_memory(
        state,
        &catalog,
        document,
        WriteMode::Update,
        Some(expected),
        &request.author,
        &request.message,
        branch,
    )
    .await
}

/// Move one memory to another id, rewriting every link to it in the same commit.
///
/// One commit, because a store in which the file has moved and the links have
/// not is a store the validator calls invalid: no revision of the store may show
/// the rename half done.
pub async fn memory_rename(
    state: &AppState,
    from: &MemoryId,
    request: &RenameRequest,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    let catalog = target_catalog(state, branch).await?;
    let to = &request.to;
    let from_path = from.repository_path();
    let to_path = to.repository_path();
    let Some(entry) = catalog.memory(from) else {
        return Err(OperationError::missing_memory(from));
    };
    let expected =
        expected_memory_version(state, &catalog, entry, request.base_version.as_deref()).await?;
    if to == from {
        return Err(OperationError::invalid(&to_path, SAME_RENAME_TARGET));
    }

    let now = state.clock.now().trunc_subsecs(0);
    let mut moved = entry.document.clone();
    moved.id = to.clone();
    // The declared name is the last segment of the id, so moving the file
    // changes it; a rename that left it behind would be an invalid store.
    moved.frontmatter.name = to.name().to_string();
    moved.frontmatter.modified = Some(now);
    moved.frontmatter.metadata.author = Some(request.author.clone());

    let report = validate::validate_write(
        &catalog,
        Candidate::Memory { document: &moved },
        WriteMode::Create,
        cross_document_rules(branch),
    );
    let mut errors: Vec<ValidationMessage> =
        report.errors().iter().map(ValidationMessage::of).collect();
    if !is_valid_message_title(&request.message) {
        errors.push(ValidationMessage {
            path: to_path.clone(),
            message: WriteError::BadMessage.to_string(),
        });
    }

    let mut files = vec![
        (to_path.clone(), Some(moved.render()?.into_bytes())),
        // The old file leaves the store in the same commit the new one arrives
        // in, so no revision holds both.
        (from_path.clone(), None),
    ];
    let mut expected_versions = vec![(to_path.clone(), None), (from_path.clone(), Some(expected))];
    for linker in catalog.memories().filter(|other| &other.id != from) {
        let targets: Vec<String> = linker
            .links
            .iter()
            .filter(|target| {
                validate::resolve_link(&catalog, &linker.id, target).as_ref() == Some(from)
            })
            .cloned()
            .collect();
        if targets.is_empty() {
            continue;
        }
        // Rewriting the link would make a memory outside a silo link into it,
        // which is the one thing a silo forbids, so the rename is refused
        // instead of being made and reported invalid afterwards.
        if let Some(silo) = to.session_silo()
            && linker.id.session_silo() != Some(silo)
        {
            errors.push(ValidationMessage {
                path: linker.path.clone(),
                message: format!(
                    "links to `{from}`, which the rename would move into session silo `{silo}`; \
                     only memories in that silo may link to it"
                ),
            });
            continue;
        }
        let replacement = link_text_for(&linker.id, to);
        let mut document = linker.document.clone();
        for target in &targets {
            document.body = rewrite_links(&document.body, target, &replacement);
        }
        files.push((linker.path.clone(), Some(document.render()?.into_bytes())));
        expected_versions.push((linker.path.clone(), Some(linker.version)));
    }

    if !errors.is_empty() {
        return Err(OperationError::Invalid { errors });
    }

    let outcome = commit_to(
        state,
        branch,
        &request.author,
        &request.message,
        &format!(
            "memory: {to}\nrenamed-from: {from}\nauthor: {}",
            request.author
        ),
        files,
        expected_versions,
    )
    .await;

    match outcome {
        Ok(outcome) => Ok(write_outcome(&outcome, &to_path)),
        Err(WriteError::VersionMismatch { path, .. }) => {
            // A file the rename touches moved under it: the answer is the
            // document the caller asked about as the store has it now.
            let catalog = target_catalog(state, branch).await?;
            match catalog.memory(from) {
                Some(entry) => Err(memory_conflict(state, &catalog, entry).await),
                None => Err(OperationError::invalid(
                    &path,
                    "a memory the rename touches changed while it was being made",
                )),
            }
        }
        Err(error) => Err(write_error(&to_path, error)),
    }
}

/// The text a link from `linker` to `target` is written as: a bare name inside
/// the silo they share, because that is how a session's notes link to each
/// other, and the full id everywhere else.
fn link_text_for(linker: &MemoryId, target: &MemoryId) -> String {
    match target.session_silo() {
        Some(silo) if linker.session_silo() == Some(silo) => target.name().to_string(),
        _ => target.as_str().to_string(),
    }
}

/// The version a memory write replaces: the one the caller read, or the one the
/// store holds when the caller sent none, which is what the operations with an
/// optional `base_version` do.
async fn expected_memory_version(
    state: &AppState,
    catalog: &Catalog,
    entry: &MemoryEntry,
    base_version: Option<&str>,
) -> Result<Oid, OperationError> {
    match base_version {
        None => Ok(entry.version),
        Some(text) => match Oid::from_str(text) {
            Ok(version) if version == entry.version => Ok(version),
            Ok(_) => Err(memory_conflict(state, catalog, entry).await),
            Err(_) => Err(OperationError::invalid(
                &entry.path,
                UNREADABLE_BASE_VERSION,
            )),
        },
    }
}

/// Validate one memory and commit it, which is the part every memory write
/// shares.
#[allow(clippy::too_many_arguments)]
async fn commit_memory(
    state: &AppState,
    catalog: &Catalog,
    document: MemoryDocument,
    mode: WriteMode,
    // The version the write replaces, `None` when it creates the file.
    expected: Option<Oid>,
    author: &str,
    message: &str,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    let id = document.id.clone();
    let path = id.repository_path();
    let report = validate::validate_write(
        catalog,
        Candidate::Memory {
            document: &document,
        },
        mode,
        cross_document_rules(branch),
    );
    let mut errors: Vec<ValidationMessage> =
        report.errors().iter().map(ValidationMessage::of).collect();
    if !is_valid_message_title(message) {
        errors.push(ValidationMessage {
            path: path.clone(),
            message: WriteError::BadMessage.to_string(),
        });
    }
    if !errors.is_empty() {
        return Err(OperationError::Invalid { errors });
    }

    let bytes = document.render()?.into_bytes();
    let outcome = commit_to(
        state,
        branch,
        author,
        message,
        &commit_body("memory", id.as_str(), author),
        vec![(path.clone(), Some(bytes))],
        vec![(path.clone(), expected)],
    )
    .await;

    match outcome {
        Ok(outcome) => Ok(write_outcome(&outcome, &path)),
        Err(WriteError::VersionMismatch { .. }) => {
            // The store moved between the version check above and the commit,
            // so the answer is the document as it is now.
            let catalog = target_catalog(state, branch).await?;
            match catalog.memory(&id) {
                Some(entry) => Err(memory_conflict(state, &catalog, entry).await),
                None => Err(OperationError::missing_memory(&id)),
            }
        }
        Err(error) => Err(write_error(&path, error)),
    }
}

/// The conflict answer for one memory: the document as the store has it.
async fn memory_conflict(
    state: &AppState,
    catalog: &Catalog,
    entry: &MemoryEntry,
) -> OperationError {
    match memory_doc(state, catalog, entry).await {
        Ok(document) => OperationError::Conflict {
            what: format!("the memory `{}`", entry.id),
            current: CurrentDocument::Memory(Box::new(document)),
        },
        // The conflict is real whatever the history read did, so the failure to
        // read the history is reported instead of being hidden behind it.
        Err(error) => error,
    }
}

fn memory_summary(entry: &MemoryEntry) -> MemorySummary {
    MemorySummary {
        id: entry.id.clone(),
        name: entry.document.name().to_string(),
        title: entry.document.title().to_string(),
        description: entry.document.description().to_string(),
        kind: entry.kind(),
        scope: entry.scope().clone(),
        source: entry.document.source(),
        modified: entry.document.modified().map(iso8601),
        version: entry.version.to_string(),
    }
}

async fn memory_doc(
    state: &AppState,
    catalog: &Catalog,
    entry: &MemoryEntry,
) -> Result<MemoryDoc, OperationError> {
    let last_commit = newest_commit(state, &entry.path).await?;
    Ok(MemoryDoc {
        id: entry.id.clone(),
        name: entry.document.name().to_string(),
        title: entry.document.title().to_string(),
        description: entry.document.description().to_string(),
        kind: entry.kind(),
        scope: entry.scope().clone(),
        source: entry.document.source(),
        metadata: entry.document.frontmatter.metadata.extra_as_json(),
        created: entry.document.created().map(iso8601),
        modified: entry.document.modified().map(iso8601),
        author: entry.document.author().map(str::to_string),
        body: entry.document.body.clone(),
        version: entry.version.to_string(),
        links: resolved_links(catalog, entry),
        backlinks: backlinks(catalog, &entry.id),
        last_commit,
    })
}

/// The memories this one links to, in the order the body names them, each named
/// once. A `[[target]]` that resolves to nothing is left out: the review page
/// reports those, and a link to nothing is not a link.
fn resolved_links(catalog: &Catalog, entry: &MemoryEntry) -> Vec<MemoryId> {
    let mut links = Vec::new();
    for target in &entry.links {
        if let Some(resolved) = validate::resolve_link(catalog, &entry.id, target)
            && !links.contains(&resolved)
        {
            links.push(resolved);
        }
    }
    links
}

/// The memories whose body links to `id`, by id.
fn backlinks(catalog: &Catalog, id: &MemoryId) -> Vec<MemoryId> {
    catalog
        .memories()
        .filter(|entry| &entry.id != id)
        .filter(|entry| {
            entry.links.iter().any(|target| {
                validate::resolve_link(catalog, &entry.id, target).as_ref() == Some(id)
            })
        })
        .map(|entry| entry.id.clone())
        .collect()
}

/// A memory file with nothing in it yet: the name the id requires, and the
/// creation time, which is the only field a later write does not set again.
fn empty_memory(id: &MemoryId, now: DateTime<Utc>) -> MemoryDocument {
    MemoryDocument {
        id: id.clone(),
        frontmatter: MemoryFrontmatter {
            name: id.name().to_string(),
            description: None,
            modified: None,
            extra: yaml_serde::Mapping::new(),
            metadata: MemoryMetadata {
                created: Some(now),
                ..MemoryMetadata::default()
            },
        },
        body: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Scopes
// ---------------------------------------------------------------------------

/// Every scope that exists, ordered by id.
///
/// A scope with a file exists by its file; `global` exists always; a machine or
/// a session scope exists once the server has seen that machine or that
/// session, or a store file names it. A reference alone creates no scope of the
/// file kind, so a memory's `scope`, a scope's `implies` and a context's active
/// set may all name a scope that is not here.
///
/// One catalog snapshot and one registry snapshot answer the whole index, so
/// every row comes from the same store revision and the same set of contexts.
pub async fn scope_index(state: &AppState) -> Result<Vec<ScopeRow>, OperationError> {
    let catalog = state.store.snapshot().await?;
    let recorded = crate::stats::read(&state.stats, &state.config.stats_path, |reader| {
        reader.machines()
    })
    .await?;
    let contexts = state.contexts.snapshot(state.clock.now()).await;

    let mut rows: BTreeMap<ScopeId, ScopeRow> = BTreeMap::new();
    for entry in catalog.scopes() {
        rows.insert(
            entry.id.clone(),
            ScopeRow {
                kind: entry.id.kind(),
                id: entry.id.clone(),
                name: None,
                file: Some(ScopeDoc::of(entry)),
            },
        );
    }
    note_scope(&mut rows, ScopeId::global());
    for machine in machine_names(&contexts, recorded) {
        note_scope(&mut rows, ScopeId::machine(&machine));
    }
    for record in &contexts.contexts {
        // A subagent works in its session's scope, so it adds no scope of its
        // own, and the name is the session's, as the contexts page shows it.
        if record.key.is_subagent() {
            continue;
        }
        // A context with nothing to name it by has no name, rather than an
        // empty one.
        let name = Some(context_name(record)).filter(|name| !name.is_empty());
        note_scope(&mut rows, record.key.session_scope()).name = name;
    }
    // A session with a silo has notes in the store, whether or not any context
    // of it is live.
    for memory in catalog.memories() {
        if let Some((machine, session_id)) =
            memory.id.session_key().and_then(|key| key.split_once('/'))
        {
            note_scope(&mut rows, ScopeId::session(machine, session_id));
        }
    }
    // A machine or a session a store file names is one the store has a record
    // of, which is what makes it exist; a file-kind id is a reference to a
    // scope file, and a file is the only thing that can create that scope.
    let named = catalog
        .memories()
        .map(|memory| memory.scope())
        .chain(catalog.scopes().flat_map(|entry| &entry.document.implies));
    for id in named {
        match id.kind() {
            ScopeKind::Machine | ScopeKind::Session => {
                note_scope(&mut rows, id.clone());
            }
            ScopeKind::Global | ScopeKind::File => {}
        }
    }
    Ok(rows.into_values().collect())
}

/// The row of `id`, added to the index when it is not there yet.
fn note_scope(rows: &mut BTreeMap<ScopeId, ScopeRow>, id: ScopeId) -> &mut ScopeRow {
    rows.entry(id.clone()).or_insert_with(|| ScopeRow::of(id))
}

/// One scope's file.
///
/// `global`, `machine:<name>` and `session:<machine>/<session-id>` exist without
/// a file, so the refusal says the file is what is missing rather than the
/// scope: the scope does exist, and there is nothing about it to read or edit.
/// The request itself is a reasonable one, so it is answered as a document that
/// is not there and not as a request the server cannot read.
pub fn scope_get(catalog: &Catalog, id: &ScopeId) -> Result<ScopeDoc, OperationError> {
    if id.is_implicit() {
        return Err(OperationError::ScopeHasNoFile(id.clone()));
    }
    catalog
        .scope(id)
        .map(ScopeDoc::of)
        .ok_or_else(|| OperationError::missing_scope(id))
}

/// Write one scope: create it or replace the version the caller read.
pub async fn scope_put(
    state: &AppState,
    id: &ScopeId,
    request: &ScopeWriteRequest,
    mode: WriteMode,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    let catalog = target_catalog(state, branch).await?;
    let path = id.repository_path();
    let existing = catalog.scope(id);

    let expected = match mode {
        WriteMode::Create => {
            if let Some(entry) = existing {
                return Err(scope_conflict(entry));
            }
            None
        }
        WriteMode::Update => {
            let Some(entry) = existing else {
                return Err(OperationError::missing_scope(id));
            };
            let Some(base_version) = request.base_version.as_deref() else {
                return Err(missing_base_version(&path, entry.version));
            };
            match Oid::from_str(base_version) {
                Ok(version) if version == entry.version => Some(version),
                Ok(_) => return Err(scope_conflict(entry)),
                Err(_) => return Err(OperationError::invalid(&path, UNREADABLE_BASE_VERSION)),
            }
        }
    };

    let document = ScopeDocument {
        id: id.clone(),
        message: request.scope_message.clone(),
        implies: request.implies.clone(),
        triggers: request.triggers.clone(),
        forget: request.forget,
    };
    let report = validate::validate_write(
        &catalog,
        Candidate::Scope {
            id,
            document: &document,
        },
        mode,
        cross_document_rules(branch),
    );
    let mut errors: Vec<ValidationMessage> =
        report.errors().iter().map(ValidationMessage::of).collect();
    if !is_valid_message_title(&request.message) {
        errors.push(ValidationMessage {
            path: path.clone(),
            message: WriteError::BadMessage.to_string(),
        });
    }
    if !errors.is_empty() {
        return Err(OperationError::Invalid { errors });
    }

    let bytes = document.render()?.into_bytes();
    let outcome = commit_to(
        state,
        branch,
        &request.author,
        &request.message,
        &commit_body("scope", id.as_str(), &request.author),
        vec![(path.clone(), Some(bytes))],
        vec![(path.clone(), expected)],
    )
    .await;

    match outcome {
        Ok(outcome) => Ok(write_outcome(&outcome, &path)),
        Err(WriteError::VersionMismatch { .. }) => {
            let catalog = target_catalog(state, branch).await?;
            match catalog.scope(id) {
                Some(entry) => Err(scope_conflict(entry)),
                None => Err(OperationError::missing_scope(id)),
            }
        }
        Err(error) => Err(write_error(&path, error)),
    }
}

/// Delete one scope: the file leaves the store in one commit, and the history
/// keeps every version it had.
///
/// The implicit scopes have no file and cannot be deleted. A scope any memory
/// is still in, or any other scope still implies, is refused with one problem
/// per file that names it: deleting it would leave those files naming a scope
/// that does not exist, which is a store `forgetmenot check` calls invalid, so
/// the caller edits them first.
///
/// A context that has the scope on keeps it: a scope is a flag, and the flag now
/// matches no memory. The memories delivered under it are reported as withdrawn
/// at the next event, because they are no longer due.
pub async fn scope_delete(
    state: &AppState,
    id: &ScopeId,
    request: &DeleteRequest,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    // Checked before the store is read: `global`, `machine:<name>` and
    // `session:<machine>/<session-id>` exist without a file, so "not found" would
    // be the wrong answer about them.
    if id.is_implicit() {
        return Err(OperationError::ImplicitScopeHasNoFile(id.clone()));
    }
    let catalog = target_catalog(state, branch).await?;
    let path = id.repository_path();
    let Some(entry) = catalog.scope(id) else {
        return Err(OperationError::missing_scope(id));
    };
    let expected = match Oid::from_str(&request.base_version) {
        Ok(version) if version == entry.version => version,
        Ok(_) => return Err(scope_conflict(entry)),
        Err(_) => return Err(OperationError::invalid(&path, UNREADABLE_BASE_VERSION)),
    };

    let mut errors = scope_references(&catalog, id);
    if !is_valid_message_title(&request.message) {
        errors.push(ValidationMessage {
            path: path.clone(),
            message: WriteError::BadMessage.to_string(),
        });
    }
    if !errors.is_empty() {
        return Err(OperationError::Invalid { errors });
    }

    let outcome = commit_to(
        state,
        branch,
        &request.author,
        &request.message,
        &commit_body("scope", id.as_str(), &request.author),
        // No content: the commit removes the file.
        vec![(path.clone(), None)],
        vec![(path.clone(), Some(expected))],
    )
    .await;

    match outcome {
        Ok(outcome) => Ok(write_outcome(&outcome, &path)),
        Err(WriteError::VersionMismatch { .. }) => {
            // The store moved between the version check above and the commit, so
            // the answer is the document as it is now.
            let catalog = target_catalog(state, branch).await?;
            match catalog.scope(id) {
                Some(entry) => Err(scope_conflict(entry)),
                None => Err(OperationError::missing_scope(id)),
            }
        }
        Err(error) => Err(write_error(&path, error)),
    }
}

/// Every file that would be left naming `id` if its scope file were removed, as
/// one refusal each, in the order a report reads them: the memories first, then
/// the scopes.
///
/// The scope's own file is not counted: a scope that implies itself would
/// otherwise block its own deletion, and after the deletion nothing is left to
/// name anything.
fn scope_references(catalog: &Catalog, id: &ScopeId) -> Vec<ValidationMessage> {
    let mut errors = Vec::new();
    for memory in catalog.memories() {
        if memory.scope() == id {
            errors.push(ValidationMessage {
                path: memory.path.clone(),
                message: format!(
                    "is in the scope `{id}`, which cannot be deleted while a memory is in it"
                ),
            });
        }
    }
    for scope in catalog.scopes() {
        if &scope.id != id && scope.document.implies.contains(id) {
            errors.push(ValidationMessage {
                path: scope.path.clone(),
                message: format!(
                    "implies the scope `{id}`, which cannot be deleted while a scope implies it"
                ),
            });
        }
    }
    errors
}

fn scope_conflict(entry: &ScopeEntry) -> OperationError {
    OperationError::Conflict {
        what: format!("the scope `{}`", entry.id),
        current: CurrentDocument::Scope(Box::new(ScopeDoc::of(entry))),
    }
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// How many commits a page of the store's history holds when the caller asks for
/// no number.
pub const DEFAULT_HISTORY_LIMIT: usize = 50;

/// The most commits one page of the store's history holds, whatever the caller
/// asks for.
pub const MAX_HISTORY_LIMIT: usize = 200;

/// One page of the store's commits, newest first.
///
/// `before` is a commit the page starts after, so a caller reads the whole
/// history by passing back the `next_before` it was given.
pub async fn store_history(
    state: &AppState,
    before: Option<String>,
    limit: usize,
) -> Result<StoreHistory, OperationError> {
    let before = match before.as_deref() {
        None => None,
        Some(text) => match Oid::from_str(text) {
            Ok(oid) => Some(oid),
            Err(_) => return Err(OperationError::NotFound(format!("the commit `{text}`"))),
        },
    };
    let limit = limit.clamp(1, MAX_HISTORY_LIMIT);
    let summaries = state.store.log(before, limit).await?;
    // A page that came back short is the end of the history: there is nothing
    // older left to ask for.
    let next_before = match summaries.last() {
        Some(oldest) if summaries.len() == limit => Some(oldest.oid.to_string()),
        _ => None,
    };
    Ok(StoreHistory {
        commits: summaries.iter().map(Commit::of).collect(),
        next_before,
    })
}

/// One commit of the store with every file it changed and each file's diff.
pub async fn store_commit(state: &AppState, oid: &str) -> Result<CommitFiles, OperationError> {
    let Ok(commit_oid) = Oid::from_str(oid) else {
        return Err(OperationError::NotFound(format!("the commit `{oid}`")));
    };
    let Some((summary, changes)) = state.store.commit_changes(commit_oid).await? else {
        return Err(OperationError::NotFound(format!("the commit `{oid}`")));
    };
    Ok(CommitFiles {
        commit: Commit::of(&summary),
        files: changes
            .into_iter()
            .map(|change| CommitFile {
                memory_id: MemoryId::from_repository_path(&change.path),
                scope_id: ScopeId::from_repository_path(&change.path),
                path: change.path,
                status: change.status.as_str().to_string(),
                diff: change.diff,
            })
            .collect(),
    })
}

/// The commits that changed one document, newest first.
pub async fn history(
    state: &AppState,
    kind: DocumentKind,
    id: &str,
) -> Result<Vec<Commit>, OperationError> {
    let path = kind.repository_path(id);
    let summaries = read_repository(state, {
        let path = path.clone();
        move |repository| repository.log_for_path(&path)
    })
    .await?;
    if summaries.is_empty() {
        return Err(OperationError::NotFound(kind.describe(id)));
    }
    Ok(summaries.iter().map(Commit::of).collect())
}

/// One memory's file at the head of `main`, line by line, with the commit that
/// last changed each line.
///
/// The whole file as it is stored, frontmatter included, so a line number here
/// is the line number in the file.
pub async fn memory_blame(state: &AppState, id: &MemoryId) -> Result<MemoryBlame, OperationError> {
    let catalog = state.store.snapshot().await?;
    let entry = catalog
        .memory(id)
        .ok_or_else(|| OperationError::missing_memory(id))?;
    let lines = state.store.blame_memory(id).await?;
    Ok(MemoryBlame {
        id: id.clone(),
        version: entry.version.to_string(),
        lines: lines.iter().map(BlameLine::of).collect(),
    })
}

/// One document at one commit, with the change that commit made to it.
///
/// Only a commit that changed this file is addressable, which is what the
/// history page links to; any other oid is reported as missing.
pub async fn history_entry(
    state: &AppState,
    kind: DocumentKind,
    id: &str,
    oid: &str,
) -> Result<HistoryEntry, OperationError> {
    let path = kind.repository_path(id);
    let Ok(commit_oid) = Oid::from_str(oid) else {
        return Err(OperationError::NotFound(format!("the commit `{oid}`")));
    };
    let found = read_repository(state, {
        let path = path.clone();
        move |repository| {
            let Some(summary) = repository
                .log_for_path(&path)?
                .into_iter()
                .find(|summary| summary.oid == commit_oid)
            else {
                return Ok(None);
            };
            let content = repository.blob_at(commit_oid, &path)?;
            let diff = repository.diff_for_path(commit_oid, &path)?;
            Ok(Some((summary, content, diff)))
        }
    })
    .await?;

    let Some((summary, content, diff)) = found else {
        return Err(OperationError::NotFound(format!(
            "{} at the commit `{oid}`",
            kind.describe(id)
        )));
    };
    Ok(HistoryEntry {
        commit: Commit::of(&summary),
        // A commit that deleted the file has no content at that commit; the
        // diff still says what happened.
        content: content
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default(),
        diff,
    })
}

/// The newest commit that changed `path`, or `None` when the file has no
/// history yet.
async fn newest_commit(state: &AppState, path: &str) -> Result<Option<Commit>, OperationError> {
    let path = path.to_string();
    let summaries =
        read_repository(state, move |repository| repository.log_for_path(&path)).await?;
    Ok(summaries.first().map(Commit::of))
}

/// Read the store's git history on a blocking thread.
///
/// A read-only handle of its own, so that reading the history of one document
/// does not queue behind the writer's lock; git's object store is safe to read
/// while another handle commits.
async fn read_repository<Answer, Work>(
    state: &AppState,
    work: Work,
) -> Result<Answer, OperationError>
where
    Work: FnOnce(&GitRepo) -> Result<Answer, GitError> + Send + 'static,
    Answer: Send + 'static,
{
    let path = state.config.store_path.clone();
    let answer = tokio::task::spawn_blocking(move || {
        let repository = GitRepo::open(&path)?;
        work(&repository)
    })
    .await
    .map_err(StoreError::Task)?;
    Ok(answer?)
}

// ---------------------------------------------------------------------------
// Contexts, machines, triggers and review
// ---------------------------------------------------------------------------

/// Every machine the server knows of, sorted and without repeats.
///
/// Two sources, because neither covers the other: a machine whose sessions are
/// all over is only in the statistics log, and a context restored from the
/// state file is in the registry before it sends its next event.
pub async fn machines(
    registry: &ContextRegistry,
    now: DateTime<Utc>,
    recorded: impl IntoIterator<Item = String>,
) -> MachineList {
    let contexts = registry.snapshot(now).await;
    MachineList {
        machines: machine_names(&contexts, recorded).into_iter().collect(),
    }
}

/// The machines of one registry snapshot together with the ones a statistics
/// log recorded, sorted and without repeats.
fn machine_names(
    contexts: &RegistrySnapshot,
    recorded: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    let mut machines: BTreeSet<String> = recorded.into_iter().collect();
    machines.extend(
        contexts
            .contexts
            .iter()
            .map(|record| record.key.machine.clone()),
    );
    machines
}

/// Every context the server knows about, most recently seen first, contexts
/// seen at the same instant by key.
///
/// The order is the one the contexts page lists, so that the page needs no
/// order of its own beyond keeping a subagent with its session.
pub async fn contexts(registry: &ContextRegistry, now: DateTime<Utc>) -> Vec<ContextRow> {
    let mut records = registry.snapshot(now).await.contexts;
    records.sort_by(|left, right| {
        right
            .state
            .last_seen
            .cmp(&left.state.last_seen)
            .then_with(|| left.key.cmp(&right.key))
    });
    records
        .into_iter()
        .map(|record| ContextRow {
            key: record.key.to_string(),
            name: context_name(&record),
            title: record.state.session_title,
            first_prompt: record.state.first_prompt,
            parent: record.state.parent.map(|parent| parent.to_string()),
            task: record.state.task,
            agent_type: record.state.agent_type,
            active_scopes: record.state.active.into_iter().collect(),
            delivered_count: record.state.delivered.len(),
            last_seen: iso8601(record.state.last_seen),
        })
        .collect()
}

/// The text one context would be given at its next hook event, or the whole of
/// what its active scopes hold, rendered by the same renderer the hook uses.
///
/// Nothing is recorded and no context is created: the state is read from the
/// registry snapshot, so a key no context has been seen at is
/// [`OperationError::UnknownContext`] rather than a new context collecting
/// deliveries no session reads.
///
/// [`PromptMode::Due`] computes what the context is owed against its own
/// delivered record, which is what the next event would carry, and
/// [`PromptMode::All`] computes it against an empty record, which is every
/// critical memory of its scopes in full and every knowledge memory as its
/// description. Both count staleness from the context size the last event
/// reported. The text is exactly what the hook would answer: `Due` is rendered
/// as the next ordinary event and `All` as a session start, with the store's
/// file notice applied to both, so a person reads the same characters the model
/// would. A context that is owed nothing has no text, the way an event that is
/// owed nothing is answered with nothing.
pub async fn context_prompt(
    state: &AppState,
    key: &ContextKey,
    mode: PromptMode,
) -> Result<ContextPrompt, OperationError> {
    let snapshot = state.contexts.snapshot(state.clock.now()).await;
    let record = snapshot
        .contexts
        .into_iter()
        .find(|record| &record.key == key)
        .ok_or_else(|| OperationError::UnknownContext(key.to_string()))?;
    let name = context_name(&record);
    let catalog = state.store.snapshot().await?;
    let held = record.state;

    let needs = match mode {
        PromptMode::Due => {
            let previous = crate::hook::previous_texts(&state.store, &held, &catalog).await;
            compute_needs(&catalog, &held, held.tokens, &previous)
        }
        PromptMode::All => {
            let nothing_delivered =
                ContextState::fresh(held.active.clone(), held.parent.clone(), held.last_seen);
            compute_needs(
                &catalog,
                &nothing_delivered,
                held.tokens,
                &PreviousTexts::new(),
            )
        }
    };
    let delivery = Delivery {
        key,
        catalog: &catalog,
        needs: &needs,
        activated: &[],
        announce_empty_scopes: false,
        session_start: matches!(mode, PromptMode::All),
        answer_file_threshold: catalog.settings().answer_file_threshold,
    };
    // An event that is owed nothing answers with nothing at all, so a context
    // that is owed nothing has no text: the heading line the renderer opens with
    // is part of an answer and not an answer of its own.
    let text = match needs.is_empty() {
        true => String::new(),
        false => render::render(&delivery).text,
    };

    // The same conversion every token figure the server reports goes through,
    // over the same length the renderer accounts in.
    let tokens = tokens_of(
        text.encode_utf16().count() as u64,
        catalog.settings().characters_per_token,
    );
    Ok(ContextPrompt {
        key: key.to_string(),
        name,
        mode,
        bytes: text.len() as u64,
        tokens,
        text,
    })
}

/// How much of a task or a first prompt a name keeps. It is one cell of a
/// table beside five others, so it is about the width of a sentence rather
/// than of the paragraph either text can run to.
const NAME_CHARACTERS: usize = 80;

/// What to call one context in a list.
///
/// The source text is, in order: for a subagent, the task it was given, else
/// the kind of subagent it is; then the name the session goes by, whether the
/// user typed it or Claude Code wrote it; else the user's first prompt; else
/// nothing at all, which gives the empty string. A subagent falls through to
/// its session's name only when nothing says what it is doing or what it is,
/// because that name is the session's and not the subagent's. Whitespace,
/// newlines included, collapses to single spaces and the text is trimmed,
/// because the result is one cell of a table and a prompt is written over
/// several lines.
///
/// A title is used whole: it is a name the user deliberately typed, and the
/// part that tells two sessions apart may be anywhere in it. A task and a first
/// prompt are the model's and the user's running prose, so they are cut to
/// [`NAME_CHARACTERS`] characters at the last word boundary among them, with a
/// single `…` in place of what was dropped. The cut counts characters and never
/// bytes, so no multi-byte character is split in half.
fn context_name(record: &ContextRecord) -> String {
    if record.key.is_subagent() {
        if let Some(task) = &record.state.task {
            return cut_at_word_boundary(&collapse_whitespace(task), NAME_CHARACTERS);
        }
        if let Some(agent_type) = &record.state.agent_type {
            return collapse_whitespace(agent_type);
        }
    }
    if let Some(title) = &record.state.session_title {
        return collapse_whitespace(title);
    }
    match &record.state.first_prompt {
        Some(prompt) => cut_at_word_boundary(&collapse_whitespace(prompt), NAME_CHARACTERS),
        None => String::new(),
    }
}

/// `text` with every run of whitespace, newlines included, replaced by one
/// space, and no whitespace at either end.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` cut to `limit` characters at a word boundary, with `…` marking what
/// was dropped. Text of `limit` characters or fewer is returned unchanged and
/// unmarked; a first `limit` characters holding no space is kept whole, because
/// there is no word boundary to cut back to.
fn cut_at_word_boundary(text: &str, limit: usize) -> String {
    let Some((end_of_the_kept_part, _)) = text.char_indices().nth(limit) else {
        return text.to_string();
    };
    let kept = &text[..end_of_the_kept_part];
    let kept = match kept.rfind(' ') {
        Some(last_space) => &kept[..last_space],
        None => kept,
    };
    format!("{}…", kept.trim_end())
}

/// Whether a pattern is a regex that compiles, and what is wrong with it when
/// it is not.
///
/// The same `regex::Regex::new` the store's validation and the trigger index
/// use, so a pattern this reports as good is one a scope file may carry, and the
/// message is the one a refused write would carry.
pub fn validate_pattern(request: &PatternRequest) -> PatternValidity {
    match regex::Regex::new(&request.pattern) {
        Ok(_) => PatternValidity {
            ok: true,
            error: None,
        },
        Err(error) => PatternValidity {
            ok: false,
            error: Some(error.to_string()),
        },
    }
}

/// Which triggers a text fires, without turning anything on.
pub fn trigger_test(catalog: &Catalog, request: &TriggerTestRequest) -> TriggerTestResult {
    let fired = catalog
        .triggers()
        .fire(request.field, &request.text, &request.machine)
        .into_iter()
        .map(|hit| TriggerMatch {
            scope_id: hit.scope,
            field: hit.field,
            pattern: hit.pattern,
        })
        .collect();
    TriggerTestResult { fired }
}

/// Everything wrong with the store, and the memories worth a second look.
pub fn review(catalog: &Catalog) -> ReviewReport {
    let report = validate::validate(catalog);
    let global_only_critical = catalog
        .memories()
        .filter(|entry| entry.kind() == MemoryKind::Critical)
        .filter(|entry| entry.scope() == &ScopeId::global())
        .map(|entry| entry.id.clone())
        .collect();
    ReviewReport {
        errors: report.errors().iter().map(ValidationMessage::of).collect(),
        warnings: report
            .warnings()
            .iter()
            .map(ValidationMessage::of_warning)
            .collect(),
        global_only_critical,
    }
}

// ---------------------------------------------------------------------------
// Sessions
//
// These change one context's own state and never touch the store. Each records
// a tool call, because the statistics answer "which session turned what on".
// ---------------------------------------------------------------------------

/// The scopes one context works in, and the ones it could turn on.
pub async fn session_scopes(
    state: &AppState,
    key: &ContextKey,
) -> Result<SessionScopes, OperationError> {
    let catalog = state.store.snapshot().await?;
    let active = state
        .contexts
        .with_context(
            key,
            state.clock.now(),
            Inheritance::of(catalog.settings()),
            |context| context.active.clone(),
        )
        .await;
    record_tool_call(state, "session_scopes", key, &[], true).await;
    Ok(session_scopes_of(&catalog, active))
}

/// Turn scopes on for one context, with everything they imply.
///
/// The whole list is one change: every id is checked against the store before
/// anything is turned on, so a list naming a scope this store does not have
/// leaves the context working in exactly the scopes it was, and the implication
/// closure is taken once over the scopes with all of them in.
pub async fn session_scope_on(
    state: &AppState,
    key: &ContextKey,
    scopes: &[ScopeId],
) -> Result<SessionScopes, OperationError> {
    if scopes.is_empty() {
        record_tool_call(state, "session_scope_on", key, scopes, false).await;
        return Err(OperationError::EmptyScopeList);
    }
    let catalog = state.store.snapshot().await?;
    if let Some(unknown) = scopes
        .iter()
        .find(|scope| !validate::is_known_scope(&catalog, scope))
    {
        record_tool_call(state, "session_scope_on", key, scopes, false).await;
        return Err(OperationError::UnknownScope(unknown.clone()));
    }
    let now = state.clock.now();
    let active = state
        .contexts
        .with_context(key, now, Inheritance::of(catalog.settings()), |context| {
            context.active.extend(scopes.iter().cloned());
            context.active = catalog.closure(&context.active);
            // The call has no hook event of its own, so the activation is
            // counted from the context size the last event reported.
            let tokens = context.tokens;
            for scope in scopes {
                context.note_activation(scope, tokens, &catalog);
            }
            context.last_seen = now;
            context.active.clone()
        })
        .await;
    record_tool_call(state, "session_scope_on", key, scopes, true).await;
    Ok(session_scopes_of(&catalog, active))
}

/// Turn scopes off for one context: it stops working in them, and nothing
/// happens to any memory.
///
/// Only the scopes named are turned off. A scope that is on because another
/// scope implies it stays on, because a scope is a flag and the state does not
/// record which trigger or which implication set it.
///
/// The whole list is one change: a list naming a scope that cannot be turned off
/// turns none of the others off either.
pub async fn session_scope_off(
    state: &AppState,
    key: &ContextKey,
    scopes: &[ScopeId],
) -> Result<SessionScopes, OperationError> {
    if scopes.is_empty() {
        record_tool_call(state, "session_scope_off", key, scopes, false).await;
        return Err(OperationError::EmptyScopeList);
    }
    // `global`, `machine:<name>` and the context's own session scope are what
    // makes a context a context; without them it could not be delivered to at
    // all, so they cannot be turned off.
    if let Some(implicit) = scopes.iter().find(|scope| scope.is_implicit()) {
        record_tool_call(state, "session_scope_off", key, scopes, false).await;
        return Err(OperationError::ImplicitScope(implicit.clone()));
    }
    let catalog = state.store.snapshot().await?;
    let now = state.clock.now();
    let active = state
        .contexts
        .with_context(key, now, Inheritance::of(catalog.settings()), |context| {
            for scope in scopes {
                context.active.remove(scope);
            }
            context.last_seen = now;
            context.active.clone()
        })
        .await;
    record_tool_call(state, "session_scope_off", key, scopes, true).await;
    Ok(session_scopes_of(&catalog, active))
}

/// Take over another session's scopes, including its own session scope, so that
/// its session memories become due here as well.
pub async fn session_inherit(
    state: &AppState,
    key: &ContextKey,
    from: &ContextKey,
) -> Result<SessionScopes, OperationError> {
    let catalog = state.store.snapshot().await?;
    let now = state.clock.now();
    // Read from the recorded contexts rather than through `with_context`, so
    // that naming a session nobody has ever seen is an error instead of
    // silently creating an empty one to inherit from.
    let snapshot = state.contexts.snapshot(now).await;
    let Some(source) = snapshot.contexts.iter().find(|record| &record.key == from) else {
        record_tool_call(state, "session_inherit", key, &[], false).await;
        return Err(OperationError::UnknownSession(from.to_string()));
    };

    let mut inherited = source.state.active.clone();
    inherited.insert(from.session_scope());
    let active = state
        .contexts
        .with_context(key, now, Inheritance::of(catalog.settings()), |context| {
            context.active.extend(inherited);
            context.last_seen = now;
            context.active.clone()
        })
        .await;
    record_tool_call(state, "session_inherit", key, &[], true).await;
    Ok(session_scopes_of(&catalog, active))
}

fn session_scopes_of(catalog: &Catalog, active: BTreeSet<ScopeId>) -> SessionScopes {
    let available = catalog
        .scopes()
        .map(|entry| entry.id.clone())
        .filter(|id| !active.contains(id))
        .collect();
    SessionScopes {
        active: active.into_iter().collect(),
        available,
    }
}

/// Record one row for one session tool call, whatever it did.
///
/// A call naming several scopes is one row, with the ids it named in the row's
/// scope text, because the row stands for the call.
async fn record_tool_call(
    state: &AppState,
    tool: &str,
    key: &ContextKey,
    scopes: &[ScopeId],
    ok: bool,
) {
    let named = scopes
        .iter()
        .map(ScopeId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    state
        .stats
        .record_tool_call(ToolCallRecord {
            ts: state.clock.now(),
            tool: tool.to_string(),
            session_key: key.to_string(),
            memory: None,
            scope: (!named.is_empty()).then_some(named),
            ok,
        })
        .await;
}

// ---------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------

/// What a write is refused for when it does not say which version it replaces.
const MISSING_BASE_VERSION: &str =
    "base_version is required: a write replaces the version it was read from";

/// That refusal about a document that exists, naming the version the store
/// holds, so that a caller which sent none can read that version and write
/// against it instead of asking for it in a call of its own.
fn missing_base_version(path: &str, current: Oid) -> OperationError {
    OperationError::invalid(
        path,
        &format!("{MISSING_BASE_VERSION}; the current version is {current}"),
    )
}

/// What a write is refused for when its `base_version` is not a version at all.
pub(crate) const UNREADABLE_BASE_VERSION: &str = "base_version is not a version of this document";

/// What a replacement is refused for when it does not say what to replace.
const EMPTY_SNIPPET: &str = "old_string is empty: a replacement needs the text it replaces";

/// What a replacement is refused for when the snippet is not in the body.
const SNIPPET_NOT_FOUND: &str =
    "old_string does not appear in this memory's body, so there is nothing to replace";

/// What a field write is refused for when it names no field.
const NO_FIELDS_TO_SET: &str =
    "no field was given: send at least one of description, kind, scope or source";

/// What a rename is refused for when it moves a memory to where it already is.
const SAME_RENAME_TARGET: &str = "the memory already has this id, so there is nothing to move";

/// The catalog a write is made against: the branch's own head when the write
/// names one, else `main`.
///
/// This is what makes a branch a transaction: the version check and the
/// validation of a write on a branch see the branch, so two writes on one branch
/// build on each other and neither sees anything landed on `main` meanwhile.
pub async fn target_catalog(
    state: &AppState,
    branch: Option<&BranchName>,
) -> Result<Arc<Catalog>, OperationError> {
    match branch {
        None => Ok(state.store.snapshot().await?),
        Some(branch) => state
            .store
            .branch_snapshot(branch)
            .await?
            .ok_or_else(|| OperationError::UnknownBranch(branch.clone())),
    }
}

/// Commit one set of files where the write says: to `main`, or to one branch.
pub(crate) async fn commit_to(
    state: &AppState,
    branch: Option<&BranchName>,
    author: &str,
    message: &str,
    body: &str,
    files: Vec<(String, Option<Vec<u8>>)>,
    expected_versions: Vec<(String, Option<Oid>)>,
) -> Result<service::WriteOutcome, WriteError> {
    match branch {
        None => {
            state
                .store
                .commit_documents(author, message, body, files, expected_versions)
                .await
        }
        Some(branch) => {
            state
                .store
                .commit_documents_on_branch(branch, author, message, body, files, expected_versions)
                .await
        }
    }
}

/// The commit's body, which records what was written and by whom, since the
/// title line is the author's own words.
fn commit_body(kind: &str, id: &str, author: &str) -> String {
    format!("{kind}: {id}\nauthor: {author}")
}

pub(crate) fn write_outcome(outcome: &crate::service::WriteOutcome, path: &str) -> WriteOutcome {
    WriteOutcome {
        commit_oid: outcome.commit_oid.to_string(),
        version: outcome.blob_oids.get(path).map(Oid::to_string),
    }
}

/// The store's write failures that are not a version mismatch.
pub(crate) fn write_error(path: &str, error: WriteError) -> OperationError {
    match error {
        WriteError::BadMessage => {
            OperationError::invalid(path, &WriteError::BadMessage.to_string())
        }
        WriteError::Conflict => OperationError::Busy,
        WriteError::NoSuchBranch(branch) => OperationError::UnknownBranch(branch),
        WriteError::Store(error) => OperationError::Store(error),
        WriteError::Git(error) => OperationError::Git(error),
        // Handled by the callers, which answer it with the current document.
        WriteError::VersionMismatch { .. } => OperationError::Busy,
    }
}

/// A body that ends with a newline, which is what a text file is; an editor
/// that drops the last one must not make every save a whitespace diff.
fn with_final_newline(body: &str) -> String {
    if body.is_empty() || body.ends_with('\n') {
        body.to_string()
    } else {
        format!("{body}\n")
    }
}

/// A timestamp in the form the API and the frontend use.
pub(crate) fn iso8601(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session's own context, with nothing said about it yet.
    fn session_record() -> ContextRecord {
        ContextRecord {
            key: ContextKey::main("alpha", "session-1"),
            state: ContextState::fresh(BTreeSet::new(), None, Utc::now()),
        }
    }

    /// A subagent's context, with nothing said about it yet.
    fn subagent_record() -> ContextRecord {
        let parent = ContextKey::main("alpha", "session-1");
        ContextRecord {
            key: ContextKey::subagent("alpha", "session-1", "agent-7f3a"),
            state: ContextState::fresh(BTreeSet::new(), Some(parent), Utc::now()),
        }
    }

    /// Detects a session listed by its identifier alone, or by a title cut to
    /// fit a cell: the name the user gave a session with `/rename` is the only
    /// thing that says what the session is, and the part that tells two
    /// sessions apart may be anywhere in it. Detects a title losing to the
    /// first prompt as well, which would name the session by what was said in
    /// it rather than by what the user called it.
    ///
    /// Source: the doc comment of [`context_name`], which uses a title whole
    /// and prefers it over the first prompt.
    #[test]
    fn a_session_the_user_named_is_named_by_that_title_whole() {
        let title =
            "Rebuild the vacuum former thermocouple rig and write up what the old one did wrong";
        assert!(
            title.chars().count() > NAME_CHARACTERS,
            "the title has to be longer than the cut, or a cut title would pass"
        );
        let mut record = session_record();
        record.state.session_title = Some(title.to_string());
        record.state.first_prompt = Some("Prüfe die Späne am Drehbankbett".to_string());

        assert_eq!(
            context_name(&record),
            title,
            "a session the user named must be named by that title, whole"
        );
    }

    /// Detects a name cut by bytes rather than by characters, and one cut in
    /// the middle of a word: a session nobody named is named by its first
    /// prompt, and a prompt runs to paragraphs, so it is cut to fit one cell of
    /// a table. A cut counted in bytes lands somewhere else, and one that
    /// splits a multi-byte character produces text no reader can show.
    ///
    /// The expectation is computed from the rule rather than captured: the
    /// prompt's first 80 characters end "… sofort de", the last space among
    /// them is the one before "der", so what is kept is the 77 characters up to
    /// "sofort" and one `…` stands for the rest.
    #[test]
    fn a_session_nobody_named_is_named_by_its_first_prompt_cut_at_a_word_boundary() {
        let prompt = "Prüfe die Späne am Drehbankbett und melde jeden Wert über neunzig Grad \
                      sofort der Werkstatt weiter";
        let mut record = session_record();
        record.state.first_prompt = Some(prompt.to_string());

        assert_eq!(
            context_name(&record),
            "Prüfe die Späne am Drehbankbett und melde jeden Wert über neunzig Grad sofort…",
            "a first prompt must be cut at a word boundary and counted in characters"
        );
    }

    /// Detects a subagent named by its session: a subagent has no title of its
    /// own and its session's first prompt says nothing about what the subagent
    /// is doing, so the task it was given is the only thing that names it, and
    /// the kind of subagent is what is left when no task was read.
    ///
    /// Source: the doc comment of [`context_name`], which takes the task first,
    /// then the agent type, and falls through to the session's name only when
    /// neither is there.
    #[test]
    fn a_subagent_is_named_by_its_task_and_by_its_kind_when_no_task_was_read() {
        let session_title = "Rebuild the vacuum former thermocouple rig";
        let task = "Survey the rocketry crate and list its public functions";

        let mut tasked = subagent_record();
        tasked.state.session_title = Some(session_title.to_string());
        tasked.state.task = Some(task.to_string());
        tasked.state.agent_type = Some("general-purpose".to_string());
        assert_eq!(
            context_name(&tasked),
            task,
            "a subagent must be named for the task it was given"
        );

        let mut untasked = subagent_record();
        untasked.state.session_title = Some(session_title.to_string());
        untasked.state.agent_type = Some("general-purpose".to_string());
        assert_eq!(
            context_name(&untasked),
            "general-purpose",
            "a subagent with no task must be named for what kind of subagent it is"
        );

        let unknown = subagent_record();
        assert_eq!(
            context_name(&unknown),
            "",
            "a subagent nothing has said anything about has no name to show"
        );
    }

    /// Detects a name built from a prompt's line breaks and runs of spaces: a
    /// prompt is written over several lines, and a cell of a table holding the
    /// newlines would break the row it is in.
    ///
    /// Source: the doc comment of [`context_name`], which collapses whitespace
    /// and trims before anything else.
    #[test]
    fn a_name_collapses_the_whitespace_of_the_text_it_comes_from() {
        let mut record = session_record();
        record.state.session_title = Some("  the thermocouple\n\trig,\n\nsecond attempt  ".into());

        assert_eq!(
            context_name(&record),
            "the thermocouple rig, second attempt",
            "every run of whitespace must collapse to one space and the ends must be trimmed"
        );
    }
}
