//! The parsed store at one revision.
//!
//! A catalog is built from a single read of one commit's tree and is immutable
//! afterwards, so every decision a request makes is made against one store
//! version. A file that cannot be parsed, or a trigger pattern that cannot be
//! compiled, is left out and recorded as a validation error: one broken file
//! must not take the whole store offline.

use std::collections::{BTreeMap, BTreeSet};

use git2::Oid;

use super::frontmatter::FrontmatterError;
use super::git::{GitError, GitRepo};
use super::memory::{MemoryDocument, MemoryFrontmatter, MemoryKind, MemoryMetadata};
use super::scope::ScopeDocument;
use super::settings::{SETTINGS_PATH, SettingProblem, Settings, SettingsFile};
use super::validate::{ValidationError, ValidationWarning};
use super::{MemoryId, ScopeId};
use crate::triggers::{TriggerIndex, TriggerIndexBuilder};

/// A failure that stops a catalog from being built at all.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("the trigger index could not be compiled: {0}")]
    Triggers(#[from] regex::Error),
}

/// One memory in the catalog.
#[derive(Clone, Debug)]
pub struct MemoryEntry {
    pub id: MemoryId,
    /// Path inside the repository.
    pub path: String,
    /// The blob id of the file, used as the document's version by the write
    /// path and as the delivered version by the state machine.
    pub version: Oid,
    pub document: MemoryDocument,
    /// The `[[target]]` links of the body, unresolved.
    pub links: Vec<String>,
}

impl MemoryEntry {
    pub fn kind(&self) -> MemoryKind {
        self.document.kind()
    }

    pub fn scopes(&self) -> &[ScopeId] {
        self.document.scopes()
    }
}

/// One scope that has a file in the catalog.
#[derive(Clone, Debug)]
pub struct ScopeEntry {
    /// The file stem, which is the id everything else refers to.
    pub id: ScopeId,
    pub path: String,
    pub version: Oid,
    pub document: ScopeDocument,
}

/// The store as parsed from one commit.
pub struct Catalog {
    /// The commit this catalog was built from.
    pub head: Oid,
    /// The behaviour settings this revision puts in force, and the entries the
    /// file itself carries.
    settings: SettingsFile,
    /// The blob id of `config.yml`, `None` when the revision has no such file.
    settings_version: Option<Oid>,
    scopes: BTreeMap<ScopeId, ScopeEntry>,
    memories: BTreeMap<MemoryId, MemoryEntry>,
    /// The message of each scope that carries one, as the critical memory it is
    /// delivered as. Kept apart from the file memories so that everything that
    /// reads the store's files reads those alone.
    scope_messages: BTreeMap<MemoryId, MemoryEntry>,
    implied: BTreeMap<ScopeId, BTreeSet<ScopeId>>,
    triggers: TriggerIndex,
    load_errors: Vec<ValidationError>,
    load_warnings: Vec<ValidationWarning>,
}

impl Catalog {
    /// Build the catalog from the repository's current head.
    pub fn load(repository: &GitRepo) -> Result<Self, LoadError> {
        let head = repository.head_oid()?;
        Self::load_at(repository, head)
    }

    /// Build the catalog from one revision.
    pub fn load_at(repository: &GitRepo, head: Oid) -> Result<Self, LoadError> {
        let mut load_errors = Vec::new();
        let mut load_warnings = Vec::new();
        let mut scopes = BTreeMap::new();
        let mut memories = BTreeMap::new();
        let mut settings = SettingsFile::default();
        let mut settings_version = None;

        for entry in repository.read_tree(head)? {
            if entry.path == SETTINGS_PATH {
                // The version is recorded whatever the file says, so that a
                // write against a file this server cannot read is still a
                // compare-and-swap against the file that is there.
                settings_version = Some(entry.blob_oid);
                match super::settings::parse(&entry.bytes) {
                    Ok((file, problems)) => {
                        settings = file;
                        load_errors.extend(
                            problems
                                .iter()
                                .map(|problem| settings_error(&entry.path, problem)),
                        );
                    }
                    Err(error) => load_errors.push(ValidationError::ParseFailure {
                        path: entry.path,
                        message: error.to_string(),
                    }),
                }
            } else if let Some(id) = ScopeId::from_repository_path(&entry.path) {
                match ScopeDocument::parse(&entry.bytes) {
                    Ok(document) => {
                        scopes.insert(
                            id.clone(),
                            ScopeEntry {
                                id,
                                path: entry.path,
                                version: entry.blob_oid,
                                document,
                            },
                        );
                    }
                    Err(error) => load_errors.push(ValidationError::ParseFailure {
                        path: entry.path,
                        message: error.to_string(),
                    }),
                }
            } else if let Some(id) = MemoryId::from_repository_path(&entry.path) {
                match MemoryDocument::parse(id.clone(), &entry.bytes) {
                    Ok(document) => {
                        let links = document.links();
                        let replaced = memories.insert(
                            id.clone(),
                            MemoryEntry {
                                id: id.clone(),
                                path: entry.path.clone(),
                                version: entry.blob_oid,
                                document,
                                links,
                            },
                        );
                        // Two paths in one tree cannot produce the same id, so
                        // this guards a catalog built from any other source.
                        if replaced.is_some() {
                            load_errors.push(ValidationError::DuplicateMemoryId {
                                path: entry.path,
                                id,
                            });
                        }
                    }
                    // A file under `memories/` with no frontmatter at all is
                    // not a memory: Claude Code's own index file looks like
                    // that, and a directory holding one has to load anyway.
                    Err(FrontmatterError::MissingOpeningDelimiter) => {
                        load_warnings.push(ValidationWarning::NotAMemoryFile { path: entry.path })
                    }
                    Err(error) => load_errors.push(ValidationError::ParseFailure {
                        path: entry.path,
                        message: error.to_string(),
                    }),
                }
            }
        }

        let implied = implies_closures(&scopes);
        let triggers = compile_triggers(&scopes, &implied, &mut load_errors)?;
        let scope_messages = scope_message_entries(&scopes, &memories, &mut load_errors);

        Ok(Self {
            head,
            settings,
            settings_version,
            scopes,
            memories,
            scope_messages,
            implied,
            triggers,
            load_errors,
            load_warnings,
        })
    }

    /// The behaviour settings in force at this revision.
    pub fn settings(&self) -> &Settings {
        &self.settings.settings
    }

    /// The entries the settings file itself carries, which a write of one key
    /// keeps.
    pub fn settings_entries(&self) -> &yaml_serde::Mapping {
        &self.settings.entries
    }

    /// The version of the settings file, `None` when the store has none.
    pub fn settings_version(&self) -> Option<Oid> {
        self.settings_version
    }

    /// The memory with this id, whether a file holds it or a scope's message is
    /// delivered as it.
    pub fn memory(&self, id: &MemoryId) -> Option<&MemoryEntry> {
        self.memories
            .get(id)
            .or_else(|| self.scope_messages.get(id))
    }

    /// The memory with this id, only if a file in the store holds it.
    ///
    /// What the version of a path and the target of a `[[link]]` are answered
    /// from: a scope's message has no file of its own and nothing links to it.
    pub fn file_memory(&self, id: &MemoryId) -> Option<&MemoryEntry> {
        self.memories.get(id)
    }

    /// Every memory a file holds, ordered by id. The message a scope carries is
    /// not one of them: it is delivered from the scope file and has no file of
    /// its own, so nothing that reads, writes or lists memory files sees it.
    pub fn memories(&self) -> impl ExactSizeIterator<Item = &MemoryEntry> {
        self.memories.values()
    }

    /// The scope with this id, if it has a file.
    pub fn scope(&self, id: &ScopeId) -> Option<&ScopeEntry> {
        self.scopes.get(id)
    }

    /// Every scope that has a file, ordered by id.
    pub fn scopes(&self) -> impl ExactSizeIterator<Item = &ScopeEntry> {
        self.scopes.values()
    }

    /// The compiled triggers of the whole store.
    pub fn triggers(&self) -> &TriggerIndex {
        &self.triggers
    }

    /// Problems found while building this catalog.
    pub fn load_errors(&self) -> &[ValidationError] {
        &self.load_errors
    }

    /// Files skipped while building this catalog, and why.
    pub fn load_warnings(&self) -> &[ValidationWarning] {
        &self.load_warnings
    }

    /// The memories due in a context whose active scopes are `active`.
    ///
    /// Critical memories come first because they are delivered in full and a
    /// reader should meet them before the index lines; within a kind the order
    /// is by id so that the rendered context is stable between events.
    pub fn due(&self, active: &BTreeSet<ScopeId>) -> Vec<&MemoryEntry> {
        let mut due: Vec<&MemoryEntry> = self
            .memories
            .values()
            .chain(self.scope_messages.values())
            .filter(|memory| memory.scopes().iter().any(|scope| active.contains(scope)))
            .collect();
        due.sort_by(|left, right| left.kind().cmp(&right.kind()).then(left.id.cmp(&right.id)));
        due
    }

    /// `active` plus every scope those scopes imply, transitively.
    pub fn closure(&self, active: &BTreeSet<ScopeId>) -> BTreeSet<ScopeId> {
        let mut closed = active.clone();
        for scope in active {
            if let Some(implied) = self.implied.get(scope) {
                closed.extend(implied.iter().cloned());
            }
        }
        closed
    }
}

/// One problem with one settings key, as the report shows it against the file.
fn settings_error(path: &str, problem: &SettingProblem) -> ValidationError {
    match problem {
        SettingProblem::UnknownKey { key } => ValidationError::UnknownSetting {
            path: path.to_string(),
            key: key.clone(),
        },
        SettingProblem::WrongType { key, mismatch } => ValidationError::BadSettingValue {
            path: path.to_string(),
            key: key.clone(),
            expected: mismatch.expected,
            found: mismatch.found.clone(),
        },
    }
}

/// The message of each scope that carries one, as the critical memory it is
/// delivered as: its scope alone, its first line as the description a reader
/// sees in the index, and the scope file's blob as the version that says the
/// message changed.
///
/// An id a memory file already has is left to that file and reported, so that
/// the two maps never hold the same id and nothing is delivered twice.
fn scope_message_entries(
    scopes: &BTreeMap<ScopeId, ScopeEntry>,
    memories: &BTreeMap<MemoryId, MemoryEntry>,
    load_errors: &mut Vec<ValidationError>,
) -> BTreeMap<MemoryId, MemoryEntry> {
    let mut messages = BTreeMap::new();
    for entry in scopes.values() {
        let Some(message) = &entry.document.message else {
            continue;
        };
        let id = MemoryId::for_scope_message(&entry.id);
        if memories.contains_key(&id) {
            load_errors.push(ValidationError::DuplicateMemoryId {
                path: entry.path.clone(),
                id,
            });
            continue;
        }
        let document = scope_message_document(&entry.id, message);
        messages.insert(
            id.clone(),
            MemoryEntry {
                id,
                path: entry.path.clone(),
                version: entry.version,
                document,
                // A message is one short text, not a document that links out.
                links: Vec::new(),
            },
        );
    }
    messages
}

/// The memory one scope's `message` is delivered as: a critical memory of that
/// scope alone, whose body is the message itself and whose description is the
/// message's first line, the one a reader sees in the index.
///
/// The one place that shape is built, so that the message read from any revision
/// of the scope file is the same memory as the one the catalog delivers.
pub fn scope_message_document(scope: &ScopeId, message: &str) -> MemoryDocument {
    MemoryDocument {
        id: MemoryId::for_scope_message(scope),
        frontmatter: MemoryFrontmatter {
            name: scope.as_str().to_string(),
            description: Some(first_line(message).to_string()),
            modified: None,
            extra: yaml_serde::Mapping::new(),
            metadata: MemoryMetadata {
                kind: Some(MemoryKind::Critical),
                scopes: Some(vec![scope.clone()]),
                ..MemoryMetadata::default()
            },
        },
        body: message.to_string(),
    }
}

/// The first line of `text`, trimmed.
fn first_line(text: &str) -> &str {
    text.trim_start().lines().next().unwrap_or_default().trim()
}

/// The transitive `implies` closure of every scope with a file.
///
/// A cycle terminates because a scope is expanded only the first time it is
/// reached; a target with no file is included in the closure but not expanded.
fn implies_closures(
    scopes: &BTreeMap<ScopeId, ScopeEntry>,
) -> BTreeMap<ScopeId, BTreeSet<ScopeId>> {
    let mut closures = BTreeMap::new();
    for (id, entry) in scopes {
        let mut reached = BTreeSet::new();
        let mut pending: Vec<ScopeId> = entry.document.implies.clone();
        while let Some(target) = pending.pop() {
            if &target == id || !reached.insert(target.clone()) {
                continue;
            }
            if let Some(implied_scope) = scopes.get(&target) {
                pending.extend(implied_scope.document.implies.iter().cloned());
            }
        }
        closures.insert(id.clone(), reached);
    }
    closures
}

/// Compile every trigger of every scope, leaving out the patterns that do not
/// compile and recording each of them as a validation error.
fn compile_triggers(
    scopes: &BTreeMap<ScopeId, ScopeEntry>,
    implied: &BTreeMap<ScopeId, BTreeSet<ScopeId>>,
    load_errors: &mut Vec<ValidationError>,
) -> Result<TriggerIndex, regex::Error> {
    let mut builder = TriggerIndexBuilder::new();
    for entry in scopes.values() {
        for trigger in &entry.document.triggers {
            if let Err(error) = builder.push(
                entry.id.clone(),
                trigger.field(),
                &trigger.pattern,
                trigger.machine.clone(),
            ) {
                load_errors.push(ValidationError::InvalidTriggerPattern {
                    path: entry.path.clone(),
                    scope: entry.id.clone(),
                    field: trigger.field(),
                    pattern: trigger.pattern.clone(),
                    message: error.to_string(),
                });
            }
        }
    }
    builder.build(implied.clone())
}
