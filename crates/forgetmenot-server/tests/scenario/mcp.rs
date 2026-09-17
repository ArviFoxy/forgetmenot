//! The MCP tools, called the way Claude Code calls them.
//!
//! A tool call from the model is three things, not one: the `PreToolUse` that
//! may stop it, the call itself, and the `PostToolUse` carrying what it
//! answered. Every method here does all three, so a scenario about a tool's
//! effect also covers what the memory system's own traffic does to the session
//! it runs in.

use forgetmenot_server::store::MemoryId;
use serde_json::{Map, Value, json};

use crate::common::{McpSession, tool_text};
use crate::scenario::answer::Answer;
use crate::scenario::session::Session;
use crate::scenario::store::MemoryBuilder;

/// The prefix Claude Code gives a tool of the server the user registered as
/// `forgetmenot`, which is the name the example configuration uses.
pub const TOOL_PREFIX: &str = "mcp__forgetmenot__";

/// The tools over one session's MCP connection.
pub struct Mcp<'s, 'w> {
    session: &'s Session<'w>,
    client: McpSession<'w>,
}

/// What a wrapped tool call did.
pub enum McpOutcome<'w> {
    /// The `PreToolUse` stopped the call, so the tool was never called and
    /// there is no `PostToolUse`. Calling the same method again is the model
    /// issuing the call a second time.
    Held(Answer<'w>),
    /// The call ran.
    Done {
        /// What the tool answered, as the JSON it reports. A tool that refused
        /// answers a message rather than JSON, which arrives here as a string.
        answer: Value,
        /// The answer to the `PostToolUse` that reported the tool's text.
        post: Answer<'w>,
    },
}

/// One open branch, on which every write is made with the branch's name.
pub struct Branch<'m, 's, 'w> {
    mcp: &'m Mcp<'s, 'w>,
    name: String,
}

impl<'s, 'w> Mcp<'s, 'w> {
    pub(crate) fn new(session: &'s Session<'w>) -> Self {
        Self {
            session,
            client: session.world().server().mcp(),
        }
    }

    // -- reading -----------------------------------------------------------

    pub fn memory_index(&self) -> McpOutcome<'w> {
        self.wrapped("memory_index", json!({}))
    }

    /// Read one memory, naming this session, so the body counts as delivered
    /// here and is not repeated at the next event.
    pub fn memory_get(&self, id: &str) -> McpOutcome<'w> {
        self.wrapped(
            "memory_get",
            json!({ "id": id, "session_key": self.session.key() }),
        )
    }

    pub fn memory_history(&self, id: &str) -> McpOutcome<'w> {
        self.wrapped("memory_history", json!({ "id": id }))
    }

    pub fn memory_blame(&self, id: &str) -> McpOutcome<'w> {
        self.wrapped("memory_blame", json!({ "id": id }))
    }

    pub fn settings_get(&self) -> McpOutcome<'w> {
        self.wrapped("settings_get", json!({}))
    }

    pub fn session_scopes(&self) -> McpOutcome<'w> {
        self.wrapped("session_scopes", self.with_session_key(json!({})))
    }

    // -- writing -----------------------------------------------------------

    /// Write one memory, described by the same builder a store is built from.
    pub fn memory_put(&self, id: &str, build: impl FnOnce(&mut MemoryBuilder)) -> McpOutcome<'w> {
        self.put(id, build, None)
    }

    pub fn memory_replace_text(
        &self,
        id: &str,
        old_string: &str,
        new_string: &str,
    ) -> McpOutcome<'w> {
        self.replace_text(id, old_string, new_string, None)
    }

    /// Set some of a memory's fields, leaving the body and the rest alone.
    /// `fields` carries the ones to set, as the tool takes them.
    pub fn memory_set_fields(&self, id: &str, fields: Value) -> McpOutcome<'w> {
        self.set_fields(id, fields, None)
    }

    pub fn memory_rename(&self, from: &str, to: &str) -> McpOutcome<'w> {
        self.rename(from, to, None)
    }

    pub fn memory_delete(&self, id: &str) -> McpOutcome<'w> {
        self.delete(id, None)
    }

    pub fn settings_set(&self, key: &str, value: Value) -> McpOutcome<'w> {
        self.set_setting(key, value, None)
    }

    // -- this session's scopes --------------------------------------------

    pub fn session_scope_on<Id: AsRef<str>>(
        &self,
        scopes: impl IntoIterator<Item = Id>,
    ) -> McpOutcome<'w> {
        let params = self.with_session_key(json!({ "scopes": ids(scopes) }));
        self.wrapped("session_scope_on", params)
    }

    pub fn session_scope_off<Id: AsRef<str>>(
        &self,
        scopes: impl IntoIterator<Item = Id>,
    ) -> McpOutcome<'w> {
        let params = self.with_session_key(json!({ "scopes": ids(scopes) }));
        self.wrapped("session_scope_off", params)
    }

    /// Take over another session's scopes, including its own session scope.
    pub fn session_inherit(&self, from_session_key: &str) -> McpOutcome<'w> {
        let params = self.with_session_key(json!({ "from_session_key": from_session_key }));
        self.wrapped("session_inherit", params)
    }

    // -- branches ----------------------------------------------------------

    /// Open a branch, on which writes change nothing any session is delivered
    /// until it lands.
    pub fn branch(&self) -> Branch<'_, 's, 'w> {
        let opened = self
            .wrapped("branch_create", self.with_session_key(json!({})))
            .expect_ok();
        let name = opened.json()["branch"]
            .as_str()
            .expect("branch_create answers with the branch's name")
            .to_string();
        Branch { mcp: self, name }
    }

    pub fn branch_list(&self) -> McpOutcome<'w> {
        self.wrapped("branch_list", json!({}))
    }

    pub fn branch_diff(&self, branch: &str) -> McpOutcome<'w> {
        self.wrapped("branch_diff", json!({ "branch": branch }))
    }

    // -- the escape hatch --------------------------------------------------

    /// Call one tool by name with the parameters given, wrapped in the two hook
    /// events like every other call here. Nothing is filled in.
    pub fn call(&self, tool: &str, params: Value) -> McpOutcome<'w> {
        self.wrapped(tool, params)
    }

    // -- the wrapping ------------------------------------------------------

    fn wrapped(&self, tool: &str, params: Value) -> McpOutcome<'w> {
        let name = format!("{TOOL_PREFIX}{tool}");
        let call = self.session.tool(&name, params.clone());
        if call.held() {
            return McpOutcome::Held(call.into_answer());
        }
        let result = self.client.call(tool, params);
        let text = tool_text(&result);
        let answer = serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.clone()));
        let post = call.result(Value::String(text));
        McpOutcome::Done { answer, post }
    }

    fn with_session_key(&self, params: Value) -> Value {
        let mut params = as_object(params);
        params.insert("session_key".to_string(), json!(self.session.key()));
        Value::Object(params)
    }

    /// The parameters every write takes: who wrote it, why, and the branch it
    /// is on when it is on one.
    fn write_params(&self, params: Value, message: &str, branch: Option<&str>) -> Value {
        let mut params = as_object(self.with_session_key(params));
        params.insert("message".to_string(), json!(message));
        if let Some(branch) = branch {
            params.insert("branch".to_string(), json!(branch));
        }
        Value::Object(params)
    }

    fn put(
        &self,
        id: &str,
        build: impl FnOnce(&mut MemoryBuilder),
        branch: Option<&str>,
    ) -> McpOutcome<'w> {
        let mut memory = MemoryBuilder::new(MemoryId::new(id));
        build(&mut memory);
        let mut params = json!({
            "id": id,
            "description": memory.description_text(),
            "kind": kind_name(&memory),
            "scopes": memory
                .scope_ids()
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>(),
            "source": memory.source_text(),
            "body": memory.body_text(),
        });
        // A write to a memory that exists carries the version it is written
        // over. It is read from the store rather than through another tool
        // call, so that one write is one call in the session's event stream.
        if let Some(version) = self.version_of(id) {
            params["base_version"] = json!(version);
        }
        let params = self.write_params(params, &format!("write {id}"), branch);
        self.wrapped("memory_put", params)
    }

    fn replace_text(
        &self,
        id: &str,
        old_string: &str,
        new_string: &str,
        branch: Option<&str>,
    ) -> McpOutcome<'w> {
        let params = self.write_params(
            json!({ "id": id, "old_string": old_string, "new_string": new_string }),
            &format!("edit {id}"),
            branch,
        );
        self.wrapped("memory_replace_text", params)
    }

    fn set_fields(&self, id: &str, fields: Value, branch: Option<&str>) -> McpOutcome<'w> {
        let mut params = as_object(fields);
        params.insert("id".to_string(), json!(id));
        let params = self.write_params(
            Value::Object(params),
            &format!("set fields of {id}"),
            branch,
        );
        self.wrapped("memory_set_fields", params)
    }

    fn rename(&self, from: &str, to: &str, branch: Option<&str>) -> McpOutcome<'w> {
        let params = self.write_params(
            json!({ "from": from, "to": to }),
            &format!("rename {from} to {to}"),
            branch,
        );
        self.wrapped("memory_rename", params)
    }

    fn delete(&self, id: &str, branch: Option<&str>) -> McpOutcome<'w> {
        let params = self.write_params(json!({ "id": id }), &format!("delete {id}"), branch);
        self.wrapped("memory_delete", params)
    }

    fn set_setting(&self, key: &str, value: Value, branch: Option<&str>) -> McpOutcome<'w> {
        let params = self.write_params(
            json!({ "key": key, "value": value }),
            &format!("set {key}"),
            branch,
        );
        self.wrapped("settings_set", params)
    }

    /// The version the store holds one memory at, or `None` when it holds none.
    fn version_of(&self, id: &str) -> Option<String> {
        self.session
            .world()
            .catalog()
            .memory(&MemoryId::new(id))
            .map(|entry| entry.version.to_string())
    }
}

impl<'w> Branch<'_, '_, 'w> {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn memory_put(&self, id: &str, build: impl FnOnce(&mut MemoryBuilder)) -> McpOutcome<'w> {
        self.mcp.put(id, build, Some(&self.name))
    }

    pub fn memory_replace_text(
        &self,
        id: &str,
        old_string: &str,
        new_string: &str,
    ) -> McpOutcome<'w> {
        self.mcp
            .replace_text(id, old_string, new_string, Some(&self.name))
    }

    pub fn memory_set_fields(&self, id: &str, fields: Value) -> McpOutcome<'w> {
        self.mcp.set_fields(id, fields, Some(&self.name))
    }

    pub fn memory_rename(&self, from: &str, to: &str) -> McpOutcome<'w> {
        self.mcp.rename(from, to, Some(&self.name))
    }

    pub fn memory_delete(&self, id: &str) -> McpOutcome<'w> {
        self.mcp.delete(id, Some(&self.name))
    }

    pub fn settings_set(&self, key: &str, value: Value) -> McpOutcome<'w> {
        self.mcp.set_setting(key, value, Some(&self.name))
    }

    /// Squash everything written on the branch onto main as one commit.
    pub fn land(self, title: &str) -> McpOutcome<'w> {
        let params = self
            .mcp
            .write_params(json!({ "branch": self.name }), title, None);
        self.mcp.wrapped("branch_land", params)
    }

    /// Throw the branch away with everything on it.
    pub fn abandon(self) -> McpOutcome<'w> {
        self.mcp
            .wrapped("branch_abandon", json!({ "branch": self.name }))
    }
}

impl<'w> McpOutcome<'w> {
    /// What the tool answered, failing the test when the call never ran.
    pub fn json(&self) -> &Value {
        match self {
            McpOutcome::Done { answer, .. } => answer,
            McpOutcome::Held(_) => panic!("the call was stopped, so the tool answered nothing"),
        }
    }

    /// Check the answer to the `PostToolUse` that reported the tool's text.
    pub fn assert_post(self, check: impl FnOnce(&Answer<'w>)) -> Self {
        match self {
            McpOutcome::Done { answer, post } => McpOutcome::Done {
                answer,
                post: post.assert(check),
            },
            McpOutcome::Held(_) => panic!("the call was stopped, so there was no PostToolUse"),
        }
    }

    /// Require that the call ran and the tool did the work.
    ///
    /// A tool that refused answers a message rather than the JSON it reports on
    /// success, which is what this tells apart.
    pub fn expect_ok(self) -> Self {
        match &self {
            McpOutcome::Held(_) => panic!("the call was stopped before the tool ran"),
            McpOutcome::Done { answer, .. } => {
                assert!(
                    !answer.is_string(),
                    "the tool refused the call: {}",
                    answer.as_str().unwrap_or_default()
                );
            }
        }
        self
    }
}

fn as_object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("tool parameters are a JSON object, not {other}"),
    }
}

fn ids<Id: AsRef<str>>(values: impl IntoIterator<Item = Id>) -> Value {
    Value::Array(values.into_iter().map(|id| json!(id.as_ref())).collect())
}

/// The name the tools take for a memory's kind, which is how the kind is
/// written in a memory's file.
fn kind_name(memory: &MemoryBuilder) -> String {
    serde_json::to_value(memory.kind())
        .expect("a memory kind serialises")
        .as_str()
        .expect("a memory kind is a string")
        .to_string()
}
