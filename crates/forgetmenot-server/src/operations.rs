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

use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat, SubsecRound, Utc};
use git2::Oid;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::context::registry::ContextRegistry;
use crate::context::{ContextKey, Form, Shown};
use crate::service::{StoreError, WriteError, is_valid_message_title};
use crate::stats::ToolCallRecord;
use crate::store::catalog::{Catalog, MemoryEntry, ScopeEntry};
use crate::store::frontmatter::FrontmatterError;
use crate::store::git::{CommitSummary, GitError, GitRepo};
use crate::store::memory::{
    MemoryDocument, MemoryFrontmatter, MemoryKind, MemoryMetadata, MemorySource,
};
use crate::store::scope::{ScopeDocument, Trigger, TriggerField};
use crate::store::validate::{self, Candidate, ValidationError, WriteMode};
use crate::store::{MemoryId, ScopeId};

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

    #[error("`{0}` is always on for a session and cannot be turned off")]
    ImplicitScope(ScopeId),

    #[error("no session `{0}` has been seen by this server")]
    UnknownSession(String),

    #[error(transparent)]
    Store(#[from] StoreError),
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
    fn invalid(path: &str, message: &str) -> Self {
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
    fn of(error: &ValidationError) -> Self {
        Self {
            path: error.path().to_string(),
            message: error.to_string(),
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
    /// Both variants are boxed: every operation's failure carries this enum, and
    /// a whole document is far larger than the answer it travels with.
    Memory(Box<MemoryDoc>),
    Scope(Box<ScopeDoc>),
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
    pub scopes: Vec<ScopeId>,
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
    pub scopes: Vec<ScopeId>,
    pub source: MemorySource,
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
    pub implies: Vec<ScopeId>,
    pub triggers: Vec<Trigger>,
    pub version: String,
}

impl ScopeDoc {
    fn of(entry: &ScopeEntry) -> Self {
        Self {
            id: entry.id.clone(),
            implies: entry.document.implies.clone(),
            triggers: entry.document.triggers.clone(),
            version: entry.version.to_string(),
        }
    }
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

/// One live context.
#[derive(Clone, Debug, Serialize)]
pub struct ContextRow {
    /// The session key, the same form the MCP tools take.
    pub key: String,
    pub active_scopes: Vec<ScopeId>,
    pub delivered_count: usize,
    /// ISO 8601.
    pub last_seen: String,
}

/// What the review page reports.
#[derive(Clone, Debug, Serialize)]
pub struct ReviewReport {
    pub errors: Vec<ValidationMessage>,
    /// Critical memories whose only scope is `global`: they are delivered in
    /// full to every session on every machine, which is worth a second look.
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
    pub field: TriggerField,
    pub text: String,
    /// The machine the text is supposed to come from, which decides whether a
    /// machine-qualified `working_directory` trigger applies.
    pub machine: String,
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
            && !entry.scopes().contains(scope)
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
    pub scopes: Vec<ScopeId>,
    pub source: MemorySource,
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

/// A deletion, which removes the file and so carries no content.
#[derive(Clone, Debug, Deserialize)]
pub struct MemoryDeleteRequest {
    pub base_version: String,
    pub author: String,
    pub message: String,
}

/// A write of one scope.
#[derive(Clone, Debug, Deserialize)]
pub struct ScopeWriteRequest {
    pub implies: Vec<ScopeId>,
    pub triggers: Vec<Trigger>,
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
/// With a context key the memory is recorded as delivered in full to that
/// context, because that is what fetching it is: the model now has the body,
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
        let version = entry.version.to_string();
        state
            .contexts
            .with_context(key, now, |context| {
                context.delivered.insert(
                    id.clone(),
                    Shown {
                        version,
                        form: Form::Full,
                        // No hook event accompanies a fetch, so the context size
                        // at this moment is unknown; staleness is judged again
                        // from the next event that does carry one.
                        tokens: None,
                    },
                );
                context.last_seen = now;
            })
            .await;
    }
    Ok(document)
}

/// Write one memory: create it or replace the version the caller read.
pub async fn memory_put(
    state: &AppState,
    id: &MemoryId,
    request: &MemoryWriteRequest,
    mode: WriteMode,
) -> Result<WriteOutcome, OperationError> {
    let catalog = state.store.snapshot().await?;
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
                return Err(OperationError::invalid(&path, MISSING_BASE_VERSION));
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
    document.frontmatter.metadata.scopes = Some(request.scopes.clone());
    document.frontmatter.metadata.source = Some(request.source);
    document.frontmatter.metadata.author = Some(request.author.clone());
    document.body = with_final_newline(&request.body);

    commit_memory(
        state,
        &catalog,
        document,
        mode,
        expected,
        &request.author,
        &request.message,
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
    request: &MemoryDeleteRequest,
) -> Result<WriteOutcome, OperationError> {
    let catalog = state.store.snapshot().await?;
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

    let outcome = state
        .store
        .commit_documents(
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
            let catalog = state.store.snapshot().await?;
            match catalog.memory(id) {
                Some(entry) => Err(memory_conflict(state, &catalog, entry).await),
                None => Err(OperationError::missing_memory(id)),
            }
        }
        Err(error) => Err(write_error(&path, error)),
    }
}

/// Validate one memory and commit it, which is the part every memory write
/// shares.
async fn commit_memory(
    state: &AppState,
    catalog: &Catalog,
    document: MemoryDocument,
    mode: WriteMode,
    // The version the write replaces, `None` when it creates the file.
    expected: Option<Oid>,
    author: &str,
    message: &str,
) -> Result<WriteOutcome, OperationError> {
    let id = document.id.clone();
    let path = id.repository_path();
    let report = validate::validate_write(
        catalog,
        Candidate::Memory {
            document: &document,
        },
        mode,
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
    let outcome = state
        .store
        .commit_documents(
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
            let catalog = state.store.snapshot().await?;
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
        scopes: entry.scopes().to_vec(),
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
        scopes: entry.scopes().to_vec(),
        source: entry.document.source(),
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

/// Every scope that has a file, ordered by id.
pub fn scope_index(catalog: &Catalog) -> Vec<ScopeDoc> {
    catalog.scopes().map(ScopeDoc::of).collect()
}

/// One scope. The implicit scopes have no file and so are not readable here.
pub fn scope_get(catalog: &Catalog, id: &ScopeId) -> Result<ScopeDoc, OperationError> {
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
) -> Result<WriteOutcome, OperationError> {
    let catalog = state.store.snapshot().await?;
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
                return Err(OperationError::invalid(&path, MISSING_BASE_VERSION));
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
        implies: request.implies.clone(),
        triggers: request.triggers.clone(),
    };
    let report = validate::validate_write(
        &catalog,
        Candidate::Scope {
            id,
            document: &document,
        },
        mode,
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
    let outcome = state
        .store
        .commit_documents(
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
            let catalog = state.store.snapshot().await?;
            match catalog.scope(id) {
                Some(entry) => Err(scope_conflict(entry)),
                None => Err(OperationError::missing_scope(id)),
            }
        }
        Err(error) => Err(write_error(&path, error)),
    }
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
// Contexts, triggers and review
// ---------------------------------------------------------------------------

/// Every context the server knows about, ordered by key.
pub async fn contexts(registry: &ContextRegistry, now: DateTime<Utc>) -> Vec<ContextRow> {
    registry
        .snapshot(now)
        .await
        .contexts
        .into_iter()
        .map(|record| ContextRow {
            key: record.key.to_string(),
            active_scopes: record.state.active.into_iter().collect(),
            delivered_count: record.state.delivered.len(),
            last_seen: iso8601(record.state.last_seen),
        })
        .collect()
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
        .filter(|entry| entry.scopes() == [ScopeId::global()])
        .map(|entry| entry.id.clone())
        .collect();
    ReviewReport {
        errors: report.errors().iter().map(ValidationMessage::of).collect(),
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
        .with_context(key, state.clock.now(), |context| context.active.clone())
        .await;
    record_tool_call(state, "session_scopes", key, None, true).await;
    Ok(session_scopes_of(&catalog, active))
}

/// Turn one scope on for one context, with everything it implies.
pub async fn session_scope_on(
    state: &AppState,
    key: &ContextKey,
    scope: &ScopeId,
) -> Result<SessionScopes, OperationError> {
    let catalog = state.store.snapshot().await?;
    if !validate::is_known_scope(&catalog, scope) {
        record_tool_call(state, "session_scope_on", key, Some(scope), false).await;
        return Err(OperationError::UnknownScope(scope.clone()));
    }
    let now = state.clock.now();
    let active = state
        .contexts
        .with_context(key, now, |context| {
            context.active.insert(scope.clone());
            context.active = catalog.closure(&context.active);
            context.last_seen = now;
            context.active.clone()
        })
        .await;
    record_tool_call(state, "session_scope_on", key, Some(scope), true).await;
    Ok(session_scopes_of(&catalog, active))
}

/// Turn one scope off for one context: it stops working in that scope, and
/// nothing happens to any memory.
///
/// Only the scope named is turned off. A scope that is on because another scope
/// implies it stays on, because a scope is a flag and the state does not record
/// which trigger or which implication set it.
pub async fn session_scope_off(
    state: &AppState,
    key: &ContextKey,
    scope: &ScopeId,
) -> Result<SessionScopes, OperationError> {
    // `global`, `machine:<name>` and the context's own session scope are what
    // makes a context a context; without them it could not be delivered to at
    // all, so they cannot be turned off.
    if scope.is_implicit() {
        record_tool_call(state, "session_scope_off", key, Some(scope), false).await;
        return Err(OperationError::ImplicitScope(scope.clone()));
    }
    let catalog = state.store.snapshot().await?;
    let now = state.clock.now();
    let active = state
        .contexts
        .with_context(key, now, |context| {
            context.active.remove(scope);
            context.last_seen = now;
            context.active.clone()
        })
        .await;
    record_tool_call(state, "session_scope_off", key, Some(scope), true).await;
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
        record_tool_call(state, "session_inherit", key, None, false).await;
        return Err(OperationError::UnknownSession(from.to_string()));
    };

    let mut inherited = source.state.active.clone();
    inherited.insert(from.session_scope());
    let active = state
        .contexts
        .with_context(key, now, |context| {
            context.active.extend(inherited);
            context.last_seen = now;
            context.active.clone()
        })
        .await;
    record_tool_call(state, "session_inherit", key, None, true).await;
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

async fn record_tool_call(
    state: &AppState,
    tool: &str,
    key: &ContextKey,
    scope: Option<&ScopeId>,
    ok: bool,
) {
    state
        .stats
        .record_tool_call(ToolCallRecord {
            ts: state.clock.now(),
            tool: tool.to_string(),
            session_key: key.to_string(),
            memory: None,
            scope: scope.map(ScopeId::to_string),
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

/// What a write is refused for when its `base_version` is not a version at all.
const UNREADABLE_BASE_VERSION: &str = "base_version is not a version of this document";

/// The commit's body, which records what was written and by whom, since the
/// title line is the author's own words.
fn commit_body(kind: &str, id: &str, author: &str) -> String {
    format!("{kind}: {id}\nauthor: {author}")
}

fn write_outcome(outcome: &crate::service::WriteOutcome, path: &str) -> WriteOutcome {
    WriteOutcome {
        commit_oid: outcome.commit_oid.to_string(),
        version: outcome.blob_oids.get(path).map(Oid::to_string),
    }
}

/// The store's write failures that are not a version mismatch.
fn write_error(path: &str, error: WriteError) -> OperationError {
    match error {
        WriteError::BadMessage => {
            OperationError::invalid(path, &WriteError::BadMessage.to_string())
        }
        WriteError::Conflict => OperationError::Busy,
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
fn iso8601(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Secs, true)
}
