//! The world a scenario runs in: a server on a store, the clock, the writes
//! made behind the server's back, and the reads a scenario checks through.

use std::collections::BTreeMap;
use std::path::Path;

use forgetmenot_server::store::catalog::Catalog;
use forgetmenot_server::store::settings::SETTINGS_PATH;
use forgetmenot_server::store::{MemoryId, ScopeId};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::common::TestServer;
use crate::scenario::claude::{CLIENT, Claude, HTTP, Transport};
use crate::scenario::store::{Store, StoreBuilder};

/// A running server on a store, with the transcripts of every session it has
/// seen. Everything is removed when the world is dropped.
pub struct World {
    server: TestServer,
    transport: &'static dyn Transport,
    /// Where the sessions' transcripts and subagent metadata files are written.
    transcripts: TempDir,
}

/// The world a scenario asks for, before it is started.
pub struct WorldBuilder {
    store: StoreBuilder,
    settings: SettingsBuilder,
    transport: &'static dyn Transport,
}

impl World {
    /// Start describing a world. The default is the example store, the store's
    /// own settings, and events posted straight to `/hook`.
    ///
    /// A world is described before it exists, so this hands back the builder
    /// rather than a world.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> WorldBuilder {
        WorldBuilder {
            store: Store::example(),
            settings: SettingsBuilder::default(),
            transport: &HTTP,
        }
    }

    /// The Claude Code running on one machine.
    pub fn claude(&self, machine: &str) -> Claude<'_> {
        Claude::new(self, machine)
    }

    /// Commit to the store the way a person with a shell does: the server is
    /// not told, and every context meets the change at its next event.
    ///
    /// A file with no content is removed.
    pub fn edit_outside(&self, title: &str, files: Vec<(String, Option<Vec<u8>>)>) {
        self.server.commit(title, files);
    }

    /// Replace one memory's body outside the server, keeping everything else
    /// the file says about it.
    pub fn rewrite_memory(&self, id: &str, body: &str) {
        let id = MemoryId::new(id);
        let catalog = self.catalog();
        let entry = catalog
            .memory(&id)
            .unwrap_or_else(|| panic!("the store holds no memory {id}"));
        let mut document = entry.document.clone();
        document.body = body.to_string();
        let text = document.render().expect("the rewritten memory renders");
        self.edit_outside(
            &format!("rewrite {id}"),
            vec![(id.repository_path(), Some(text.into_bytes()))],
        );
    }

    /// Remove one file from the store outside the server.
    pub fn delete_file(&self, path: &str) {
        self.edit_outside(&format!("remove {path}"), vec![(path.to_string(), None)]);
    }

    /// Stop the server and start it again on the same store, state file and
    /// statistics database.
    ///
    /// Takes the world by exclusive reference, so no session may be held across
    /// the call; a session carries on after a restart by being picked up again
    /// with [`Claude::session_named`], which keeps writing the transcript it was
    /// writing before.
    pub fn restart(&mut self) {
        self.server.restart();
    }

    /// Move the clock forward, so what follows is recorded later than what came
    /// before it.
    pub fn advance(&self, by: chrono::Duration) {
        self.server.advance(by);
    }

    /// Every context the server knows about, as the contexts page reads them.
    pub fn contexts(&self) -> Vec<Value> {
        let (status, body) = self.api("GET", "/api/contexts", None);
        assert_eq!(status, 200, "the contexts must be readable, got {body}");
        body.as_array()
            .expect("the contexts are a JSON array")
            .clone()
    }

    /// One context by its session key, failing the test when the server has not
    /// seen it.
    pub fn context(&self, key: &str) -> Value {
        self.contexts()
            .into_iter()
            .find(|row| row["key"] == json!(key))
            .unwrap_or_else(|| panic!("the server has seen no context {key}"))
    }

    /// What the statistics recorded about one memory's deliveries: one row per
    /// memory that was ever delivered or fetched, so no row at all when nothing
    /// about it was ever sent anywhere.
    ///
    /// Read from the statistics reader, which flushes what the answered events
    /// queued before it answers.
    pub fn stats_deliveries_of(&self, memory_id: &str) -> Vec<Value> {
        let rows = self
            .server
            .stats()
            .memory_stats()
            .expect("the statistics are readable");
        rows.into_iter()
            .filter(|row| row.memory == memory_id)
            .map(|row| serde_json::to_value(row).expect("a statistics row is JSON"))
            .collect()
    }

    /// What the statistics recorded about one scope: how often it was turned on
    /// and how often it turned itself off, with the live contexts working in it.
    ///
    /// A scope nothing has happened to still has a row, with everything zero.
    pub fn stats_scope(&self, scope_id: &str) -> Value {
        let active_sets: Vec<std::collections::BTreeSet<ScopeId>> = self
            .contexts()
            .iter()
            .map(|row| {
                row["active_scopes"]
                    .as_array()
                    .expect("a context reports its active scopes as an array")
                    .iter()
                    .map(|scope| {
                        ScopeId::new(scope.as_str().expect("a scope id is a string").to_string())
                    })
                    .collect()
            })
            .collect();
        let rows = self
            .server
            .stats()
            .scope_stats(&active_sets)
            .expect("the statistics are readable");
        rows.into_iter()
            .find(|row| row.scope_id == scope_id)
            .map(|row| serde_json::to_value(row).expect("a statistics row is JSON"))
            .unwrap_or_else(|| panic!("the statistics know no scope {scope_id}"))
    }

    /// One request to the JSON API, with its status and its answer.
    pub fn api(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        self.server.api(method, path, body)
    }

    /// The catalog of the store as it stands, which is what the delivery
    /// matchers read a memory's body and description from.
    pub(crate) fn catalog(&self) -> Catalog {
        self.server.store().catalog()
    }

    pub(crate) fn url(&self) -> String {
        self.server.url()
    }

    pub(crate) fn server(&self) -> &TestServer {
        &self.server
    }

    pub(crate) fn transport(&self) -> &'static dyn Transport {
        self.transport
    }

    /// The directory the sessions' transcripts live in.
    pub(crate) fn transcripts(&self) -> &Path {
        self.transcripts.path()
    }
}

impl WorldBuilder {
    /// The store the world starts with.
    pub fn store(mut self, store: StoreBuilder) -> Self {
        self.store = store;
        self
    }

    /// The store's behaviour settings, written into its `config.yml` over
    /// whatever the store already says.
    pub fn settings(mut self, set: impl FnOnce(&mut SettingsBuilder)) -> Self {
        set(&mut self.settings);
        self
    }

    /// Send every event through the real `forgetmenot-hook` binary against the
    /// session's transcript, instead of posting the body the client would build.
    pub fn through_client(mut self) -> Self {
        self.transport = &CLIENT;
        self
    }

    /// Start the server.
    pub fn build(self) -> World {
        let mut store = self.store;
        if let Some(text) = self.settings.over(store.text(SETTINGS_PATH).as_deref()) {
            store = store.file(SETTINGS_PATH, text);
        }
        World {
            server: TestServer::start(store.files(), |_| {}),
            transport: self.transport,
            transcripts: TempDir::new().expect("a temporary directory for the transcripts"),
        }
    }
}

/// The store's behaviour settings a scenario sets.
///
/// Only the keys a scenario names are written; the rest of the store's
/// `config.yml` is left as it was, so a scenario that sets one key does not
/// silently reset the others.
#[derive(Clone, Debug, Default)]
pub struct SettingsBuilder {
    values: BTreeMap<String, Value>,
}

impl SettingsBuilder {
    /// Deliver everything that applies again after this many context tokens;
    /// `None` turns reminders off.
    pub fn reminder_tokens(&mut self, tokens: Option<u64>) -> &mut Self {
        self.set("reminder_tokens", json!(tokens))
    }

    /// Hold a tool call when a critical memory is due and unseen.
    pub fn interrupt_on_critical(&mut self, hold: bool) -> &mut Self {
        self.set("interrupt_on_critical", json!(hold))
    }

    /// Tool names that are never held.
    pub fn interrupt_exempt_tools<Name: AsRef<str>>(
        &mut self,
        tools: impl IntoIterator<Item = Name>,
    ) -> &mut Self {
        self.set("interrupt_exempt_tools", names(tools))
    }

    /// Tool names whose inputs and results are never matched against triggers.
    pub fn trigger_exempt_tools<Name: AsRef<str>>(
        &mut self,
        tools: impl IntoIterator<Item = Name>,
    ) -> &mut Self {
        self.set("trigger_exempt_tools", names(tools))
    }

    /// Whether a subagent starts with its parent's active scopes.
    pub fn subagents_inherit_scopes(&mut self, inherit: bool) -> &mut Self {
        self.set("subagents_inherit_scopes", json!(inherit))
    }

    /// Whether the descriptions of knowledge memories are delivered.
    pub fn deliver_knowledge_index(&mut self, deliver: bool) -> &mut Self {
        self.set("deliver_knowledge_index", json!(deliver))
    }

    /// Bytes of a tool result matched against triggers.
    pub fn tool_result_match_limit(&mut self, bytes: u64) -> &mut Self {
        self.set("tool_result_match_limit", json!(bytes))
    }

    /// Characters of an answer above which it opens with the notice to read the
    /// file Claude Code saved it to; `None` sends no notice.
    pub fn answer_file_threshold(&mut self, characters: Option<u64>) -> &mut Self {
        self.set("answer_file_threshold", json!(characters))
    }

    /// Name a scope that became active with nothing to deliver.
    pub fn announce_empty_scopes(&mut self, announce: bool) -> &mut Self {
        self.set("announce_empty_scopes", json!(announce))
    }

    fn set(&mut self, key: &str, value: Value) -> &mut Self {
        self.values.insert(key.to_string(), value);
        self
    }

    /// The settings file's text with these keys written over `existing`, or
    /// `None` when the scenario named no setting at all.
    fn over(&self, existing: Option<&str>) -> Option<String> {
        if self.values.is_empty() {
            return None;
        }
        let mut mapping: yaml_serde::Mapping = match existing {
            Some(text) => yaml_serde::from_str(text).expect("the store's settings file parses"),
            None => yaml_serde::Mapping::new(),
        };
        for (key, value) in &self.values {
            let value: yaml_serde::Value =
                yaml_serde::to_value(value).expect("a setting value is writable as YAML");
            mapping.insert(yaml_serde::Value::String(key.clone()), value);
        }
        Some(yaml_serde::to_string(&mapping).expect("the settings file renders"))
    }
}

fn names<Name: AsRef<str>>(values: impl IntoIterator<Item = Name>) -> Value {
    Value::Array(
        values
            .into_iter()
            .map(|name| json!(name.as_ref()))
            .collect(),
    )
}
