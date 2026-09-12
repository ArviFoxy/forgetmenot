//! Tests of the MCP tools, driven by a real rmcp streamable HTTP client against a
//! real server on a temporary copy of the example store.
//!
//! These test the adapter: that the eight tools are there, that each says which
//! family it belongs to, that a tool's parameters reach the operation it stands
//! for, and that a failure comes back as something the model can act on. What the
//! operations themselves guarantee is tested in `operations_test.rs` and
//! `api_test.rs`; what is asserted here is only visible through MCP.
//!
//! The expectations come from the plan's two tool families and its state machine,
//! and from the example store committed in this repository.

mod common;

use std::collections::BTreeSet;

use forgetmenot_server::stats::Table;
use rmcp::model::ErrorCode;
use rmcp::service::ServiceError;
use serde_json::{Value, json};

use common::{
    TestServer, additional_context, example_store_files, hook_fixture, test_agent, tool_json,
    tool_text,
};

/// The machine every session in these tests runs on.
const MACHINE: &str = "alpha";

/// The context size reported with the hook events used here, which are not about
/// staleness.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// The memory management tools, which change the store.
const MEMORY_TOOLS: [&str; 4] = ["memory_index", "memory_get", "memory_put", "memory_archive"];

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

// Lines that appear in one memory's body of the example store and nowhere else,
// so that "delivered in full" can be told apart from "named in an index line".
const WIDGET_NAMING_BODY: &str = "Downstream drawings cite part numbers by value";
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";
/// The index entry of the session memory in the example store's silo.
const SESSION_NOTES_DESCRIPTION: &str =
    "Working notes for the bracket rework, measured in millimetres";

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

/// Detects a tool that drifted between the two families: a memory tool whose
/// description does not say that the call is a commit in the shared store, or a
/// session tool that does not say the store is untouched, would have the model
/// committing to everyone's store when it meant to change its own scopes, or
/// expecting its own scope change to reach everyone. Detects a missing or an
/// extra tool as well.
#[test]
fn tools_list_names_the_eight_tools_and_each_description_names_its_family() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let tools = session.tools();

    let offered: BTreeSet<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
    let expected: BTreeSet<String> = MEMORY_TOOLS
        .iter()
        .chain(SESSION_TOOLS.iter())
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        offered, expected,
        "tools/list must name exactly the eight tools of the two families"
    );
    for tool in &tools {
        let description = tool.description.as_deref().unwrap_or_default();
        let phrase = if MEMORY_TOOLS.contains(&tool.name.as_ref()) {
            MEMORY_FAMILY_PHRASE
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
    assert!(
        instructions.contains("memory management") && instructions.contains("session management"),
        "the instructions must name both families, got {instructions:?}"
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
        json!({ "session_key": "alpha/session-1", "scope": "widgets" }),
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
fn a_scope_turned_off_through_mcp_is_reported_as_scope_off_at_the_next_hook_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    session.call(
        "session_scope_on",
        json!({ "session_key": "alpha/session-1", "scope": "widgets" }),
    );
    let delivered = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert!(
        context_of(&delivered.1).contains(WIDGET_NAMING_BODY),
        "the memory has to have been delivered before it can be withdrawn, got {delivered:?}"
    );

    let answer = session.call(
        "session_scope_off",
        json!({ "session_key": "alpha/session-1", "scope": "widgets" }),
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
        has_retracted_line(text, "widget-naming", "scope off"),
        "the memory must be reported as withdrawn because the scope was turned off, got {text:?}"
    );
}

/// Detects an archival that does not reach the contexts the memory was delivered
/// to, and one reported as a scope being turned off: a memory taken out of force
/// everywhere and a session stepping out of a scope are different events, and the
/// model decides what to do next by which one it was told.
#[test]
fn a_memory_archived_through_mcp_is_reported_as_archived_at_the_next_hook_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let delivered = server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        context_of(&delivered.1).contains(BENCH_POWER_BODY),
        "the memory has to have been delivered before it can be withdrawn, got {delivered:?}"
    );

    // No base_version: the tool archives the version the store holds now, which
    // is what a model that has only read the index can do.
    let answer = session.call(
        "memory_archive",
        json!({
            "session_key": "alpha/session-1",
            "id": "bench-power",
            "message": "the bench was removed"
        }),
    );

    assert_ne!(
        answer.is_error,
        Some(true),
        "archiving a memory of the store must be answered as done, got {}",
        tool_text(&answer)
    );
    let (status, withdrawn) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&withdrawn);
    assert!(
        has_retracted_line(text, "bench-power", "archived"),
        "the memory must be reported as withdrawn because it was archived, got {text:?}"
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
