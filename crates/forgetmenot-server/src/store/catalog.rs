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
use super::memory::{MemoryDocument, MemoryKind};
use super::scope::ScopeDocument;
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
    scopes: BTreeMap<ScopeId, ScopeEntry>,
    memories: BTreeMap<MemoryId, MemoryEntry>,
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

        for entry in repository.read_tree(head)? {
            if let Some(id) = ScopeId::from_repository_path(&entry.path) {
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

        Ok(Self {
            head,
            scopes,
            memories,
            implied,
            triggers,
            load_errors,
            load_warnings,
        })
    }

    /// The memory with this id.
    pub fn memory(&self, id: &MemoryId) -> Option<&MemoryEntry> {
        self.memories.get(id)
    }

    /// Every memory, ordered by id.
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
                trigger.on,
                &trigger.pattern,
                trigger.machine.clone(),
            ) {
                load_errors.push(ValidationError::InvalidTriggerPattern {
                    path: entry.path.clone(),
                    scope: entry.id.clone(),
                    field: trigger.on,
                    pattern: trigger.pattern.clone(),
                    message: error.to_string(),
                });
            }
        }
    }
    builder.build(implied.clone())
}
