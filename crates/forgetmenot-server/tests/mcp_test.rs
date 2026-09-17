//! The contract of the MCP tools, driven by a real rmcp streamable HTTP client
//! against a real server on a temporary copy of the example store.
//!
//! These test the adapter and nothing else: that the tools offered are exactly
//! the ones this server registers, that each says which family it belongs to,
//! that a tool's parameters reach the operation it stands for, that a call is
//! recorded, and that a failure comes back as something the model can read and
//! act on. What the operations themselves guarantee is `operations_test.rs`,
//! what the store guarantees is `store_test.rs` and `branch_test.rs`, and what
//! a session's own writes do to the sessions around it is
//! `scenarios_own_writes.rs`. What is asserted here is only visible through MCP.
//!
//! The expectations come from the plan's four tool families and from the example
//! store committed in this repository.

mod common;

use std::collections::BTreeSet;

use forgetmenot_server::mcp::FORGETMENOT_TOOL_NAMES;
use forgetmenot_server::stats::Table;
use rmcp::model::ErrorCode;
use rmcp::service::ServiceError;
use serde_json::{Value, json};

use common::{TestServer, example_store_files, test_agent, tool_json, tool_text};

/// The memory management tools, which read and change the store.
const MEMORY_TOOLS: [&str; 9] = [
    "memory_index",
    "memory_get",
    "memory_history",
    "memory_blame",
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

/// What a memory tool's description has to say: a write to the store is a commit.
const MEMORY_FAMILY_PHRASE: &str = "git commit";

/// What a session tool's description has to say: the store is not involved.
const SESSION_FAMILY_PHRASE: &str = "never touches the store";

/// What a branch tool's description has to say: what a branch is for.
const BRANCH_FAMILY_PHRASE: &str = "one commit on main";

/// What a settings tool's description has to say: what these settings are.
const SETTINGS_FAMILY_PHRASE: &str = "behaviour settings";

/// A line of `bench-power`'s body of the example store and of no other memory,
/// so that "the whole body came back" can be told apart from "an index line
/// did".
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";

/// Detects a tool offered that the hook does not know about, and one the hook
/// knows about that no client can call: the hook recognises the memory system's
/// own traffic by that list, so a tool outside it has its inputs and its answers
/// matched against the triggers and turns scopes on from the memory system's own
/// chatter. Detects a tool that drifted between the families as well: a memory
/// tool whose description does not say that the call is a commit in the shared
/// store, a branch tool that does not say what a branch is for, or a session
/// tool that does not say the store is untouched, would have the model
/// committing to everyone's store when it meant to change its own scopes.
#[test]
fn tools_list_offers_exactly_the_tools_the_hook_knows_and_each_description_names_its_family() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let tools = session.tools();

    let offered: BTreeSet<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
    assert_eq!(
        offered,
        FORGETMENOT_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<BTreeSet<String>>(),
        "tools/list must offer exactly the tools the hook keeps away from the triggers"
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
/// session would see half a change. A branch that were required would stop every
/// ordinary write instead.
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
/// none. Detects the families going unnamed as well, which is how a model tells
/// a call that commits to everyone's store from one that changes only its own
/// scopes.
#[test]
fn the_server_says_which_families_it_has_and_where_the_session_key_comes_from() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let instructions = session
        .instructions()
        .expect("the server sends instructions at initialisation");

    assert!(
        instructions.contains("session_key") && instructions.contains("hook"),
        "the instructions must say that session_key comes from the hook context, got \
         {instructions:?}"
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
        "the instructions must say that a branch is how several changes land as one commit, got \
         {instructions:?}"
    );
}

/// Detects a `memory_put` whose parameters do not reach the write: a body, a
/// kind and a message sent by the model would be answered as done and the store
/// would hold something else. Detects an author taken from anywhere but the
/// calling session as well, which makes "which session wrote this memory"
/// unanswerable, and an answer without the commit and the version, which are
/// what the model needs in order to write again.
#[test]
fn a_memory_written_through_mcp_is_authored_by_the_calling_session_and_answers_what_it_wrote() {
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

    let (status, commits) = server.api("GET", "/api/memories/bracket-torque/history", None);
    assert_eq!(
        status, 200,
        "the memory's history must be readable, got {commits}"
    );
    let commits = commits.as_array().expect("a history is a list");
    assert_eq!(
        commits.len(),
        1,
        "a created memory must have exactly one commit, got {commits:?}"
    );
    assert_eq!(
        (commits[0]["author"].clone(), commits[0]["title"].clone()),
        (json!(session_key), json!("record the bracket torque")),
        "the commit must be authored by the calling session and carry the message it sent"
    );

    let document = tool_json(&session.call("memory_get", json!({ "id": "bracket-torque" })));
    assert_eq!(
        (
            document["kind"].clone(),
            document["body"]
                .as_str()
                .map(|body| body.contains("9 Nm in two passes"))
        ),
        (json!("critical"), Some(true)),
        "the document must read back with the kind and the body that were written, got {document}"
    );
}

/// Detects a `scopes` filter that is ignored or inverted. The index is the only
/// way the model finds a memory it has not been given, and a filter that answers
/// with the memories of every other scope, or with nothing, makes it useless for
/// the one scope it asked about. The tool takes several scopes where the JSON
/// API takes one, so this is the MCP adapter's own filtering.
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

/// Detects a scope tool that takes one scope where its parameter is a list: the
/// model asked to work in several, would be moved into one, and would read the
/// answer as all of them being on. Detects an answer that does not report the
/// scopes now active and the ones still on offer, which is the only thing the
/// model can decide its next call from.
///
/// Source: the ticket for the list form of the session scope tools.
#[test]
fn a_session_scope_call_takes_a_list_of_scopes_and_answers_the_active_and_the_available_ones() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let active_and_available = |answer: &Value| -> (BTreeSet<String>, BTreeSet<String>) {
        let names = |key: &str| {
            answer[key]
                .as_array()
                .unwrap_or_else(|| panic!("the answer must list the {key} scopes, got {answer}"))
                .iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        };
        (names("active"), names("available"))
    };

    let turned_on = tool_json(&session.call(
        "session_scope_on",
        json!({ "session_key": "alpha/session-1", "scopes": ["widgets", "workshop"] }),
    ));

    let (active, available) = active_and_available(&turned_on);
    for id in ["widgets", "workshop"] {
        assert!(
            active.contains(id),
            "every scope the list named must be reported as active, {id} is not in {active:?}"
        );
        assert!(
            !available.contains(id),
            "a scope that is now active must not be offered again, {id} is in {available:?}"
        );
    }

    let listed = tool_json(&session.call(
        "session_scopes",
        json!({ "session_key": "alpha/session-1" }),
    ));
    assert_eq!(
        active_and_available(&listed),
        (active, available),
        "listing the scopes must answer what the call that changed them answered, got {listed}"
    );
}

/// Detects a `memory_get` that drops its `session_key`: the tool call would be
/// recorded against no memory, and the statistics would answer "how often is
/// this memory fetched by the model rather than delivered to it" with a number
/// that counts nothing. A call recorded per scope or per answer line instead of
/// once would be just as wrong in the other direction.
#[test]
fn a_memory_fetched_through_mcp_is_recorded_as_one_tool_call_against_the_memory_it_read() {
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
        "the call must be counted against the memory it read, got {fetched:?}"
    );
}

/// Detects a refused operation answered as a success the model would act on,
/// and one whose text says nothing it can use: an MCP answer is text, so what
/// was wrong has to be in the text, naming the thing the call got wrong. A model
/// told only "failed" repeats the same call.
#[test]
fn a_refused_operation_is_an_error_result_whose_text_names_what_the_call_got_wrong() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    for (case, tool, arguments, named) in [
        (
            "a memory the store does not have",
            "memory_blame",
            json!({ "id": "bench-power-notes" }),
            "bench-power-notes",
        ),
        (
            "a commit message the history cannot show",
            "memory_replace_text",
            json!({
                "session_key": "alpha/session-7",
                "id": "bench-power",
                "old_string": "at the wall",
                "new_string": "at the wall switch",
                "message": "   "
            }),
            "message",
        ),
        (
            "an id that is no scope of this store",
            "session_scope_on",
            json!({ "session_key": "alpha/session-1", "scopes": ["spanners"] }),
            "spanners",
        ),
    ] {
        let refused = session.call(tool, arguments);
        assert_eq!(
            refused.is_error,
            Some(true),
            "{case} must be answered as a failure, got {}",
            tool_text(&refused)
        );
        let text = tool_text(&refused);
        assert!(
            text.contains(named),
            "the refusal of {case} must name {named:?}, got {text:?}"
        );
    }
}

/// Detects a `session_key` that is not a context key being taken anyway: a key
/// with a part missing names a context no session ever reads, so the scopes
/// would be turned on for nobody and the model would be told it worked. A
/// malformed key is a parameter this server will not act on at all, which is a
/// protocol error rather than a failed operation, and it must leave the session
/// able to go on calling.
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

/// Detects a land that reports a conflict the model cannot act on: the operation
/// answers with the three versions of each file, and an MCP answer is text, so a
/// failure that names neither the file nor the two texts leaves the model with
/// nothing to write on the branch and no way to resolve. The branch has to be
/// left open, because that is where the resolution is written.
#[test]
fn a_conflicting_land_through_mcp_reports_the_file_and_both_texts_as_text() {
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
