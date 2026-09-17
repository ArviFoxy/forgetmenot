//! The store a scenario starts from.
//!
//! A builder describes memories and scopes as values and renders them through
//! the crate's own document types, so a builder cannot produce a file the server
//! would refuse to load or would load as something else.

use std::collections::BTreeMap;

use forgetmenot_server::store::memory::{
    MemoryDocument, MemoryFrontmatter, MemoryKind, MemoryMetadata, MemorySource,
};
use forgetmenot_server::store::scope::{Forget, ScopeDocument, Trigger};
use forgetmenot_server::store::{MemoryId, ScopeId};

pub use forgetmenot_server::store::scope::TriggerField as Field;

use crate::common;

/// The store a scenario runs on: the files, in repository paths.
#[derive(Clone, Debug, Default)]
pub struct StoreBuilder {
    files: BTreeMap<String, Vec<u8>>,
}

/// Where a store starts from.
pub struct Store;

impl Store {
    /// A store with no files at all, which is a store nobody has written to.
    pub fn empty() -> StoreBuilder {
        StoreBuilder::default()
    }

    /// A store holding the files of `examples/store`.
    pub fn example() -> StoreBuilder {
        let mut files = BTreeMap::new();
        for (path, bytes) in common::example_store_files() {
            if let Some(bytes) = bytes {
                files.insert(path, bytes);
            }
        }
        StoreBuilder { files }
    }
}

impl StoreBuilder {
    /// Add or replace one memory, described by the builder `build` fills in.
    pub fn memory(mut self, id: &str, build: impl FnOnce(&mut MemoryBuilder)) -> Self {
        let id = MemoryId::new(id);
        let mut memory = MemoryBuilder::new(id.clone());
        build(&mut memory);
        self.files.insert(id.repository_path(), memory.bytes());
        self
    }

    /// Add or replace one scope, described by the builder `build` fills in.
    pub fn scope(mut self, id: &str, build: impl FnOnce(&mut ScopeBuilder)) -> Self {
        let id = ScopeId::new(id);
        let mut scope = ScopeBuilder::new(id.clone());
        build(&mut scope);
        self.files.insert(id.repository_path(), scope.bytes());
        self
    }

    /// Add or replace one file by its repository path, for the files that are
    /// neither a memory nor a scope and for text no builder would write.
    pub fn file(mut self, path: &str, bytes: impl Into<Vec<u8>>) -> Self {
        self.files.insert(path.to_string(), bytes.into());
        self
    }

    /// Leave one file out of the store.
    pub fn without(mut self, path: &str) -> Self {
        self.files.remove(path);
        self
    }

    /// The text of one file the builder holds, or `None` when it holds none.
    pub(crate) fn text(&self, path: &str) -> Option<String> {
        let bytes = self.files.get(path)?;
        Some(String::from_utf8(bytes.clone()).expect("a store file this builder wrote is utf-8"))
    }

    /// The files as the server's commit path takes them.
    pub(crate) fn files(&self) -> Vec<(String, Option<Vec<u8>>)> {
        self.files
            .iter()
            .map(|(path, bytes)| (path.clone(), Some(bytes.clone())))
            .collect()
    }
}

/// One memory of the store.
///
/// The defaults are the ones a file that says nothing gets: a knowledge memory
/// in `global`, written by the assistant.
#[derive(Clone, Debug)]
pub struct MemoryBuilder {
    id: MemoryId,
    description: String,
    kind: MemoryKind,
    scopes: Vec<ScopeId>,
    source: String,
    body: String,
}

impl MemoryBuilder {
    pub(crate) fn new(id: MemoryId) -> Self {
        Self {
            id,
            description: String::new(),
            kind: MemoryKind::Knowledge,
            scopes: vec![ScopeId::global()],
            source: "assistant".to_string(),
            body: String::new(),
        }
    }

    /// Delivered in full, and able to stop a tool call.
    pub fn critical(&mut self) -> &mut Self {
        self.kind = MemoryKind::Critical;
        self
    }

    /// Delivered as an index line the model can fetch the body by.
    pub fn knowledge(&mut self) -> &mut Self {
        self.kind = MemoryKind::Knowledge;
        self
    }

    pub fn scopes<Id: AsRef<str>>(&mut self, scopes: impl IntoIterator<Item = Id>) -> &mut Self {
        self.scopes = scopes
            .into_iter()
            .map(|scope| ScopeId::new(scope.as_ref()))
            .collect();
        self
    }

    pub fn description(&mut self, description: &str) -> &mut Self {
        self.description = description.to_string();
        self
    }

    pub fn body(&mut self, body: &str) -> &mut Self {
        self.body = body.to_string();
        self
    }

    pub fn source(&mut self, source: &str) -> &mut Self {
        self.source = source.to_string();
        self
    }

    pub(crate) fn id(&self) -> &MemoryId {
        &self.id
    }

    pub(crate) fn description_text(&self) -> &str {
        &self.description
    }

    pub(crate) fn kind(&self) -> MemoryKind {
        self.kind
    }

    pub(crate) fn scope_ids(&self) -> &[ScopeId] {
        &self.scopes
    }

    pub(crate) fn source_text(&self) -> &str {
        &self.source
    }

    pub(crate) fn body_text(&self) -> &str {
        &self.body
    }

    /// The memory as a document, which is what decides the file's text.
    pub(crate) fn document(&self) -> MemoryDocument {
        MemoryDocument {
            id: self.id.clone(),
            frontmatter: MemoryFrontmatter {
                name: self.id.name().to_string(),
                description: Some(self.description.clone()).filter(|text| !text.is_empty()),
                modified: None,
                extra: yaml_serde::Mapping::new(),
                metadata: MemoryMetadata {
                    kind: Some(self.kind),
                    scopes: Some(self.scopes.clone()),
                    source: Some(MemorySource::new(self.source.clone())),
                    created: None,
                    author: None,
                    extra: yaml_serde::Mapping::new(),
                },
            },
            body: self.body.clone(),
        }
    }

    fn bytes(&self) -> Vec<u8> {
        self.document()
            .render()
            .expect("a memory this builder describes renders to a file")
            .into_bytes()
    }
}

/// One scope of the store.
#[derive(Clone, Debug)]
pub struct ScopeBuilder {
    document: ScopeDocument,
}

impl ScopeBuilder {
    pub(crate) fn new(id: ScopeId) -> Self {
        Self {
            document: ScopeDocument {
                id,
                message: None,
                implies: Vec::new(),
                triggers: Vec::new(),
                forget: None,
            },
        }
    }

    /// Turn this scope on wherever `pattern` matches the text of `field`.
    pub fn trigger(&mut self, field: Field, pattern: &str) -> &mut Self {
        self.document.triggers.push(Trigger {
            on: Some(field),
            pattern: pattern.to_string(),
            machine: None,
        });
        self
    }

    /// The same, but only for sessions running on `machine`.
    pub fn trigger_on_machine(&mut self, field: Field, pattern: &str, machine: &str) -> &mut Self {
        self.document.triggers.push(Trigger {
            on: Some(field),
            pattern: pattern.to_string(),
            machine: Some(machine.to_string()),
        });
        self
    }

    /// Scopes that are on whenever this one is.
    pub fn implies<Id: AsRef<str>>(&mut self, scopes: impl IntoIterator<Item = Id>) -> &mut Self {
        self.document.implies = scopes
            .into_iter()
            .map(|scope| ScopeId::new(scope.as_ref()))
            .collect();
        self
    }

    /// Text delivered in full whenever this scope becomes active.
    pub fn message(&mut self, message: &str) -> &mut Self {
        self.document.message = Some(message.to_string());
        self
    }

    /// Turn this scope off again after this many context tokens without a
    /// trigger.
    pub fn forget_after(&mut self, tokens: u64) -> &mut Self {
        self.document.forget = Some(Forget {
            tokens_since_trigger: tokens,
        });
        self
    }

    fn bytes(&self) -> Vec<u8> {
        self.document
            .render()
            .expect("a scope this builder describes renders to a file")
            .into_bytes()
    }
}
