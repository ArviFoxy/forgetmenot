//! Tests of the MCP tools, driven by a real rmcp streamable HTTP client against a
//! real server on a temporary copy of the example store.
//!
//! These test the adapter: that every tool is there, that each says which family
//! it belongs to, that a tool's parameters reach the operation it stands for, and
//! that a failure comes back as something the model can act on. What the
//! operations themselves guarantee is tested in `operations_test.rs`,
//! `api_test.rs` and `branch_test.rs`; what is asserted here is only visible
//! through MCP.
//!
//! The expectations come from the plan's three tool families and its state
//! machine, and from the example store committed in this repository.

mod common;

use std::collections::BTreeSet;

use forgetmenot_server::stats::Table;
use forgetmenot_server::store::MemoryId;
use forgetmenot_server::store::memory::MemoryDocument;
use rmcp::model::ErrorCode;
use rmcp::service::ServiceError;
use serde_json::{Value, json};

use common::{
    McpSession, TestServer, additional_context, example_store_files, hook_fixture,
    permission_decision, permission_decision_reason, test_agent, tool_json, tool_text,
};

/// The machine every session in these tests runs on.
const MACHINE: &str = "alpha";

/// The context size reported with the hook events used here, which are not about
/// staleness.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// The memory management tools, which read and change the store.
const MEMORY_TOOLS: [&str; 7] = [
    "memory_index",
    "memory_get",
    "memory_put",
    "memory_replace_text",
    "memory_set_fields",
    "memory_rename",
    "memory_delete",
];

/// The branch tools, which open, inspect and land a transaction.
const BRANCH_TOOLS: [&str; 5] = [
    "branch_create",
    "branch_list",
    "branch_diff",
    "branch_land",
    "branch_abandon",
];

/// The settings tools, which read and change the store's behaviour settings.
const SETTINGS_TOOLS: [&str; 2] = ["settings_get", "settings_set"];

/// The tools that write to the store, each of which takes an optional branch.
const WRITE_TOOLS: [&str; 6] = [
    "memory_put",
    "memory_replace_text",
    "memory_set_fields",
    "memory_rename",
    "memory_delete",
    "settings_set",
];

/// The session management tools, which change only the calling context.
const SESSION_TOOLS: [&str; 4] = [
    "session_scopes",
    "session_scope_on",
    "session_scope_off",
    "session_inherit",
];

/// What a memory tool's description has to say: a write to the store is a commit.
const MEMORY_FAMILY_PHRASE: &str = "git commit";

/// What a session tool's description has to say: the store is not involved.
const SESSION_FAMILY_PHRASE: &str = "never touches the store";

/// What a branch tool's description has to say: what a branch is for.
const BRANCH_FAMILY_PHRASE: &str = "one commit on main";

/// What a settings tool's description has to say: what these settings are.
const SETTINGS_FAMILY_PHRASE: &str = "behaviour settings";

// Lines that appear in one memory's body of the example store and nowhere else,
// so that "delivered in full" can be told apart from "named in an index line".
const WIDGET_NAMING_BODY: &str = "Downstream drawings cite part numbers by value";
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";
/// The index entry of the session memory in the example store's silo.
const SESSION_NOTES_DESCRIPTION: &str =
    "Working notes for the bracket rework, measured in millimetres";

/// A memory in the `workshop` scope, which the example store has none of, so
/// that a call naming two scopes has one memory of its own per scope. The
/// store's `widgets` implies `rocketry`, so those two are delivered together by
/// a call that names either of them.
const WORKSHOP_RULE: &str = concat!(
    "---\n",
    "name: workshop-tidy\n",
    "description: The bench is cleared before the next job is set up\n",
    "metadata:\n",
    "  kind: critical\n",
    "  scopes:\n",
    "  - workshop\n",
    "  source: user\n",
    "---\n",
    "# Clear the bench between jobs\n\nEvery tool goes back on the wall before the next job is set up.\n",
);

/// The line of [`WORKSHOP_RULE`] that appears in no other memory.
const WORKSHOP_RULE_BODY: &str = "Every tool goes back on the wall";

/// A memory file in the shape Claude Code's auto-memory writes: its own `type`,
/// a `source` that is neither of the two conventional values, the strength its
/// keeper marks rules with, and Claude Code's bookkeeping keys. The content is
/// invented; the shape is the one such a directory carries.
const CLAUDE_CODE_MEMORY: &str = concat!(
    "---\n",
    "name: lathe-collets\n",
    "description: Collets go back in the rack by size after every job\n",
    "metadata:\n",
    "  node_type: memory\n",
    "  type: feedback\n",
    "  source: derived\n",
    "  strength: hard\n",
    "  originSessionId: 6f3c0a12-7b41-4e2b-9a55-0c1d2e3f4a5b\n",
    "  modified: 2026-09-12T13:33:29.156Z\n",
    "---\n",
    "# Collets go back in the rack\n\nEvery collet goes back in its own slot, by size, before the\nnext job is set up.\n",
);

/// The keys of [`CLAUDE_CODE_MEMORY`] that forgetmenot does not interpret,
/// written out here so that what the import has to preserve is stated apart
/// from whatever the reader makes of the file.
fn claude_code_metadata() -> Value {
    json!({
        "node_type": "memory",
        "type": "feedback",
        "strength": "hard",
        "originSessionId": "6f3c0a12-7b41-4e2b-9a55-0c1d2e3f4a5b",
        "modified": "2026-09-12T13:33:29.156Z",
    })
}

/// The text a hook answer injects, or the empty string when it injects nothing.
fn context_of(answer: &Value) -> &str {
    additional_context(answer).unwrap_or_default()
}

/// One hook payload with some fields replaced, so that a fixture can be sent for
/// another session than the one it was recorded for.
fn event_with(fixture: &str, fields: &[(&str, Value)]) -> Value {
    let mut event = hook_fixture(fixture);
    let object = event.as_object_mut().expect("a hook payload is an object");
    for (key, value) in fields {
        object.insert((*key).to_string(), value.clone());
    }
    event
}

/// An event that matches no trigger in the example store, used to ask a context
/// what it is owed without changing which scopes it works in.
fn neutral_event(session_id: &str) -> Value {
    event_with(
        "pre_tool_use_read",
        &[
            ("session_id", json!(session_id)),
            ("tool_name", json!("Read")),
            (
                "tool_input",
                json!({ "file_path": "/home/dev/notes/README.md" }),
            ),
            ("cwd", json!("/home/dev/notes")),
        ],
    )
}

/// Whether `text` carries a line saying `id` was withdrawn for `reason`.
fn has_retracted_line(text: &str, id: &str, reason: &str) -> bool {
    text.lines()
        .any(|line| line.contains(id) && line.contains(reason))
}

/// Whether `text` carries an index line for `id` and its description.
fn has_index_line(text: &str, id: &str, description: &str) -> bool {
    text.lines()
        .any(|line| line.contains(id) && line.contains(description))
}

/// The store revision the server answers from, which every commit moves.
fn store_revision(server: &TestServer) -> String {
    let (status, health) = server.api("GET", "/api/health", None);
    assert_eq!(status, 200, "the server must report its store revision");
    health["head"]
        .as_str()
        .expect("the health answer names the store revision")
        .to_string()
}

/// The commits that changed one memory, newest first.
fn history(server: &TestServer, id: &str) -> Vec<Value> {
    let (status, commits) = server.api("GET", &format!("/api/memories/{id}/history"), None);
    assert_eq!(
        status, 200,
        "the history of {id} must be readable, got {commits}"
    );
    commits
        .as_array()
        .expect("a history is an array of commits")
        .clone()
}

/// Detects a tool that drifted between the families: a memory tool whose
/// description does not say that the call is a commit in the shared store, a
/// branch tool that does not say what a branch is for, or a session tool that
/// does not say the store is untouched, would have the model committing to
/// everyone's store when it meant to change its own scopes, or expecting a scope
/// change to reach everyone. Detects a missing or an extra tool as well.
#[test]
fn tools_list_names_every_tool_and_each_description_names_its_family() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let tools = session.tools();

    let offered: BTreeSet<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
    let expected: BTreeSet<String> = MEMORY_TOOLS
        .iter()
        .chain(BRANCH_TOOLS.iter())
        .chain(SETTINGS_TOOLS.iter())
        .chain(SESSION_TOOLS.iter())
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        offered, expected,
        "tools/list must name exactly the tools of the four families"
    );
    for tool in &tools {
        let description = tool.description.as_deref().unwrap_or_default();
        let phrase = if MEMORY_TOOLS.contains(&tool.name.as_ref()) {
            MEMORY_FAMILY_PHRASE
        } else if BRANCH_TOOLS.contains(&tool.name.as_ref()) {
            BRANCH_FAMILY_PHRASE
        } else if SETTINGS_TOOLS.contains(&tool.name.as_ref()) {
            SETTINGS_FAMILY_PHRASE
        } else {
            SESSION_FAMILY_PHRASE
        };
        assert!(
            description.contains(phrase),
            "the description of {} must carry its family's phrase {phrase:?}, got {description:?}",
            tool.name
        );
    }
}

/// Detects a write tool that cannot be given a branch, or one whose branch
/// parameter does not say what it does: a model that cannot tell which parameter
/// makes a write part of a transaction would write straight to main, and every
/// session would see half a change.
#[test]
fn every_write_tool_takes_an_optional_branch_that_says_it_commits_there_instead() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let tools = session.tools();

    for name in WRITE_TOOLS {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} must be offered"));
        let schema = Value::Object((*tool.input_schema).clone());
        let branch = schema
            .get("properties")
            .and_then(|properties| properties.get("branch"))
            .unwrap_or_else(|| panic!("{name} must take a branch parameter, got {schema}"));
        let description = branch
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            description.contains("branch instead of") || description.contains("instead of to main"),
            "{name}'s branch parameter must say that it commits to the branch instead of main, \
             got {description:?}"
        );
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert!(
            !required.contains(&"branch"),
            "{name}'s branch parameter must be optional, got required {required:?}"
        );
    }
}

/// Detects a server that offers the tools without saying where `session_key`
/// comes from: an MCP call carries no session identity, so a model that is not
/// told would have to guess the key and would change another session's state or
/// none.
#[test]
fn the_server_says_which_families_it_has_and_where_the_session_key_comes_from() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let instructions = session
        .instructions()
        .expect("the server sends instructions at initialisation");

    assert!(
        instructions.contains("session_key") && instructions.contains("hook"),
        "the instructions must say that session_key comes from the hook context, got {instructions:?}"
    );
    for family in [
        "memory management",
        "settings management",
        "session management",
    ] {
        assert!(
            instructions.contains(family),
            "the instructions must name the {family} family, got {instructions:?}"
        );
    }
    assert!(
        instructions.contains("branch") && instructions.contains("one commit"),
        "the instructions must say that a branch is how several changes land as one \
         commit, got {instructions:?}"
    );
}

/// Detects a `memory_put` that writes without the caller's identity or without
/// the caller's message: the history is how a person finds out which session
/// wrote a memory and why, and an author taken from anywhere else makes it
/// unanswerable. Detects a write that answers success without the document
/// reaching the store as well.
#[test]
fn a_memory_written_through_mcp_is_one_commit_by_the_calling_session_and_reads_back() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let session_key = "alpha/session-7";

    let written = session.call(
        "memory_put",
        json!({
            "session_key": session_key,
            "id": "bracket-torque",
            "description": "Bracket bolts are torqued to 9 Nm, in two passes",
            "kind": "critical",
            "scopes": ["global"],
            "source": "assistant",
            "body": "# Bracket torque\n\nTorque the bracket bolts to 9 Nm in two passes.\n",
            "message": "record the bracket torque"
        }),
    );

    assert_ne!(
        written.is_error,
        Some(true),
        "the write must be answered as done, got {}",
        tool_text(&written)
    );
    let outcome = tool_json(&written);
    assert!(
        outcome["commit_oid"].is_string() && outcome["version"].is_string(),
        "the answer must name the commit and the new version, got {outcome}"
    );

    let commits = history(&server, "bracket-torque");
    assert_eq!(
        commits.len(),
        1,
        "a created memory must have exactly one commit, got {commits:?}"
    );
    assert_eq!(
        commits[0]["author"],
        json!(session_key),
        "the commit's author must be the calling session"
    );
    assert_eq!(
        commits[0]["title"],
        json!("record the bracket torque"),
        "the commit's title must be the message the caller sent"
    );

    let document = tool_json(&session.call("memory_get", json!({ "id": "bracket-torque" })));
    assert!(
        document["body"]
            .as_str()
            .is_some_and(|body| body.contains("9 Nm in two passes")),
        "the document must read back with the body that was written, got {document}"
    );
    assert_eq!(
        document["kind"],
        json!("critical"),
        "the document must read back with the kind that was written"
    );
}

/// Detects a `memory_put` that drops the `metadata` keys it was given, or reads
/// them back from nowhere: importing a Claude Code memory directory through
/// this tool would silently lose Claude Code's own `type` and every key the
/// person keeping the files added, and the files would come out saying less
/// than they said going in.
#[test]
fn metadata_keys_given_to_memory_put_are_read_back_and_are_in_the_file() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let written = session.call(
        "memory_put",
        json!({
            "session_key": "alpha/session-7",
            "id": "bracket-torque",
            "description": "Bracket bolts are torqued to 9 Nm, in two passes",
            "kind": "knowledge",
            "scopes": ["global"],
            "source": "user",
            "metadata": { "type": "feedback", "strength": "hard" },
            "body": "# Bracket torque\n\nTorque the bracket bolts to 9 Nm in two passes.\n",
            "message": "record the bracket torque"
        }),
    );

    assert_ne!(
        written.is_error,
        Some(true),
        "a write carrying metadata must be answered as done, got {}",
        tool_text(&written)
    );
    let document = tool_json(&session.call("memory_get", json!({ "id": "bracket-torque" })));
    assert_eq!(
        document["metadata"],
        json!({ "type": "feedback", "strength": "hard" }),
        "the memory must read back with the metadata keys it was written with, got {document}"
    );

    // The file is read for the keys and their order; how the renderer quotes a
    // value is its own business, and what the values are is asserted above.
    let file = server.store().file_text("memories/bracket-torque.md");
    let key_at = |key: &str| {
        file.find(key)
            .unwrap_or_else(|| panic!("the file must carry {key}, got:\n{file}"))
    };
    assert!(
        key_at("\n  source:") < key_at("\n  type:").min(key_at("\n  strength:")),
        "forgetmenot's own metadata keys come first in the file, got:\n{file}"
    );
    assert!(
        key_at("\n  type:") < key_at("\n  strength:"),
        "the extra keys must be written in the order they were given, got:\n{file}"
    );
}

/// Detects a write that takes one of forgetmenot's own fields from inside
/// `metadata`: the memory would be filed under a kind or a scope the write's
/// own fields never named, and the two places would disagree with no way to
/// tell which won.
#[test]
fn a_put_with_one_of_forgetmenots_own_keys_inside_metadata_is_refused_naming_it_and_makes_no_commit()
 {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let revision_before = store_revision(&server);

    let refused = session.call(
        "memory_put",
        json!({
            "session_key": "alpha/session-7",
            "id": "bracket-torque",
            "description": "Bracket bolts are torqued to 9 Nm, in two passes",
            "kind": "knowledge",
            "scopes": ["global"],
            "source": "user",
            "metadata": { "kind": "critical" },
            "body": "# Bracket torque\n\nTorque the bracket bolts to 9 Nm in two passes.\n",
            "message": "record the bracket torque"
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "a metadata block carrying one of forgetmenot's own fields must be refused, got {}",
        tool_text(&refused)
    );
    assert!(
        tool_text(&refused).contains("kind"),
        "the refusal must name the key that cannot be set there, got {:?}",
        tool_text(&refused)
    );
    assert_eq!(
        store_revision(&server),
        revision_before,
        "a refused write must not move the store"
    );
    let (status, answer) = server.api("GET", "/api/memories/bracket-torque", None);
    assert_eq!(
        status, 404,
        "the refused memory must not be in the store, got {answer}"
    );
}

/// Detects a write path that cannot carry a Claude Code memory whole: its own
/// classification, the source a store in use carries, and its bookkeeping keys
/// would be dropped on the way in, and the memory that came out of an import
/// would say less than the file that went in.
#[test]
fn a_claude_code_memory_written_through_mcp_reads_back_with_every_key_it_had() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let document = MemoryDocument::parse(
        MemoryId::new("lathe-collets"),
        CLAUDE_CODE_MEMORY.as_bytes(),
    )
    .expect("the fixture is a memory file");

    let written = session.call(
        "memory_put",
        json!({
            "session_key": "alpha/session-7",
            "id": "lathe-collets",
            "description": document.description(),
            "kind": "knowledge",
            "scopes": ["global"],
            "source": document.source().as_str(),
            "metadata": claude_code_metadata(),
            "body": document.body,
            "message": "import the collet rule from the memory directory"
        }),
    );

    assert_ne!(
        written.is_error,
        Some(true),
        "the imported memory must be written, got {}",
        tool_text(&written)
    );
    let read_back = tool_json(&session.call("memory_get", json!({ "id": "lathe-collets" })));
    assert_eq!(
        read_back["metadata"],
        claude_code_metadata(),
        "every metadata key and value of the imported file must read back equal, got {read_back}"
    );
    assert_eq!(
        read_back["source"],
        json!("derived"),
        "the source of the imported file must read back as it was, got {read_back}"
    );
    assert_eq!(
        read_back["description"],
        json!(document.description()),
        "the description of the imported file must read back as it was, got {read_back}"
    );
    assert_eq!(
        read_back["body"],
        json!(document.body),
        "the body of the imported file must read back as it was, got {read_back}"
    );
}

/// Detects a write that overwrites a version it never read: two sessions editing
/// one memory would silently lose one of the two edits. The current version has
/// to be in the answer, because that is what the model needs to read and write
/// again.
#[test]
fn a_write_from_a_version_that_is_no_longer_current_is_refused_and_names_the_current_version() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let stale = tool_json(&session.call("memory_get", json!({ "id": "bench-power" })))["version"]
        .as_str()
        .expect("the document names the version it was read at")
        .to_string();
    // Changed by hand, the way a person with a shell does, so the version the
    // session holds is no longer the one the store has.
    server.commit(
        "add the charger bench to the rule",
        vec![(
            "memories/bench-power.md".to_string(),
            Some(
                b"---\nname: bench-power\ndescription: Cut bench power at the wall before rewiring and confirm with the meter\nmetadata:\n  kind: critical\n  scopes:\n  - global\n  source: user\n---\n# Cut bench power before rewiring\n\nThe rule now also covers the charger bench.\n"
                    .to_vec(),
            ),
        )],
    );
    let current = tool_json(&session.call("memory_get", json!({ "id": "bench-power" })))["version"]
        .as_str()
        .expect("the document names the version it was read at")
        .to_string();
    let revision_before = store_revision(&server);
    let commits_before = history(&server, "bench-power").len();

    let refused = session.call(
        "memory_put",
        json!({
            "session_key": "alpha/session-7",
            "id": "bench-power",
            "description": "Cut bench power at the wall before rewiring and confirm with the meter",
            "kind": "critical",
            "scopes": ["global"],
            "source": "user",
            "body": "# Cut bench power before rewiring\n\nWritten from the version that was read first.\n",
            "message": "rewrite the bench power rule",
            "base_version": stale
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "a write from a stale version must be refused, got {}",
        tool_text(&refused)
    );
    let text = tool_text(&refused);
    assert!(
        text.contains(&current),
        "the refusal must name the current version {current}, got {text:?}"
    );
    assert_eq!(
        store_revision(&server),
        revision_before,
        "a refused write must not move the store"
    );
    assert_eq!(
        history(&server, "bench-power").len(),
        commits_before,
        "a refused write must make no commit"
    );
}

/// Detects a write that skips validation: a memory outside a session silo that
/// links into one would put that session's notes in front of every other
/// session, which is the one thing the silo exists to prevent.
#[test]
fn a_write_whose_body_links_into_a_session_silo_is_refused_with_no_commit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let revision_before = store_revision(&server);

    let refused = session.call(
        "memory_put",
        json!({
            "session_key": "alpha/session-7",
            "id": "bracket-summary",
            "description": "Where the bracket rework is written up",
            "kind": "knowledge",
            "scopes": ["global"],
            "source": "assistant",
            "body": "# Bracket rework\n\nThe measurements are in [[sessions/alpha/session-1/notes]].\n",
            "message": "point at the bracket notes"
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "a link into another session's silo must be refused, got {}",
        tool_text(&refused)
    );
    assert!(
        tool_text(&refused).contains("silo"),
        "the refusal must say what was wrong with the document, got {:?}",
        tool_text(&refused)
    );
    assert_eq!(
        store_revision(&server),
        revision_before,
        "a refused write must not move the store"
    );
    let (status, answer) = server.api("GET", "/api/memories/bracket-summary", None);
    assert_eq!(
        status, 404,
        "the refused memory must not be in the store, got {answer}"
    );
}

/// Detects a `session_scope_on` whose scope never reaches the calling context:
/// the model asked to work in a scope and would get none of its memories, which
/// no later event would put right.
#[test]
fn a_scope_turned_on_through_mcp_delivers_that_scopes_memories_at_the_next_hook_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let answer = session.call(
        "session_scope_on",
        json!({ "session_key": "alpha/session-1", "scopes": ["widgets"] }),
    );

    assert_ne!(
        answer.is_error,
        Some(true),
        "turning on a scope of the example store must be answered as done, got {}",
        tool_text(&answer)
    );
    let scopes = tool_json(&answer);
    assert!(
        scopes["active"]
            .as_array()
            .is_some_and(|active| active.contains(&json!("widgets"))),
        "the answer must report the scope as active, got {scopes}"
    );
    let (status, delivered) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    assert!(
        context_of(&delivered).contains(WIDGET_NAMING_BODY),
        "the scope's critical memory must be delivered in full, got {delivered}"
    );
}

/// Detects a `session_scope_off` that reaches no context, or that is not reported
/// to the session: the model would go on acting on a rule it asked to stop
/// working under.
#[test]
fn a_scope_turned_off_through_mcp_leaves_its_memories_reported_as_out_of_scope() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    session.call(
        "session_scope_on",
        json!({ "session_key": "alpha/session-1", "scopes": ["widgets"] }),
    );
    let delivered = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert!(
        context_of(&delivered.1).contains(WIDGET_NAMING_BODY),
        "the memory has to have been delivered before it can be withdrawn, got {delivered:?}"
    );

    let answer = session.call(
        "session_scope_off",
        json!({ "session_key": "alpha/session-1", "scopes": ["widgets"] }),
    );

    assert_ne!(
        answer.is_error,
        Some(true),
        "turning off a scope with a file must be answered as done, got {}",
        tool_text(&answer)
    );
    let (status, withdrawn) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&withdrawn);
    assert!(
        has_retracted_line(text, "widget-naming", "no longer in an active scope"),
        "the memory must be reported as withdrawn because the scope was turned off, got {text:?}"
    );
}

/// Detects a call that turns on only one of the scopes it was given: the model
/// asked to work in several, would get the memories of one, and would read the
/// answer as all of them being on. Source: the ticket for the list form of the
/// session scope tools.
#[test]
fn two_scopes_turned_on_in_one_mcp_call_both_deliver_their_memories() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.commit(
        "keep the bench clear between jobs",
        vec![(
            "memories/workshop-tidy.md".to_string(),
            Some(WORKSHOP_RULE.as_bytes().to_vec()),
        )],
    );
    let session = server.mcp();

    let answer = session.call(
        "session_scope_on",
        json!({ "session_key": "alpha/session-1", "scopes": ["widgets", "workshop"] }),
    );

    assert_ne!(
        answer.is_error,
        Some(true),
        "turning on two scopes of the store must be answered as done, got {}",
        tool_text(&answer)
    );
    let scopes = tool_json(&answer);
    let active: BTreeSet<String> = scopes["active"]
        .as_array()
        .expect("the answer lists the active scopes")
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        active.contains("widgets"),
        "the answer must report the first scope as active, got {scopes}"
    );
    assert!(
        active.contains("workshop"),
        "the answer must report the second scope as active, got {scopes}"
    );
    let (status, delivered) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    assert!(
        context_of(&delivered).contains(WIDGET_NAMING_BODY),
        "the first scope's memory must be delivered in full, got {delivered}"
    );
    assert!(
        context_of(&delivered).contains(WORKSHOP_RULE_BODY),
        "the second scope's memory must be delivered in full, got {delivered}"
    );
}

/// Detects a list that turns on the ids it knows and then refuses the one it does
/// not: the model would read the call as failed while the session had moved into
/// scopes it is not told about. Source: the ticket for the list form of the
/// session scope tools.
#[test]
fn an_id_that_is_not_a_scope_refuses_the_whole_mcp_call_and_turns_none_of_them_on() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let refused = session.call(
        "session_scope_on",
        json!({
            "session_key": "alpha/session-1",
            "scopes": ["widgets", "spanners", "workshop"]
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "a list naming an id the store has no scope for must be refused, got {}",
        tool_text(&refused)
    );
    assert!(
        tool_text(&refused).contains("spanners"),
        "the refusal must name the id that is not a scope, got {:?}",
        tool_text(&refused)
    );
    let listed = tool_json(&session.call(
        "session_scopes",
        json!({ "session_key": "alpha/session-1" }),
    ));
    let available: BTreeSet<String> = listed["available"]
        .as_array()
        .expect("the answer lists the scopes available")
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        available.contains("widgets"),
        "the scope named before the unknown id must still be on offer, got {listed}"
    );
    assert!(
        available.contains("workshop"),
        "the scope named after the unknown id must still be on offer, got {listed}"
    );
}

/// Detects a deletion that does not reach the contexts the memory was delivered
/// to, and one reported as a scope being turned off: a memory taken out of the
/// store and a session stepping out of a scope are different events, and the
/// model decides what to do next by which one it was told.
#[test]
fn a_memory_deleted_through_mcp_is_reported_as_deleted_at_the_next_hook_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let delivered = server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        context_of(&delivered.1).contains(BENCH_POWER_BODY),
        "the memory has to have been delivered before it can be withdrawn, got {delivered:?}"
    );

    // No base_version: the tool deletes the version the store holds now, which
    // is what a model that has only read the index can do. Deleted by another
    // session, because the session that deletes a memory is recorded as holding
    // the deletion and is told nothing about it.
    let answer = session.call(
        "memory_delete",
        json!({
            "session_key": "alpha/session-2",
            "id": "bench-power",
            "message": "the bench was removed"
        }),
    );

    assert_ne!(
        answer.is_error,
        Some(true),
        "deleting a memory of the store must be answered as done, got {}",
        tool_text(&answer)
    );
    let gone = session.call("memory_get", json!({ "id": "bench-power" }));
    assert_eq!(
        gone.is_error,
        Some(true),
        "a deleted memory must no longer be readable, got {}",
        tool_text(&gone)
    );
    let (status, withdrawn) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&withdrawn);
    assert!(
        has_retracted_line(text, "bench-power", "deleted"),
        "the memory must be reported as withdrawn because it was deleted, got {text:?}"
    );
}

/// Detects a deletion that removes a version it never read: a model holding a
/// memory as it was before someone else rewrote it would take away text it never
/// saw. The current version has to be in the answer, because that is what the
/// model needs to read and decide again.
#[test]
fn a_deletion_from_a_version_that_is_no_longer_current_is_refused_and_keeps_the_memory() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let stale = tool_json(&session.call("memory_get", json!({ "id": "bench-power" })))["version"]
        .as_str()
        .expect("the document names the version it was read at")
        .to_string();
    // Changed by hand, the way a person with a shell does, so the version the
    // session holds is no longer the one the store has.
    server.commit(
        "add the charger bench to the rule",
        vec![(
            "memories/bench-power.md".to_string(),
            Some(
                b"---\nname: bench-power\ndescription: Cut bench power at the wall before rewiring and confirm with the meter\nmetadata:\n  kind: critical\n  scopes:\n  - global\n  source: user\n---\n# Cut bench power before rewiring\n\nThe rule now also covers the charger bench.\n"
                    .to_vec(),
            ),
        )],
    );
    let current = tool_json(&session.call("memory_get", json!({ "id": "bench-power" })))["version"]
        .as_str()
        .expect("the document names the version it was read at")
        .to_string();
    let revision_before = store_revision(&server);

    let refused = session.call(
        "memory_delete",
        json!({
            "session_key": "alpha/session-7",
            "id": "bench-power",
            "message": "the bench was removed",
            "base_version": stale
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "a deletion from a stale version must be refused, got {}",
        tool_text(&refused)
    );
    let text = tool_text(&refused);
    assert!(
        text.contains(&current),
        "the refusal must name the current version {current}, got {text:?}"
    );
    assert_eq!(
        store_revision(&server),
        revision_before,
        "a refused deletion must not move the store"
    );
    assert!(
        tool_json(&session.call("memory_get", json!({ "id": "bench-power" })))["body"].is_string(),
        "a refused deletion must leave the memory readable"
    );
}

/// Detects inheritance that copies nothing through MCP, or that copies the other
/// session's scopes without its own: the reason to inherit is that the other
/// session's notes become readable here, and those live in its session scope
/// alone.
#[test]
fn inheriting_another_session_through_mcp_makes_its_memories_due_here() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    // The silo in the example store belongs to session-1, and a session can only
    // be inherited from once this server has seen it.
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));

    let answer = session.call(
        "session_inherit",
        json!({ "session_key": "alpha/session-2", "from_session_key": "alpha/session-1" }),
    );

    assert_ne!(
        answer.is_error,
        Some(true),
        "inheriting a session this server has seen must be answered as done, got {}",
        tool_text(&answer)
    );
    let (status, due) = server.hook(
        MACHINE,
        SOME_TOKENS,
        &event_with(
            "session_start",
            &[
                ("session_id", json!("session-2")),
                ("source", json!("resume")),
            ],
        ),
    );
    assert_eq!(status, 200, "the event must be answered");
    assert!(
        has_index_line(
            context_of(&due),
            "sessions/alpha/session-1/notes",
            SESSION_NOTES_DESCRIPTION
        ),
        "the other session's notes must be due in the caller, got {due}"
    );
}

/// Detects a `memory_get` that drops its `session_key`: the memory the model just
/// read in full would be delivered to it again at the very next event, and the
/// statistics would count a fetch that never happened, or none where one did.
#[test]
fn a_memory_fetched_through_mcp_for_a_session_is_recorded_as_delivered_and_as_a_fetch() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let document = tool_json(&session.call(
        "memory_get",
        json!({ "id": "bench-power", "session_key": "alpha/session-1" }),
    ));

    assert!(
        document["body"]
            .as_str()
            .is_some_and(|body| body.contains(BENCH_POWER_BODY)),
        "the fetch must return the body, got {document}"
    );
    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the event must be answered");
    assert!(
        !context_of(&answer).contains(BENCH_POWER_BODY),
        "a memory this session has just fetched must not be delivered again, got {answer}"
    );

    server.commit(
        "add the charger bench to the rule",
        vec![(
            "memories/bench-power.md".to_string(),
            Some(
                b"---\nname: bench-power\ndescription: Cut bench power at the wall before rewiring and confirm with the meter\nmetadata:\n  kind: critical\n  scopes:\n  - global\n  source: user\n---\n# Cut bench power before rewiring\n\nThe rule now also covers the charger bench.\n"
                    .to_vec(),
            ),
        )],
    );
    let (status, changed) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the event must be answered");
    assert!(
        context_of(&changed).contains("The rule now also covers the charger bench."),
        "the changed memory must be delivered again, got {changed}"
    );

    let statistics = server.stats();
    assert_eq!(
        statistics.count_rows(Table::ToolCalls),
        1,
        "the fetch must be recorded as exactly one tool call"
    );
    let rows = statistics
        .memory_stats()
        .expect("the statistics are readable");
    let fetched = rows
        .iter()
        .find(|row| row.memory == "bench-power")
        .expect("the fetched memory must have a statistics row");
    assert_eq!(
        fetched.fetched_full, 1,
        "the row must count the fetch under the fetch tool's name, got {fetched:?}"
    );
}

/// Detects a `session_key` that is not a context key being taken anyway: a key
/// with a part missing names a context no session ever reads, so the scopes would
/// be turned on for nobody and the model would be told it worked. A malformed key
/// must also leave the session able to go on calling.
#[test]
fn a_session_key_that_names_no_context_is_an_invalid_parameter() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let refused = session
        .try_call("session_scopes", json!({ "session_key": "alpha" }))
        .expect_err("a key that names no context must be refused");

    match &refused {
        ServiceError::McpError(error) => assert_eq!(
            error.code,
            ErrorCode::INVALID_PARAMS,
            "a malformed session_key must be reported as an invalid parameter, got {refused}"
        ),
        other => panic!("a malformed session_key must be an MCP error, got {other}"),
    }
    let answered = session.call(
        "session_scopes",
        json!({ "session_key": "alpha/session-1" }),
    );
    assert_ne!(
        answered.is_error,
        Some(true),
        "the session must go on answering after a refused call, got {}",
        tool_text(&answered)
    );
}

/// Detects an `/mcp` open to any `Host`, which is what lets a page in a browser
/// reach a server bound to loopback, and one closed to the hosts the
/// configuration allows, which would make the server unreachable by name on the
/// LAN it is configured for.
#[test]
fn a_request_to_mcp_is_refused_unless_its_host_header_is_allowed() {
    let configured_host = "memory.example";
    let server = TestServer::start(example_store_files(), |config| {
        config.allowed_hosts = vec![configured_host.to_string()];
    });
    let loopback = server
        .url()
        .strip_prefix("http://")
        .expect("the test server's URL is HTTP")
        .to_string();

    assert_eq!(
        initialize_with_host(&server, "elsewhere.example").0,
        403,
        "a Host outside the allowed list must be refused"
    );
    assert_eq!(
        initialize_with_host(&server, configured_host).0,
        200,
        "the configured Host must be accepted"
    );
    let (status, body) = initialize_with_host(&server, &loopback);
    assert_eq!(
        status, 200,
        "the loopback address the server is bound to must stay accepted, got {body}"
    );
}

/// Open an MCP session with `host` as the `Host` header and report the status and
/// the body, which is how a request from another name than the server's own
/// arrives.
fn initialize_with_host(server: &TestServer, host: &str) -> (u16, String) {
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "forgetmenot-test", "version": "0" }
        }
    });
    let mut response = test_agent()
        .post(format!("{}/mcp", server.url()))
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .send(serde_json::to_string(&initialize).expect("the body serialises"))
        .expect("the request reaches the server");
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .expect("the answer is text");
    (status, text)
}

/// Detects a `scopes` filter that is ignored or inverted. The index is the only
/// way the model finds a memory it has not been given, and a filter that answers
/// with the memories of every other scope, or with nothing, makes it useless for
/// the one scope it asked about. The tool takes several scopes where the JSON API
/// takes one, so this is the MCP adapter's own filtering.
#[test]
fn the_memory_index_filtered_by_scopes_lists_the_memories_of_those_scopes_only() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let listed = tool_json(&session.call("memory_index", json!({ "scopes": ["widgets"] })));

    let ids: BTreeSet<String> = listed
        .as_array()
        .expect("the index is an array")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from(["widget-naming".to_string()]),
        "only the memories carrying the named scope may be listed"
    );
    let all = tool_json(&session.call("memory_index", json!({})));
    assert!(
        all.as_array().is_some_and(|summaries| summaries.len() > 1),
        "without a filter every memory must be listed, got {all}"
    );
}

// ---------------------------------------------------------------------------
// Branches
// ---------------------------------------------------------------------------

/// Detects a branch family that does not work through MCP: a transaction opened
/// and landed by a model must keep its writes out of the index until it lands and
/// then land them as one commit, or a model would publish half a change and have
/// no way to see what it was about to publish.
#[test]
fn three_writes_on_a_branch_through_mcp_are_invisible_until_they_land_as_one_commit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let session_key = "alpha/session-11";

    let opened = tool_json(&session.call("branch_create", json!({ "session_key": session_key })));
    let branch = opened["branch"]
        .as_str()
        .expect("branch_create names the branch")
        .to_string();
    let revision_before = store_revision(&server);

    let written = session.call(
        "memory_put",
        json!({
            "session_key": session_key,
            "id": "jig-storage",
            "description": "The bracket jig lives in the second drawer",
            "kind": "knowledge",
            "scopes": ["global"],
            "source": "assistant",
            "body": "# The bracket jig\n\nThe jig lives in the second drawer.\n",
            "message": "record where the bracket jig lives",
            "branch": branch,
        }),
    );
    assert_ne!(
        written.is_error,
        Some(true),
        "the write on the branch must be accepted, got {}",
        tool_text(&written)
    );
    let replaced = session.call(
        "memory_replace_text",
        json!({
            "session_key": session_key,
            "id": "bench-power",
            "old_string": "at the wall",
            "new_string": "at the wall switch",
            "message": "say which switch cuts the bench supply",
            "branch": branch,
        }),
    );
    assert_ne!(
        replaced.is_error,
        Some(true),
        "the replacement on the branch must be accepted, got {}",
        tool_text(&replaced)
    );
    let fields = session.call(
        "memory_set_fields",
        json!({
            "session_key": session_key,
            "id": "reading-list",
            "kind": "critical",
            "message": "make the reading list critical",
            "branch": branch,
        }),
    );
    assert_ne!(
        fields.is_error,
        Some(true),
        "the field write on the branch must be accepted, got {}",
        tool_text(&fields)
    );

    let index = tool_json(&session.call("memory_index", json!({})));
    assert!(
        !tool_text(&session.call("memory_index", json!({}))).contains("jig-storage"),
        "nothing on the branch may be in the index yet, got {index}"
    );
    assert_eq!(
        store_revision(&server),
        revision_before,
        "nothing on the branch may move the store's head"
    );
    let diff = tool_json(&session.call("branch_diff", json!({ "branch": branch })));
    assert_eq!(
        diff["ahead"],
        json!(3),
        "the diff must count the three writes, got {diff}"
    );

    let landed = tool_json(&session.call(
        "branch_land",
        json!({
            "session_key": session_key,
            "branch": branch,
            "message": "record the jig and the bench power wording",
        }),
    ));

    let commits = history(&server, "bench-power");
    assert_eq!(
        commits.first().map(|commit| commit["oid"].clone()),
        Some(landed["commit_oid"].clone()),
        "the landed commit must be the newest one to touch the memory, got {commits:?}"
    );
    assert_eq!(
        commits.first().map(|commit| commit["title"].clone()),
        Some(json!("record the jig and the bench power wording")),
        "the one commit must carry the title the land was given, got {commits:?}"
    );
    let index = tool_text(&session.call("memory_index", json!({})));
    assert!(
        index.contains("jig-storage"),
        "the landed memory must be in the index, got {index}"
    );
    let open = tool_json(&session.call("branch_list", json!({})));
    assert_eq!(
        open,
        json!([]),
        "the landed branch must no longer be open, got {open}"
    );
}

/// Detects a settings change on a branch that changes how sessions behave before
/// it lands, and one that does not take effect once it has: the settings are part
/// of the store, so a branch has to hold them back exactly as it holds back a
/// memory, and landing has to put them in force at the next event.
#[test]
fn a_settings_change_on_a_branch_through_mcp_is_invisible_until_it_lands() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let session_key = "alpha/session-13";
    let branch =
        tool_json(&session.call("branch_create", json!({ "session_key": session_key })))["branch"]
            .as_str()
            .expect("branch_create names the branch")
            .to_string();

    let written = session.call(
        "settings_set",
        json!({
            "session_key": session_key,
            "key": "interrupt_on_critical",
            "value": false,
            "message": "stop holding tool calls for critical memories",
            "branch": branch,
        }),
    );
    assert_ne!(
        written.is_error,
        Some(true),
        "the settings write on the branch must be accepted, got {}",
        tool_text(&written)
    );

    let on_main = tool_json(&session.call("settings_get", json!({})));
    assert_eq!(
        on_main["settings"]["interrupt_on_critical"],
        json!(true),
        "main must still carry the setting it had, got {on_main}"
    );
    let (_, before) = server.hook(
        MACHINE,
        SOME_TOKENS,
        &event_with("pre_tool_use_bash", &[("session_id", json!("session-14"))]),
    );
    assert_eq!(
        permission_decision(&before),
        Some("deny"),
        "the behaviour must not change before the branch lands, got {before}"
    );

    let landed = session.call(
        "branch_land",
        json!({
            "session_key": session_key,
            "branch": branch,
            "message": "stop holding tool calls for critical memories",
        }),
    );
    assert_ne!(
        landed.is_error,
        Some(true),
        "the branch must land, got {}",
        tool_text(&landed)
    );

    let after_landing = tool_json(&session.call("settings_get", json!({})));
    assert_eq!(
        after_landing["settings"]["interrupt_on_critical"],
        json!(false),
        "the landed setting must be the one in force, got {after_landing}"
    );
    let (_, after) = server.hook(
        MACHINE,
        SOME_TOKENS,
        &event_with("pre_tool_use_bash", &[("session_id", json!("session-15"))]),
    );
    assert_eq!(
        permission_decision(&after),
        None,
        "the landed setting must be in force at the next event, got {after}"
    );
}

/// Detects a land that reports a conflict the model cannot act on: the answer has
/// to name the file and both texts, so that the model can write the version it
/// wants on the branch instead of guessing or giving up.
#[test]
fn a_conflicting_land_through_mcp_reports_the_file_and_both_texts() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let session_key = "alpha/session-12";

    let branch =
        tool_json(&session.call("branch_create", json!({ "session_key": session_key })))["branch"]
            .as_str()
            .expect("branch_create names the branch")
            .to_string();
    session.call(
        "memory_replace_text",
        json!({
            "session_key": session_key,
            "id": "bench-power",
            "old_string": "reads zero",
            "new_string": "reads zero volts",
            "message": "say what the meter reads",
            "branch": branch,
        }),
    );
    let on_main = session.call(
        "memory_replace_text",
        json!({
            "session_key": session_key,
            "id": "bench-power",
            "old_string": "reads zero",
            "new_string": "reads nothing at all",
            "message": "say what the meter reads on main",
        }),
    );
    assert_ne!(
        on_main.is_error,
        Some(true),
        "the write straight to main must be accepted, got {}",
        tool_text(&on_main)
    );

    let failed = session.call(
        "branch_land",
        json!({
            "session_key": session_key,
            "branch": branch,
            "message": "say what the meter reads",
        }),
    );

    assert_eq!(
        failed.is_error,
        Some(true),
        "a conflicting land must be reported as a failure, got {}",
        tool_text(&failed)
    );
    let text = tool_text(&failed);
    assert!(
        text.contains("memories/bench-power.md"),
        "the failure must name the file, got {text:?}"
    );
    assert!(
        text.contains("reads nothing at all") && text.contains("reads zero volts"),
        "the failure must carry the text of both sides, got {text:?}"
    );
    let open = tool_json(&session.call("branch_list", json!({})));
    assert_eq!(
        open.as_array().map(Vec::len),
        Some(1),
        "the branch must be left open to resolve on, got {open}"
    );
}

// ---------------------------------------------------------------------------
// What a write records about its own writer
// ---------------------------------------------------------------------------

/// A body marker for the edit a branch carries, written nowhere else in the
/// example store, so that "the writer was given its own edit back" can be told
/// from "the writer was given nothing".
const BRANCH_EDIT_MARKER: &str = "wall switch in the corner";

/// The body of the memory created on a branch, and the marker inside it.
const BRANCH_MEMORY_ID: &str = "bench-lighting";
const BRANCH_MEMORY_MARKER: &str = "The bench lamp is switched at the bench";

/// The marker of an edit committed to the store by hand, outside the server.
const OUTSIDE_EDIT_MARKER: &str = "The rule now also covers the charger bench.";

/// Start one session, so that everything its scopes owe it, the global rule
/// among them, has been delivered before the test changes anything.
fn start_session(server: &TestServer, session_id: &str) {
    let (status, answer) = server.hook(
        MACHINE,
        SOME_TOKENS,
        &event_with("session_start", &[("session_id", json!(session_id))]),
    );
    assert_eq!(status, 200, "a session start must be answered");
    assert!(
        context_of(&answer).contains(BENCH_POWER_BODY),
        "the session must be given the global rule in full at its start, got {answer}"
    );
}

/// A prompt event whose text matches no trigger in the example store, so that
/// what the answer carries is what the session is owed and nothing a scope was
/// turned on for.
fn plain_prompt(session_id: &str) -> Value {
    event_with(
        "user_prompt_submit",
        &[
            ("session_id", json!(session_id)),
            ("prompt", json!("carry on from where we stopped")),
        ],
    )
}

/// Everything one hook answer tells the model: the context it injects and the
/// reason it gives for holding a tool call, which is where a held call names
/// what has arrived.
fn answer_text(answer: &Value) -> String {
    format!(
        "{}\n{}",
        context_of(answer),
        permission_decision_reason(answer).unwrap_or_default()
    )
}

/// The version of one memory as the store reports it, which a write has to be
/// made against. Fetched for no session, so the fetch itself records nothing.
fn memory_version(session: &McpSession<'_>, id: &str) -> String {
    let document = tool_json(&session.call("memory_get", json!({ "id": id })));
    document["version"]
        .as_str()
        .expect("a fetched memory names its version")
        .to_string()
}

/// Assert that one tool call was accepted, naming what the store said if not.
fn accepted(result: &rmcp::model::CallToolResult, what: &str) {
    assert_ne!(
        result.is_error,
        Some(true),
        "{what} must be accepted, got {}",
        tool_text(result)
    );
}

/// The bench power memory as a file, with `body` as its text, for a commit made
/// outside the server.
fn bench_power_file(body: &str) -> (String, Option<Vec<u8>>) {
    let text = format!(
        "---\nname: bench-power\ndescription: Cut bench power at the wall before rewiring and \
         confirm with the meter\nmetadata:\n  kind: critical\n  scopes:\n  - global\n  source: \
         user\n---\n# Cut bench power before rewiring\n\n{body}\n"
    );
    (
        "memories/bench-power.md".to_string(),
        Some(text.into_bytes()),
    )
}

/// Detects a write through the MCP tools being delivered back to the session
/// that made it: the writer composed the text and already holds it, so repeating
/// it at the next event spends its context on what it just wrote. Every other
/// session must still be given the new version, because it holds the old one.
///
/// The expectation is the one issue 11 states: a write records the writer as
/// having seen what it wrote, and changes nothing for any other context.
#[test]
fn a_memory_written_through_mcp_is_not_delivered_back_to_its_writer_but_is_to_another_session() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    start_session(&server, "session-1");
    start_session(&server, "session-2");

    let version = memory_version(&session, "bench-power");
    accepted(
        &session.call(
            "memory_set_fields",
            json!({
                "session_key": "alpha/session-1",
                "id": "bench-power",
                "description": "Cut bench power at the wall and prove the rail reads zero",
                "base_version": version,
                "message": "sharpen the bench power description",
            }),
        ),
        "the write by session-1",
    );

    let (status, writer) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-1"));
    assert_eq!(status, 200, "the writer's next event must be answered");
    assert_eq!(
        context_of(&writer),
        "",
        "the session that wrote the memory must be given nothing back, got {writer}"
    );
    let (status, other) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-2"));
    assert_eq!(status, 200, "the other session's event must be answered");
    assert!(
        context_of(&other).contains(BENCH_POWER_BODY),
        "the other session must be given the changed memory in full, got {other}"
    );
}

/// Detects a retraction sent to the session that deleted the memory itself: it
/// knows it deleted it, and being told the memory was withdrawn is one more thing
/// to read about a decision it made. A session that still holds the memory must
/// be told, or it would go on acting on a rule that is gone.
///
/// The expectation is the one issue 11 states for a deletion.
#[test]
fn a_memory_deleted_through_mcp_sends_no_retraction_to_its_deleter() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    start_session(&server, "session-1");
    start_session(&server, "session-2");

    let version = memory_version(&session, "bench-power");
    accepted(
        &session.call(
            "memory_delete",
            json!({
                "session_key": "alpha/session-1",
                "id": "bench-power",
                "base_version": version,
                "message": "the bench supply rule is no longer in force",
            }),
        ),
        "the deletion by session-1",
    );

    let (status, deleter) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-1"));
    assert_eq!(status, 200, "the deleter's next event must be answered");
    assert!(
        !has_retracted_line(context_of(&deleter), "bench-power", "deleted"),
        "the session that deleted the memory must not be told it was withdrawn, got {deleter}"
    );
    let (status, other) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-2"));
    assert_eq!(status, 200, "the other session's event must be answered");
    assert!(
        has_retracted_line(context_of(&other), "bench-power", "deleted"),
        "the other session must be told the memory was withdrawn, got {other}"
    );
}

/// Detects a rename delivered back to the session that made it, under either id:
/// the renamer would be told the id it moved away from is gone and handed the
/// body again under the id it chose, both of which it did itself. A session that
/// holds the old id must be told both, because for it the memory has moved.
///
/// The expectation is the one issue 11 states for a rename, which is two ids: the
/// one the memory left and the one it took.
#[test]
fn a_memory_renamed_through_mcp_is_delivered_to_its_renamer_under_neither_id() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    start_session(&server, "session-1");
    start_session(&server, "session-2");

    let version = memory_version(&session, "bench-power");
    accepted(
        &session.call(
            "memory_rename",
            json!({
                "session_key": "alpha/session-1",
                "from": "bench-power",
                "to": "bench-supply",
                "base_version": version,
                "message": "call the rule after the supply it is about",
            }),
        ),
        "the rename by session-1",
    );

    let (status, renamer) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-1"));
    assert_eq!(status, 200, "the renamer's next event must be answered");
    let text = context_of(&renamer);
    assert!(
        !has_retracted_line(text, "bench-power", "deleted"),
        "the session that renamed the memory must not be told the old id is gone, got {renamer}"
    );
    assert!(
        !text.contains(BENCH_POWER_BODY),
        "the session that renamed the memory must not be given it again, got {renamer}"
    );
    let (status, other) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-2"));
    assert_eq!(status, 200, "the other session's event must be answered");
    let text = context_of(&other);
    assert!(
        has_retracted_line(text, "bench-power", "deleted"),
        "the other session must be told the old id is gone, got {other}"
    );
    assert!(
        text.contains("bench-supply") && text.contains(BENCH_POWER_BODY),
        "the other session must be given the memory under its new id, got {other}"
    );
}

/// Detects a landed branch delivered back to the session that landed it: every
/// write on the branch is that session's own work, and the land is the moment it
/// reaches `main`, so that is where the landing session is recorded as holding
/// it. Every other session must be given the whole landed change.
///
/// The expectation is the one issue 11 states for a branch: a write on a branch
/// records nothing until it lands, and the land records every memory the one
/// commit changed.
#[test]
fn a_landed_branch_is_not_delivered_back_to_the_session_that_landed_it() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    start_session(&server, "session-1");
    start_session(&server, "session-2");

    let branch = tool_json(
        &session.call("branch_create", json!({ "session_key": "alpha/session-1" })),
    )["branch"]
        .as_str()
        .expect("branch_create names the branch")
        .to_string();
    accepted(
        &session.call(
            "memory_replace_text",
            json!({
                "session_key": "alpha/session-1",
                "id": "bench-power",
                "old_string": "off at the wall",
                "new_string": "off at the wall switch in the corner",
                "message": "say which switch cuts the bench supply",
                "branch": branch,
            }),
        ),
        "the edit on the branch",
    );
    accepted(
        &session.call(
            "memory_put",
            json!({
                "session_key": "alpha/session-1",
                "id": BRANCH_MEMORY_ID,
                "description": "The bench lamp has its own switch at the bench",
                "kind": "critical",
                "scopes": ["global"],
                "source": "assistant",
                "body": format!("# Bench lighting\n\n{BRANCH_MEMORY_MARKER}.\n"),
                "message": "record where the bench lamp is switched",
                "branch": branch,
            }),
        ),
        "the new memory on the branch",
    );
    accepted(
        &session.call(
            "branch_land",
            json!({
                "session_key": "alpha/session-1",
                "branch": branch,
                "message": "say where the bench supply and the lamp are switched",
            }),
        ),
        "the land",
    );

    let (status, lander) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-1"));
    assert_eq!(status, 200, "the landing session's event must be answered");
    assert_eq!(
        context_of(&lander),
        "",
        "the session that landed the branch must be given none of it back, got {lander}"
    );
    let (status, other) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-2"));
    assert_eq!(status, 200, "the other session's event must be answered");
    let text = context_of(&other);
    assert!(
        text.contains(BRANCH_EDIT_MARKER),
        "the other session must be given the edited memory in full, got {other}"
    );
    assert!(
        text.contains(BRANCH_MEMORY_MARKER),
        "the other session must be given the memory the branch created, got {other}"
    );
}

/// Detects a write recorded against the whole session rather than the context
/// that made it: a subagent is a context of its own, with an empty record and a
/// model that has read nothing, so a memory its parent session wrote has to reach
/// it like any other. A subagent starved of it would work without the rule.
///
/// The expectation is the one issue 11 states: the writer alone is recorded, and
/// the writer's own subagents are other contexts.
#[test]
fn a_write_by_the_session_is_still_delivered_to_its_subagent() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    start_session(&server, "session-1");

    accepted(
        &session.call(
            "memory_replace_text",
            json!({
                "session_key": "alpha/session-1",
                "id": "bench-power",
                "old_string": "off at the wall",
                "new_string": "off at the wall switch in the corner",
                "message": "say which switch cuts the bench supply",
            }),
        ),
        "the write by session-1",
    );

    let (status, started) = server.hook(MACHINE, SOME_TOKENS, &hook_fixture("subagent_start"));
    assert_eq!(status, 200, "the subagent start must be answered");
    let (status, in_subagent) = server.hook(
        MACHINE,
        SOME_TOKENS,
        &hook_fixture("pre_tool_use_in_subagent"),
    );
    assert_eq!(status, 200, "the subagent's tool call must be answered");

    let told = format!("{}\n{}", answer_text(&started), answer_text(&in_subagent));
    assert!(
        told.contains(BRANCH_EDIT_MARKER),
        "the subagent must be given what its session wrote, got {told:?}"
    );
    let (status, writer) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-1"));
    assert_eq!(status, 200, "the writing session's event must be answered");
    assert!(
        !context_of(&writer).contains(BRANCH_EDIT_MARKER),
        "the session that wrote the memory must still be given nothing back, got {writer}"
    );
}

/// Detects a session held to hold every memory rather than the ones it wrote: a
/// change committed to the store by hand is nobody's own write, so a session that
/// has written something else must still be given it. Marking more than the ids
/// written would silently withhold changes the session has never seen.
///
/// The expectation is the one issue 11 states: what a write records is the
/// memories that write touched.
#[test]
fn a_change_committed_outside_the_server_is_still_delivered_to_the_session() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    start_session(&server, "session-1");

    accepted(
        &session.call(
            "memory_put",
            json!({
                "session_key": "alpha/session-1",
                "id": "bench-checklist",
                "description": "The bench checklist hangs by the door and is worked top to bottom",
                "kind": "knowledge",
                "scopes": ["global"],
                "source": "assistant",
                "body": "# The bench checklist\n\nIt hangs by the door and is worked top to \
                         bottom.\n",
                "message": "record where the bench checklist hangs",
            }),
        ),
        "the write by session-1",
    );
    server.commit(
        "add the charger bench to the rule",
        vec![bench_power_file(OUTSIDE_EDIT_MARKER)],
    );

    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &plain_prompt("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&answer);
    assert!(
        text.contains(OUTSIDE_EDIT_MARKER),
        "a change the session did not write must be delivered to it, got {answer}"
    );
    assert!(
        !text.contains("bench-checklist"),
        "the memory the session wrote itself must not be delivered back, got {answer}"
    );
}
