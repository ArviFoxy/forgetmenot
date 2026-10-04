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
//! The expectations come from the plan's tool families and from the example
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

/// The scope tools, which read and change the scopes themselves.
const SCOPE_TOOLS: [&str; 6] = [
    "scope_index",
    "scope_get",
    "scope_put",
    "scope_delete",
    "scope_activations",
    "scope_activation_get",
];

/// The settings tools, which read and change the store's behaviour settings.
const SETTINGS_TOOLS: [&str; 2] = ["settings_get", "settings_set"];

/// The tools that write to the store, each of which takes an optional branch.
const WRITE_TOOLS: [&str; 8] = [
    "memory_put",
    "memory_replace_text",
    "memory_set_fields",
    "memory_rename",
    "memory_delete",
    "scope_put",
    "scope_delete",
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

/// What a scope tool's description has to say: what a scope is.
const SCOPE_FAMILY_PHRASE: &str = "label that groups memories";

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
/// store, a scope tool that does not say what a scope is, a branch tool that
/// does not say what a branch is for, or a session tool that does not say the
/// store is untouched, would have the model committing to everyone's store when
/// it meant to change its own scopes.
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
        } else if SCOPE_TOOLS.contains(&tool.name.as_ref()) {
            SCOPE_FAMILY_PHRASE
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
        "scope management",
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
            "scope": "global",
            "source": "assistant",
            "body": "# Bracket torque\n\nTorque the bracket bolts to 9 Nm in two passes.\n",
            "commit_title": "record the bracket torque"
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

/// Detects a `commit_body` that does not reach the commit, that is cut at its
/// first blank line or folded onto one line, or that ends up after the server's
/// own lines instead of before them: the model gives its reasons there, and
/// `memory_history` is where it and every later session read them back.
/// Expectation source: the commit body contract, where the body is the
/// writer's text, trimmed, then a blank line and the server's `memory:` and
/// `author:` lines.
#[test]
fn a_commit_body_written_through_mcp_reads_back_word_for_word_in_the_memory_history() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let session_key = "alpha/session-7";
    let commit_body = "The torque came from the bracket maker's sheet.\n\n\
                       Two passes keep the bracket from warping:\n- first pass at 5 Nm\n- second at 9 Nm";

    let written = session.call(
        "memory_put",
        json!({
            "session_key": session_key,
            "id": "bracket-torque",
            "description": "Bracket bolts are torqued to 9 Nm, in two passes",
            "kind": "critical",
            "scope": "global",
            "source": "assistant",
            "body": "# Bracket torque\n\nTorque the bracket bolts to 9 Nm in two passes.\n",
            "commit_title": "record the bracket torque",
            "commit_body": format!("\n{commit_body}\n\n"),
        }),
    );
    assert_ne!(
        written.is_error,
        Some(true),
        "the write must be answered as done, got {}",
        tool_text(&written)
    );

    let history = tool_json(&session.call("memory_history", json!({ "id": "bracket-torque" })));
    assert_eq!(
        history[0]["body"],
        json!(format!(
            "{commit_body}\n\nmemory: bracket-torque\nauthor: {session_key}"
        )),
        "the commit's body must be the body sent, trimmed, then the server's lines, got {history}"
    );
}

/// Detects a `scope` filter that is ignored or inverted. The index is the only
/// way the model finds a memory it has not been given, and a filter that answers
/// with the memories of every other scope, or with nothing, makes it useless for
/// the one scope it asked about.
#[test]
fn the_memory_index_filtered_by_scope_lists_the_memories_of_that_scope_only() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let listed = tool_json(&session.call("memory_index", json!({ "scope": "widgets" })));

    let ids: BTreeSet<String> = listed
        .as_array()
        .expect("the index is an array")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from(["widget-naming".to_string()]),
        "only the memories in the named scope may be listed"
    );
    let all = tool_json(&session.call("memory_index", json!({})));
    assert!(
        all.as_array().is_some_and(|summaries| summaries.len() > 1),
        "without a filter every memory must be listed, got {all}"
    );
}

// ---------------------------------------------------------------------------
// The scope tools, which define the scopes the memories are grouped by.
// ---------------------------------------------------------------------------

/// The id of the scope the tests below define, which the example store does not
/// have.
const NEW_SCOPE: &str = "lathe";

/// The file that scope is written to, which is what a refusal names.
const NEW_SCOPE_PATH: &str = "scopes/lathe.yaml";

/// A line of the memory written into the new scope, in no other memory of the
/// store and in no description, so that "the new scope delivered its memory" can
/// be told apart from "something the example store already had arrived".
const LATHE_RULE: &str = "The chuck key never stays in the chuck";

/// The scope ids `scope_index` reports.
fn indexed_scopes(session: &common::McpSession<'_>) -> BTreeSet<String> {
    tool_json(&session.call("scope_index", json!({})))
        .as_array()
        .expect("the index is an array")
        .iter()
        .map(|row| row["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The version `scope_get` reports for one scope, which is what a write is made
/// against.
fn scope_version(session: &common::McpSession<'_>, id: &str) -> String {
    let document = tool_json(&session.call("scope_get", json!({ "id": id })));
    document["version"]
        .as_str()
        .unwrap_or_else(|| panic!("a scope reports the version to write against, got {document}"))
        .to_string()
}

/// How many commits the store has on main, for the assertions that a refused
/// write left the store as it was.
fn commit_count(server: &TestServer) -> usize {
    let (status, answer) = server.api("GET", "/api/history", None);
    assert_eq!(status, 200, "the history must be readable, got {answer}");
    answer["commits"]
        .as_array()
        .unwrap_or_else(|| panic!("the history lists its commits, got {answer}"))
        .len()
}

/// Detects a scope written through MCP that the server never acts on: the file
/// would be in the store, its trigger would never fire, and the memory the agent
/// filed under the scope it had just defined would reach no session, with both
/// calls answered as done. Nothing short of a hook event after the write sees
/// this, because a scope only does anything when a trigger matches.
///
/// Source: issue 22, where a scope created with a `user_message` trigger
/// activates at the next matching prompt and its memory is delivered.
#[test]
fn a_scope_created_through_mcp_fires_at_the_next_matching_prompt_and_delivers_its_memory() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    // Written by another session than the one the prompt comes from: a writer is
    // recorded as holding what it wrote, and this is about what the trigger
    // delivers, not about what a writer is spared.
    let session_key = "alpha/session-9";

    let written = session.call(
        "scope_put",
        json!({
            "session_key": session_key,
            "id": NEW_SCOPE,
            "implies": [],
            "triggers": [{ "on": "user_message", "pattern": "\\blathe\\b" }],
            "commit_title": "add a scope for the lathe",
        }),
    );
    assert_ne!(
        written.is_error,
        Some(true),
        "the scope must be written, got {}",
        tool_text(&written)
    );
    let filed = session.call(
        "memory_put",
        json!({
            "session_key": session_key,
            "id": "lathe-chuck-key",
            "description": "How the chuck key is handled at the lathe",
            "kind": "critical",
            "scope": NEW_SCOPE,
            "source": "assistant",
            "body": format!("# Chuck key\n\n{LATHE_RULE}.\n"),
            "commit_title": "record what happens to the chuck key",
        }),
    );
    assert_ne!(
        filed.is_error,
        Some(true),
        "a memory must be filable under the new scope, got {}",
        tool_text(&filed)
    );

    server.hook(
        "alpha",
        Some(10_000),
        &common::hook_fixture("session_start"),
    );
    let (status, answer) = server.hook(
        "alpha",
        Some(10_000),
        &json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "session-1",
            "cwd": "/home/dev/widgets",
            "prompt": "face the end of the bar on the lathe"
        }),
    );

    assert_eq!(status, 200, "the prompt must be answered, got {answer}");
    let delivered = common::additional_context(&answer).unwrap_or_default();
    assert!(
        delivered.contains(LATHE_RULE),
        "the prompt matched the new scope's trigger, so its memory must be delivered, got \
         {delivered:?}"
    );
}

/// Detects a create at an id that already has a file being written anyway: a
/// scope somebody else defined would be replaced whole, triggers and implies and
/// all, by a caller that never read it. The refusal has to name the version the
/// store holds, which is what the caller sends back to write against.
///
/// Source: issue 22, where a create at an existing id without `base_version` is
/// refused with the current version.
#[test]
fn creating_a_scope_at_an_id_that_has_a_file_is_refused_and_names_the_version_the_store_holds() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let before = tool_json(&session.call("scope_get", json!({ "id": "widgets" })));
    let version = scope_version(&session, "widgets");

    let refused = session.call(
        "scope_put",
        json!({
            "session_key": "alpha/session-7",
            "id": "widgets",
            "implies": [],
            "triggers": [],
            "commit_title": "take the widgets scope over",
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "a create at an id that has a file must be refused, got {}",
        tool_text(&refused)
    );
    let text = tool_text(&refused);
    assert!(
        text.contains(&version),
        "the refusal must name the version the store holds, {version} is not in {text:?}"
    );
    assert_eq!(
        tool_json(&session.call("scope_get", json!({ "id": "widgets" }))),
        before,
        "a refused create must leave the scope exactly as it was"
    );
}

/// Detects a write made against a version that is no longer current: the change
/// somebody else committed in between would be overwritten silently, and the
/// caller would be told its write was done. The refusal names the current
/// version, which is what the caller reads and writes against to try again.
///
/// Source: issue 22, where an update from a stale `base_version` is refused with
/// the current document.
#[test]
fn updating_a_scope_from_a_version_that_is_no_longer_current_is_refused_with_the_current_one() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let session_key = "alpha/session-7";
    let stale = scope_version(&session, "widgets");
    let meanwhile = session.call(
        "scope_put",
        json!({
            "session_key": session_key,
            "id": "widgets",
            "implies": [],
            "triggers": [{ "on": "user_message", "pattern": "\\bwidget\\b" }],
            "base_version": stale,
            "commit_title": "match widgets in prompts only",
        }),
    );
    assert_ne!(
        meanwhile.is_error,
        Some(true),
        "the first update must be accepted, got {}",
        tool_text(&meanwhile)
    );
    let current = scope_version(&session, "widgets");

    let refused = session.call(
        "scope_put",
        json!({
            "session_key": session_key,
            "id": "widgets",
            "implies": ["rocketry"],
            "triggers": [],
            "base_version": stale,
            "commit_title": "drop the widget triggers",
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "a write against a version that has moved on must be refused, got {}",
        tool_text(&refused)
    );
    let text = tool_text(&refused);
    assert!(
        text.contains(&current),
        "the refusal must name the version the store holds now, {current} is not in {text:?}"
    );
    let document = tool_json(&session.call("scope_get", json!({ "id": "widgets" })));
    assert_eq!(
        document["triggers"].as_array().map(Vec::len),
        Some(1),
        "the refused write must not have replaced what the accepted one wrote, got {document}"
    );
}

/// Detects a trigger pattern that reaches the store without being compiled: the
/// scope file would hold a pattern that can never match, so the scope would
/// never turn on and the store would need a hand edit to fix. The refusal names
/// the file, because that is what the caller is being told is unwritable, and a
/// refused write commits nothing at all.
///
/// Source: issue 22, where a pattern that does not compile is refused the way
/// the API refuses it, which is the store's validation rule that every trigger
/// pattern compiles.
#[test]
fn a_scope_whose_trigger_pattern_does_not_compile_is_refused_naming_the_file_and_commits_nothing() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let before = commit_count(&server);

    let refused = session.call(
        "scope_put",
        json!({
            "session_key": "alpha/session-7",
            "id": NEW_SCOPE,
            "implies": [],
            "triggers": [{ "on": "user_message", "pattern": "(lathe" }],
            "commit_title": "add a scope for the lathe",
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "a pattern that does not compile must be refused, got {}",
        tool_text(&refused)
    );
    let text = tool_text(&refused);
    assert!(
        text.contains(NEW_SCOPE_PATH),
        "the refusal must name the file it would not write, got {text:?}"
    );
    assert_eq!(
        commit_count(&server),
        before,
        "a refused write must leave the store without a commit"
    );
    assert!(
        !indexed_scopes(&session).contains(NEW_SCOPE),
        "the refused scope must not exist"
    );
}

/// Detects a scope deleted out from under the memories that are in it: every one
/// of them would name a scope that is not there, which makes the whole store
/// invalid, and the memories would be delivered to nobody. The refusal names the
/// memory, because moving it is what the caller has to do first.
///
/// Source: issue 22, where a delete of a referenced scope is refused naming the
/// memory, as `DELETE /api/scopes/{id}` refuses it.
#[test]
fn deleting_a_scope_a_memory_is_in_is_refused_naming_that_memory() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let refused = session.call(
        "scope_delete",
        json!({
            "session_key": "alpha/session-7",
            "id": "widgets",
            "commit_title": "drop the widgets scope",
        }),
    );

    assert_eq!(
        refused.is_error,
        Some(true),
        "deleting a scope a memory is in must be refused, got {}",
        tool_text(&refused)
    );
    let text = tool_text(&refused);
    assert!(
        text.contains("widget-naming"),
        "the refusal must name the memory that is in the scope, got {text:?}"
    );
    assert!(
        indexed_scopes(&session).contains("widgets"),
        "the refused delete must leave the scope where it was"
    );
}

/// Detects a delete that answers as done and leaves the file in place, which
/// would keep the scope's triggers firing in every session after the caller was
/// told the scope was gone.
///
/// Source: issue 22, where deleting an unreferenced scope removes it and
/// `scope_index` stops listing it. `workshop` is the example store's scope that
/// no memory is in and no other scope implies.
#[test]
fn deleting_a_scope_nothing_names_removes_it_and_the_index_stops_listing_it() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    assert!(
        indexed_scopes(&session).contains("workshop"),
        "the scope to delete must exist first"
    );

    let deleted = session.call(
        "scope_delete",
        json!({
            "session_key": "alpha/session-7",
            "id": "workshop",
            "commit_title": "drop the workshop scope, nothing is in it",
        }),
    );

    assert_ne!(
        deleted.is_error,
        Some(true),
        "a scope nothing names must be deletable, got {}",
        tool_text(&deleted)
    );
    assert!(
        !indexed_scopes(&session).contains("workshop"),
        "a deleted scope must stop being listed"
    );
}

/// Detects `global`, `machine:<name>` and `session:<machine>/<id>` answered with
/// a document or reported as not existing: they are scopes that do exist and
/// have no file, so a model told "no such scope" would try to create one, and a
/// model given a document would try to edit a file nothing reads.
///
/// Source: issue 22, where a scope with no file is refused saying it has none.
#[test]
fn reading_a_scope_that_has_no_file_is_refused_as_having_none_rather_than_as_missing() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let refused = session.call("scope_get", json!({ "id": "global" }));

    assert_eq!(
        refused.is_error,
        Some(true),
        "a scope with no file has nothing to read, got {}",
        tool_text(&refused)
    );
    let text = tool_text(&refused);
    assert!(
        text.contains("global") && text.contains("no file"),
        "the refusal must name the scope and say it has no file, got {text:?}"
    );
}

/// Detects a scope write that ignores the branch it was given: the scope would
/// be in force in every session at once, which is exactly what a branch is for
/// avoiding, and the land would have nothing left to do.
///
/// Source: issue 22, where a scope written on a branch is invisible until the
/// branch lands.
#[test]
fn a_scope_written_on_a_branch_is_invisible_until_the_branch_lands() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();
    let session_key = "alpha/session-12";
    let branch =
        tool_json(&session.call("branch_create", json!({ "session_key": session_key })))["branch"]
            .as_str()
            .expect("branch_create names the branch")
            .to_string();

    let written = session.call(
        "scope_put",
        json!({
            "session_key": session_key,
            "id": NEW_SCOPE,
            "implies": [],
            "triggers": [{ "on": "user_message", "pattern": "\\blathe\\b" }],
            "commit_title": "add a scope for the lathe",
            "branch": branch,
        }),
    );

    assert_ne!(
        written.is_error,
        Some(true),
        "a scope must be writable on a branch, got {}",
        tool_text(&written)
    );
    assert!(
        !indexed_scopes(&session).contains(NEW_SCOPE),
        "a scope written on a branch must not exist on main"
    );
    let landed = session.call(
        "branch_land",
        json!({
            "session_key": session_key,
            "branch": branch,
            "commit_title": "add a scope for the lathe",
        }),
    );
    assert_ne!(
        landed.is_error,
        Some(true),
        "the branch must land, got {}",
        tool_text(&landed)
    );
    assert!(
        indexed_scopes(&session).contains(NEW_SCOPE),
        "the landed scope must exist on main"
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
        .memory_stats(&forgetmenot_server::stats::Filter::all())
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
            "a commit title the history cannot show",
            "memory_replace_text",
            json!({
                "session_key": "alpha/session-7",
                "id": "bench-power",
                "old_string": "at the wall",
                "new_string": "at the wall switch",
                "commit_title": "   "
            }),
            "commit title",
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
            "commit_title": "say what the meter reads",
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
            "commit_title": "say what the meter reads on main",
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
            "commit_title": "say what the meter reads",
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

// ---------------------------------------------------------------------------
// The activation log tools.
// ---------------------------------------------------------------------------

/// A prompt of the example store that turns `widgets` on by its user-message
/// trigger, and `rocketry` through `widgets`.
const WIDGET_PROMPT: &str = "rename the widget brackets";

/// Send `prompt` as the user's prompt in `session` on `alpha`.
fn prompt_in(server: &TestServer, session: &str, prompt: &str) {
    let (status, answer) = server.hook(
        "alpha",
        Some(10_000),
        &json!({
            "session_id": session,
            "transcript_path": "/nonexistent/transcript.jsonl",
            "cwd": "/home/dev/notes",
            "permission_mode": "default",
            "hook_event_name": "UserPromptSubmit",
            "prompt": prompt,
        }),
    );
    assert_eq!(status, 200, "the prompt must be answered, got {answer}");
}

/// Start the subagent `agent_id` of `session` on `alpha`.
fn subagent_in(server: &TestServer, session: &str, agent_id: &str) {
    let (status, answer) = server.hook(
        "alpha",
        Some(10_000),
        &json!({
            "session_id": session,
            "transcript_path": "/nonexistent/transcript.jsonl",
            "cwd": "/home/dev/notes",
            "permission_mode": "default",
            "hook_event_name": "SubagentStart",
            "agent_id": agent_id,
            "agent_type": "general-purpose",
        }),
    );
    assert_eq!(
        status, 200,
        "the subagent start must be answered, got {answer}"
    );
}

/// The entries of one `scope_activations` answer.
fn listed_entries(answer: &Value) -> Vec<Value> {
    answer["entries"]
        .as_array()
        .expect("the answer lists its entries in an array")
        .clone()
}

/// The values of one key of every entry, in the order listed.
fn values_of(entries: &[Value], key: &str) -> Vec<Value> {
    entries.iter().map(|entry| entry[key].clone()).collect()
}

/// Detects a list that carries the whole text a trigger matched, which a tool
/// answer listing many entries cannot afford, and an entry that leaves out what
/// the caller needs to tell why the scope came on: the trigger as written, the
/// field, what the regex found and where, and the length of the text. Detects
/// too a `scope_activation_get` that does not return that text whole, or that
/// describes the entry differently from the list.
///
/// Expectation source: the example store's `widgets` trigger, `\bwidget\b` on
/// what the user writes, and the prompt, in which `widget` follows the eleven
/// characters of `rename the `.
#[test]
fn the_activation_list_leaves_the_message_out_and_one_entry_is_read_with_it() {
    let server = TestServer::start(example_store_files(), |_| {});
    prompt_in(&server, "session-1", WIDGET_PROMPT);
    let session = server.mcp();

    let listed = tool_json(&session.call("scope_activations", json!({ "scope": "widgets" })));

    let entries = listed_entries(&listed);
    assert_eq!(entries.len(), 1, "widgets came on once, got {listed}");
    let entry = &entries[0];
    let keys: BTreeSet<&str> = entry
        .as_object()
        .expect("an entry is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "id",
            "time",
            "context",
            "scope",
            "cause",
            "trigger_kind",
            "trigger",
            "scope_version",
            "field",
            "evidence",
            "message_id",
            "message_chars",
        ]),
        "a listed trigger entry carries its cause's details and not the message, got {entry}"
    );
    assert_eq!(
        (
            entry["cause"].clone(),
            entry["trigger"].clone(),
            entry["field"].clone()
        ),
        (
            json!("trigger"),
            json!({ "on": "user_message", "pattern": "\\bwidget\\b" }),
            json!("user_message"),
        ),
        "the entry names the trigger as the scope file writes it and the field it matched"
    );
    assert_eq!(
        entry["evidence"],
        json!({
            "kind": "regex",
            "pattern": "\\bwidget\\b",
            "start": 11,
            "end": 17,
            "matched": "widget",
        }),
        "the evidence is the pattern, the span in characters and the text in it"
    );
    assert_eq!(
        entry["message_chars"],
        json!(WIDGET_PROMPT.chars().count()),
        "the entry gives the length of the message it leaves out"
    );

    let read =
        tool_json(&session.call("scope_activation_get", json!({ "id": entry["id"].clone() })));
    assert_eq!(
        read["message"],
        json!(WIDGET_PROMPT),
        "one entry is read with the whole message"
    );
    let mut without_message = read.clone();
    without_message
        .as_object_mut()
        .expect("an entry is an object")
        .remove("message");
    assert_eq!(
        &without_message, entry,
        "an entry read on its own is the entry the list gives, with the message"
    );
}

/// Detects a filter of `scope_activations` that is ignored or reads the wrong
/// rows: a session's own entries without its subagents, one subagent alone, one
/// scope, and the two ends of a span of time.
///
/// Expectation source: `session-1` turns `widgets` and `rocketry` on, its
/// subagent inherits both, and an hour later `session-2` turns them on too.
#[test]
fn scope_activations_filters_by_session_subagent_scope_and_time() {
    let server = TestServer::start(example_store_files(), |_| {});
    prompt_in(&server, "session-1", WIDGET_PROMPT);
    subagent_in(&server, "session-1", "agent-1");
    server.advance(chrono::Duration::hours(1));
    prompt_in(&server, "session-2", WIDGET_PROMPT);
    let session = server.mcp();
    let contexts_of = |params: Value| -> BTreeSet<String> {
        let answer = tool_json(&session.call("scope_activations", params));
        values_of(&listed_entries(&answer), "context")
            .into_iter()
            .map(|context| context.as_str().expect("a context key").to_string())
            .collect()
    };

    assert_eq!(
        contexts_of(json!({ "session": "alpha/session-1" })),
        BTreeSet::from([
            "alpha/session-1".to_string(),
            "alpha/session-1/agent-1".to_string()
        ]),
        "a session's key reads the session and its subagents"
    );
    assert_eq!(
        contexts_of(json!({ "session": "alpha/session-1", "include_subagents": false })),
        BTreeSet::from(["alpha/session-1".to_string()]),
        "without its subagents a session's key reads the session alone"
    );
    assert_eq!(
        contexts_of(json!({ "session": "alpha/session-1/agent-1" })),
        BTreeSet::from(["alpha/session-1/agent-1".to_string()]),
        "a subagent's key reads that subagent alone"
    );
    let rocketry = tool_json(&session.call("scope_activations", json!({ "scope": "rocketry" })));
    assert_eq!(
        values_of(&listed_entries(&rocketry), "scope"),
        vec![json!("rocketry"); 3],
        "a scope reads that scope's entries: session-1, its subagent and session-2"
    );
    assert_eq!(
        contexts_of(json!({ "from": "2026-01-02T03:34:05Z" })),
        BTreeSet::from(["alpha/session-2".to_string()]),
        "from half an hour in reads what came on an hour in"
    );
    assert_eq!(
        contexts_of(json!({ "to": "2026-01-02T03:34:05+00:00" })),
        BTreeSet::from([
            "alpha/session-1".to_string(),
            "alpha/session-1/agent-1".to_string()
        ]),
        "to half an hour in reads what came on at the start"
    );
}

/// Detects pages that overlap, skip an entry, come in the wrong order, or end
/// with a cursor that leads nowhere: the model reads a long log one page after
/// another and has to see every entry once.
///
/// Expectation source: four entries, `widgets` and `rocketry` in each of two
/// sessions, whose ids rise in the order they were written.
#[test]
fn scope_activations_pages_both_ways_with_the_cursor_it_answers() {
    let server = TestServer::start(example_store_files(), |_| {});
    prompt_in(&server, "session-1", WIDGET_PROMPT);
    prompt_in(&server, "session-2", WIDGET_PROMPT);
    let session = server.mcp();
    let page = |params: Value| tool_json(&session.call("scope_activations", params));
    let every = values_of(&listed_entries(&page(json!({ "order": "oldest" }))), "id");
    assert_eq!(every.len(), 4, "the two prompts turned four scopes on");

    let newest = page(json!({ "limit": 3 }));
    assert_eq!(
        values_of(&listed_entries(&newest), "id"),
        vec![every[3].clone(), every[2].clone(), every[1].clone()],
        "newest first by default, as many as the limit"
    );
    assert_eq!(newest["next"], every[1], "the cursor is the last id listed");
    let rest = page(json!({ "limit": 3, "before": newest["next"].clone() }));
    assert_eq!(
        values_of(&listed_entries(&rest), "id"),
        vec![every[0].clone()],
        "before the cursor is the rest"
    );
    assert_eq!(rest["next"], Value::Null, "the last page has no cursor");

    let oldest = page(json!({ "order": "oldest", "limit": 3 }));
    assert_eq!(
        values_of(&listed_entries(&oldest), "id"),
        every[..3].to_vec(),
        "oldest first when asked"
    );
    let rest = page(json!({ "order": "oldest", "limit": 3, "after": oldest["next"].clone() }));
    assert_eq!(
        values_of(&listed_entries(&rest), "id"),
        vec![every[3].clone()],
        "after the cursor is the rest"
    );
}

/// Detects activation tools that answer an empty log on a server that records
/// none, which the model would read as no scope ever having come on.
///
/// Expectation source: the `--no-activation-log` flag.
#[test]
fn the_activation_tools_say_the_log_is_off_on_a_server_that_keeps_none() {
    let server = TestServer::start(example_store_files(), |config| {
        config.activation_log = false;
    });
    prompt_in(&server, "session-1", WIDGET_PROMPT);
    let session = server.mcp();

    for (tool, params) in [
        ("scope_activations", json!({})),
        ("scope_activation_get", json!({ "id": 1 })),
    ] {
        let answer = session.call(tool, params);
        assert_eq!(
            (answer.is_error, tool_text(&answer)),
            (
                Some(true),
                "the activation log is turned off on this server".to_string()
            ),
            "{tool} must say that the log is off"
        );
    }
}

/// Detects a time bound that is not a time being read as no bound, which lists
/// the whole log as if it were the span asked for.
#[test]
fn a_time_bound_that_is_not_iso_8601_is_an_invalid_parameter() {
    let server = TestServer::start(example_store_files(), |_| {});
    let session = server.mcp();

    let refused = session
        .try_call("scope_activations", json!({ "from": "yesterday" }))
        .expect_err("a bound that is not a time must be refused");

    match &refused {
        ServiceError::McpError(error) => assert_eq!(
            error.code,
            ErrorCode::INVALID_PARAMS,
            "a malformed bound must be reported as an invalid parameter, got {refused}"
        ),
        other => panic!("a malformed bound must be an MCP error, got {other}"),
    }
}
