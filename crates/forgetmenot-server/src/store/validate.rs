//! Store validation.
//!
//! The same rules run in three places: `forgetmenot check`, every write, and
//! the review page. They are expressed once here, against a catalog, so that
//! all three agree.

use super::catalog::Catalog;
use super::memory::MemoryDocument;
use super::scope::{ScopeDocument, TriggerField};
use super::settings::SettingType;
use super::{MemoryId, ScopeId, is_valid_scope_file_id};

/// One problem in one file.
///
/// The path is carried on every variant because every report is read per file:
/// `forgetmenot check` prints `<path>: <error>` and the review page groups by
/// file. The messages therefore do not repeat the path.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("cannot be read: {message}")]
    ParseFailure { path: String, message: String },

    #[error("declares id `{declared}` but its file name requires `{expected}`")]
    ScopeIdMismatch {
        path: String,
        declared: ScopeId,
        expected: ScopeId,
    },

    #[error("id `{id}` does not match the required pattern ^[a-z0-9][a-z0-9-]*$")]
    InvalidScopeId { path: String, id: String },

    #[error("implies `{target}`, which is not a scope in this store")]
    UnknownImpliesTarget {
        path: String,
        scope: ScopeId,
        target: ScopeId,
    },

    #[error("trigger on {field} has a pattern that does not compile: {message}")]
    InvalidTriggerPattern {
        path: String,
        scope: ScopeId,
        field: TriggerField,
        pattern: String,
        message: String,
    },

    #[error("declares the name `{declared}` but its file stem is `{expected}`")]
    MemoryNameMismatch {
        path: String,
        declared: String,
        expected: String,
    },

    #[error("names `{scope}`, which is not a scope in this store")]
    UnknownScope {
        path: String,
        memory: MemoryId,
        scope: ScopeId,
    },

    #[error("would be a second memory with the id `{id}`")]
    DuplicateMemoryId { path: String, id: MemoryId },

    #[error("links to `{target}`, which is not a memory in this store")]
    MissingLinkTarget {
        path: String,
        memory: MemoryId,
        target: String,
    },

    #[error(
        "links to `{target}`, which belongs to session silo `{silo}`; only memories in that silo may link to it"
    )]
    LinkCrossesSessionSilo {
        path: String,
        memory: MemoryId,
        target: MemoryId,
        silo: String,
    },

    #[error("is a session memory, so its scopes must be exactly [{expected}], not [{found}]")]
    SessionMemoryScopes {
        path: String,
        memory: MemoryId,
        expected: ScopeId,
        found: String,
    },

    #[error("sets `{key}`, which is not a setting of this server")]
    UnknownSetting { path: String, key: String },

    #[error("sets `{key}` to {found}, but it takes {expected}")]
    BadSettingValue {
        path: String,
        key: String,
        expected: SettingType,
        found: String,
    },
}

impl ValidationError {
    /// The file this error is about.
    pub fn path(&self) -> &str {
        match self {
            ValidationError::ParseFailure { path, .. }
            | ValidationError::ScopeIdMismatch { path, .. }
            | ValidationError::InvalidScopeId { path, .. }
            | ValidationError::UnknownImpliesTarget { path, .. }
            | ValidationError::InvalidTriggerPattern { path, .. }
            | ValidationError::MemoryNameMismatch { path, .. }
            | ValidationError::UnknownScope { path, .. }
            | ValidationError::DuplicateMemoryId { path, .. }
            | ValidationError::MissingLinkTarget { path, .. }
            | ValidationError::LinkCrossesSessionSilo { path, .. }
            | ValidationError::SessionMemoryScopes { path, .. }
            | ValidationError::UnknownSetting { path, .. }
            | ValidationError::BadSettingValue { path, .. } => path,
        }
    }
}

/// Something that is worth reporting but does not make the store invalid.
///
/// Warnings exist so that a directory of memories Claude Code wrote can be used
/// as a store as it stands: its index file has no frontmatter and some of its
/// memories may have no description, and neither is a reason to refuse the whole
/// store.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationWarning {
    #[error("has no frontmatter, so it is not a memory and was skipped")]
    NotAMemoryFile { path: String },

    #[error("has no `description`, so its index entry is empty")]
    MissingDescription { path: String, memory: MemoryId },
}

impl ValidationWarning {
    /// The file this warning is about.
    pub fn path(&self) -> &str {
        match self {
            ValidationWarning::NotAMemoryFile { path }
            | ValidationWarning::MissingDescription { path, .. } => path,
        }
    }
}

/// Every problem found, in the order the files were visited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationReport {
    errors: Vec<ValidationError>,
    warnings: Vec<ValidationWarning>,
}

impl ValidationReport {
    pub fn errors(&self) -> &[ValidationError] {
        &self.errors
    }

    pub fn warnings(&self) -> &[ValidationWarning] {
        &self.warnings
    }

    /// Whether the store is invalid. This is what `forgetmenot check` and the
    /// write path gate on; warnings do not gate anything.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Whether there is nothing at all to report.
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }

    fn push(&mut self, error: ValidationError) {
        self.errors.push(error);
    }

    fn warn(&mut self, warning: ValidationWarning) {
        self.warnings.push(warning);
    }

    fn extend(&mut self, errors: impl IntoIterator<Item = ValidationError>) {
        self.errors.extend(errors);
    }

    fn extend_warnings(&mut self, warnings: impl IntoIterator<Item = ValidationWarning>) {
        self.warnings.extend(warnings);
    }
}

/// Validate a whole catalog.
pub fn validate(catalog: &Catalog) -> ValidationReport {
    let mut report = ValidationReport::default();
    // Files that could not be parsed and patterns that could not be compiled
    // are found while the catalog is built, since the catalog cannot hold them.
    report.extend(catalog.load_errors().iter().cloned());
    report.extend_warnings(catalog.load_warnings().iter().cloned());

    for scope in catalog.scopes() {
        validate_scope_document(
            catalog,
            &scope.path,
            &scope.id,
            &scope.document,
            CrossDocumentRules::CheckedNow,
            &mut report,
        );
    }
    for memory in catalog.memories() {
        validate_memory_document(
            catalog,
            &memory.path,
            &memory.document,
            CrossDocumentRules::CheckedNow,
            &mut report,
        );
    }
    report
}

/// Whether a write creates a document or replaces the one already at its path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    Create,
    Update,
}

/// A document a caller wants to write, not yet in any catalog.
///
/// Borrowed, because the write path already holds the parsed document and
/// validation only reads it. A memory carries its own id; a scope's id comes
/// from the path it will be written to, since the id declared inside the file
/// may disagree with it and that disagreement is one of the errors reported
/// here.
#[derive(Clone, Copy, Debug)]
pub enum Candidate<'a> {
    Memory {
        document: &'a MemoryDocument,
    },
    Scope {
        id: &'a ScopeId,
        document: &'a ScopeDocument,
    },
}

/// When the rules that are answered from files other than the candidate itself
/// are checked: by this write, or by the land of the branch it is written to.
///
/// A branch is a transaction. Nothing reads the revisions it holds before it
/// lands, and `land_branch` validates the whole merged tree and refuses to land
/// an inconsistent one, so the landed tree is what has to be consistent. A set
/// of memories that link to each other has no write order in which every
/// intermediate revision resolves every link, so checking those rules per write
/// on a branch would make such a set impossible to write at all.
///
/// The rules this governs are the ones answered from other files:
/// [`ValidationError::UnknownScope`], [`ValidationError::UnknownImpliesTarget`],
/// [`ValidationError::MissingLinkTarget`] and
/// [`ValidationError::LinkCrossesSessionSilo`]. Every rule one document answers
/// by itself is checked by every write, on a branch as on `main`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossDocumentRules {
    /// Checked by this write. Every commit on `main` is a revision sessions
    /// read, so it has to be consistent on its own.
    CheckedNow,
    /// Left to the land of the branch this write is on.
    DeferredToLanding,
}

impl CrossDocumentRules {
    fn are_checked(self) -> bool {
        self == CrossDocumentRules::CheckedNow
    }
}

/// Validate one document against the current catalog.
///
/// This is the check the write path runs before committing: rules that involve
/// other files (unknown scopes, link targets, the session silo) are answered
/// from `catalog`, and the candidate itself is not in it yet. `cross_document`
/// says whether those rules are answered here or at landing.
pub fn validate_write(
    catalog: &Catalog,
    candidate: Candidate<'_>,
    mode: WriteMode,
    cross_document: CrossDocumentRules,
) -> ValidationReport {
    let mut report = ValidationReport::default();
    match candidate {
        Candidate::Memory { document } => {
            let path = document.id.repository_path();
            if mode == WriteMode::Create && catalog.memory(&document.id).is_some() {
                report.push(ValidationError::DuplicateMemoryId {
                    path: path.clone(),
                    id: document.id.clone(),
                });
            }
            validate_memory_document(catalog, &path, document, cross_document, &mut report);
        }
        Candidate::Scope { id, document } => {
            let path = id.repository_path();
            validate_scope_document(catalog, &path, id, document, cross_document, &mut report);
        }
    }
    report
}

fn validate_scope_document(
    catalog: &Catalog,
    path: &str,
    id: &ScopeId,
    document: &ScopeDocument,
    cross_document: CrossDocumentRules,
    report: &mut ValidationReport,
) {
    if !is_valid_scope_file_id(id.as_str()) {
        report.push(ValidationError::InvalidScopeId {
            path: path.to_string(),
            id: id.as_str().to_string(),
        });
    }
    if &document.id != id {
        report.push(ValidationError::ScopeIdMismatch {
            path: path.to_string(),
            declared: document.id.clone(),
            expected: id.clone(),
        });
    }
    for target in &document.implies {
        if cross_document.are_checked() && !is_known_scope(catalog, target) {
            report.push(ValidationError::UnknownImpliesTarget {
                path: path.to_string(),
                scope: id.clone(),
                target: target.clone(),
            });
        }
    }
    for trigger in &document.triggers {
        if let Err(error) = regex::Regex::new(&trigger.pattern) {
            report.push(ValidationError::InvalidTriggerPattern {
                path: path.to_string(),
                scope: id.clone(),
                field: trigger.field(),
                pattern: trigger.pattern.clone(),
                message: error.to_string(),
            });
        }
    }
}

fn validate_memory_document(
    catalog: &Catalog,
    path: &str,
    document: &MemoryDocument,
    cross_document: CrossDocumentRules,
    report: &mut ValidationReport,
) {
    let id = &document.id;
    // `name` is what a person sees and what the file claims to be; the id is
    // what every link and every API call resolves by. They have to agree.
    if document.name() != id.name() {
        report.push(ValidationError::MemoryNameMismatch {
            path: path.to_string(),
            declared: document.name().to_string(),
            expected: id.name().to_string(),
        });
    }
    if !document.has_description() {
        report.warn(ValidationWarning::MissingDescription {
            path: path.to_string(),
            memory: id.clone(),
        });
    }
    for scope in document.scopes() {
        if cross_document.are_checked() && !is_known_scope(catalog, scope) {
            report.push(ValidationError::UnknownScope {
                path: path.to_string(),
                memory: id.clone(),
                scope: scope.clone(),
            });
        }
    }
    if let Some((machine, session_id)) = id.session_key().and_then(|key| key.split_once('/')) {
        // A memory in a silo belongs to exactly one session; extra scopes would
        // deliver it to contexts that cannot see the silo it links inside.
        let expected = ScopeId::session(machine, session_id);
        if document.scopes() != [expected.clone()] {
            report.push(ValidationError::SessionMemoryScopes {
                path: path.to_string(),
                memory: id.clone(),
                expected,
                found: join_scopes(document.scopes()),
            });
        }
    }
    if !cross_document.are_checked() {
        // Whether a link resolves and whether it crosses a silo are questions
        // only the other files answer, so a branch write leaves both to the
        // land, where the whole tree is there to answer them.
        return;
    }
    for target in document.links() {
        match resolve_link(catalog, id, &target) {
            None => report.push(ValidationError::MissingLinkTarget {
                path: path.to_string(),
                memory: id.clone(),
                target,
            }),
            Some(resolved) => {
                if let Some(silo) = resolved.session_silo()
                    && id.session_silo() != Some(silo)
                {
                    report.push(ValidationError::LinkCrossesSessionSilo {
                        path: path.to_string(),
                        memory: id.clone(),
                        target: resolved.clone(),
                        silo: silo.to_string(),
                    });
                }
            }
        }
    }
}

/// The memory a `[[target]]` in `from` refers to.
///
/// From inside a session silo a bare name means the memory of that name in the
/// same silo, which is what a session note writing about its own siblings
/// intends; otherwise the target is a memory id.
pub fn resolve_link(catalog: &Catalog, from: &MemoryId, target: &str) -> Option<MemoryId> {
    if let Some(sibling) = from.sibling_in_silo(target)
        && catalog.memory(&sibling).is_some()
    {
        return Some(sibling);
    }
    let direct = MemoryId::new(target);
    catalog.memory(&direct).map(|entry| entry.id.clone())
}

/// Whether `scope` exists: implicit scopes always do, others need a file.
pub fn is_known_scope(catalog: &Catalog, scope: &ScopeId) -> bool {
    scope.is_implicit() || catalog.scope(scope).is_some()
}

fn join_scopes(scopes: &[ScopeId]) -> String {
    scopes
        .iter()
        .map(ScopeId::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}
