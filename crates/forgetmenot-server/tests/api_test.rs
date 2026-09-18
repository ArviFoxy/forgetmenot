//! The contract of the JSON API: for each route, the status code it answers
//! with, the shape of the JSON it carries, and the parameters it parses out of
//! the path, the query string and the body.
//!
//! Nothing about memories, scopes, sessions or the store is decided here: a
//! handler reads a request, calls one function in `operations` and turns the
//! answer into a status and a body. What those functions guarantee is
//! `operations_test.rs`, what the store guarantees underneath them is
//! `store_test.rs` and `branch_test.rs`, and what emerges when a session runs is
//! the `scenarios_*` suites. What is asserted here is only visible over HTTP:
//! the codes and bodies the frontend is built against, and the mapping from an
//! operation's failure to one of them. The two writer tests at the end are the
//! exception: several writers at once is a property of the running server.
//!
//! The expectations come from the plan's write contract, from the status codes
//! `api/mod.rs` documents, and from the example store committed in this
//! repository.

mod common;

use std::collections::BTreeMap;

use serde_json::{Value, json};

use common::{
    TestServer, api_request, example_store_files, example_store_without_settings, tool_json,
};

/// The author name a write carries; the frontend sends this one.
const AUTHOR: &str = "wiki";

/// A line that appears in no memory of the example store, so that "the write
/// landed" can be told apart from "the old text is still there".
const NEW_LINE: &str = "The binder lives on the shelf by the door.";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// One memory as the API reports it, failing the test when it cannot be read.
fn document(server: &TestServer, id: &str) -> Value {
    let (status, answer) = server.api("GET", &format!("/api/memories/{id}"), None);
    assert_eq!(status, 200, "reading {id} must succeed, got {answer}");
    answer
}

/// One scope as the API reports it, failing the test when it cannot be read.
fn scope(server: &TestServer, id: &str) -> Value {
    let (status, answer) = server.api("GET", &format!("/api/scopes/{id}"), None);
    assert_eq!(
        status, 200,
        "reading the scope {id} must succeed, got {answer}"
    );
    answer
}

/// A write of `body` to the version `document` was read at.
fn write_of(document: &Value, body: &str, message: &str) -> Value {
    json!({
        "description": document["description"],
        "kind": document["kind"],
        "scopes": document["scopes"],
        "source": document["source"],
        "body": body,
        "base_version": document["version"],
        "author": AUTHOR,
        "message": message,
    })
}

/// The store's head, read through the test's own handle on the repository.
fn head(server: &TestServer) -> String {
    server
        .store()
        .repository()
        .head_oid()
        .expect("the store has a head")
        .to_string()
}

/// The ids one list answer reports, in the order it lists them.
fn ids(answer: &Value) -> Vec<String> {
    answer
        .as_array()
        .unwrap_or_else(|| panic!("the answer must be a list, got {answer}"))
        .iter()
        .map(|row| row["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The row one key has in the answer of `GET /api/contexts`.
fn context_row<'answer>(answer: &'answer Value, key: &str) -> &'answer Value {
    answer
        .as_array()
        .expect("the contexts are a list")
        .iter()
        .find(|row| row["key"] == json!(key))
        .unwrap_or_else(|| panic!("{key} must be listed, got {answer}"))
}

// ---------------------------------------------------------------------------
// The memory routes
// ---------------------------------------------------------------------------

/// Detects a write route that answers 200 without saying what it committed: the
/// editor needs the version to send as the next write's `base_version`, and the
/// commit to link to, or it can neither save again nor show what it just did.
/// Detects a body whose `body` and `description` never reach the operation as
/// well, which would answer success for a write of something else.
#[test]
fn putting_a_memory_answers_200_with_the_commit_and_the_version_it_wrote() {
    let server = TestServer::start(example_store_files(), |_| {});
    let description = "The workshop references are in the binder on the shelf by the door";
    let body = format!("# Where the workshop references live\n\n{NEW_LINE}\n");
    let mut request = write_of(
        &document(&server, "reading-list"),
        &body,
        "note where the binder lives",
    );
    request["description"] = json!(description);

    let (status, answer) = server.api("PUT", "/api/memories/reading-list", Some(&request));

    assert_eq!(status, 200, "the write must be accepted, got {answer}");
    assert_eq!(
        answer["commit_oid"].as_str(),
        Some(head(&server).as_str()),
        "the answer must name the commit the write made, got {answer}"
    );
    let written = document(&server, "reading-list");
    assert_eq!(
        (written["body"].as_str(), written["description"].as_str()),
        (Some(body.as_str()), Some(description)),
        "the document must read back with the body and the description that were written"
    );
    assert_eq!(
        written["version"].as_str(),
        answer["version"].as_str(),
        "the write must report the version the document now has, got {answer}"
    );
}

/// Detects a create route that does not take the id out of the body, or that
/// answers without the commit: a memory created through the page would be
/// unreachable at the id it was given, and the page would have nothing to
/// navigate to.
#[test]
fn creating_a_memory_answers_200_with_the_commit_and_the_memory_reads_back_at_its_id() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api(
        "POST",
        "/api/memories",
        Some(&json!({
            "id": "bracket-tolerances",
            "description": "Bracket holes are reamed after welding, not before",
            "kind": "knowledge",
            "scopes": ["widgets"],
            "source": "user",
            "body": "# Bracket tolerances\n\nReam the holes after welding.\n",
            "author": AUTHOR,
            "message": "record the bracket tolerance",
        })),
    );

    assert_eq!(status, 200, "the create must be accepted, got {answer}");
    assert_eq!(
        answer["commit_oid"].as_str(),
        Some(head(&server).as_str()),
        "the answer must name the commit the create made, got {answer}"
    );
    let created = document(&server, "bracket-tolerances");
    assert_eq!(
        (created["kind"].as_str(), created["scopes"].clone()),
        (Some("knowledge"), json!(["widgets"])),
        "the memory must read back with the kind and the scopes the body gave, got {created}"
    );
}

/// Detects an index route that drops its query string, or that answers a filter
/// it cannot read with an empty list: a page asking for one scope's memories
/// would be given the whole store, and a mistyped filter would look like a scope
/// with nothing in it.
#[test]
fn the_memory_index_route_applies_its_filters_and_refuses_a_kind_it_has_no_notion_of() {
    let server = TestServer::start(example_store_files(), |_| {});
    let listed = |query: &str| {
        let (status, answer) = server.api("GET", &format!("/api/memories{query}"), None);
        assert_eq!(status, 200, "the index must be readable, got {answer}");
        ids(&answer)
    };

    // `widget-naming` is the example store's only memory in `widgets`, and it
    // is critical.
    assert_eq!(
        listed("?scope=widgets"),
        vec!["widget-naming"],
        "only the memories carrying the scope asked for may be listed"
    );
    assert_eq!(
        listed("?scope=widgets&kind=knowledge"),
        Vec::<String>::new(),
        "both filters must apply, so a critical memory must not answer a knowledge filter"
    );
    assert!(
        listed("").len() > 1,
        "without a filter every memory must be listed"
    );

    let (status, answer) = server.api("GET", "/api/memories?kind=rules", None);
    assert_eq!(
        status, 400,
        "a kind this route has no notion of must be refused, got {answer}"
    );
    assert!(
        answer["error"]
            .as_str()
            .is_some_and(|message| message.contains("critical")),
        "the refusal must say which kinds the route takes, got {answer}"
    );
}

/// Detects the `fields` action being routed to the whole-document write, which
/// would need a body the page does not send and would truncate the memory, and
/// an action this API has no notion of being answered as a write.
#[test]
fn setting_one_field_answers_200_and_a_path_naming_no_action_is_reported_as_missing() {
    let server = TestServer::start(example_store_files(), |_| {});
    let before = document(&server, "bench-power");

    let (status, answer) = server.api(
        "POST",
        "/api/memories/bench-power/fields",
        Some(&json!({
            "description": "Cut bench power at the wall and prove the rail reads zero",
            "author": AUTHOR,
            "message": "sharpen the bench power description",
        })),
    );

    assert_eq!(
        status, 200,
        "the field write must be accepted, got {answer}"
    );
    let after = document(&server, "bench-power");
    assert_eq!(
        after["description"].as_str(),
        Some("Cut bench power at the wall and prove the rail reads zero"),
        "the field the write named must have changed, got {after}"
    );
    assert_eq!(
        after["body"], before["body"],
        "a field write must leave the body alone, got {after}"
    );

    let (status, answer) = server.api(
        "POST",
        "/api/memories/bench-power/sharpen",
        Some(&json!({ "author": AUTHOR, "message": "m" })),
    );
    assert_eq!(
        status, 404,
        "a path naming no action of this API must be reported as missing, got {answer}"
    );
}

/// Detects a delete that answers 200 while the memory is still readable: the
/// page navigates away on the answer, and the answer names the commit it links
/// to.
#[test]
fn deleting_a_memory_answers_200_with_the_commit_and_the_memory_is_then_missing() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api(
        "DELETE",
        "/api/memories/bench-power",
        Some(&json!({
            "base_version": document(&server, "bench-power")["version"],
            "author": AUTHOR,
            "message": "the bench was taken out of the workshop",
        })),
    );

    assert_eq!(status, 200, "the delete must be accepted, got {answer}");
    assert_eq!(
        answer["commit_oid"].as_str(),
        Some(head(&server).as_str()),
        "the answer must name the commit the delete made, got {answer}"
    );
    let (status, gone) = server.api("GET", "/api/memories/bench-power", None);
    assert_eq!(
        status, 404,
        "a deleted memory must no longer be readable, got {gone}"
    );
    let (status, index) = server.api("GET", "/api/memories", None);
    assert_eq!(status, 200, "the index must be readable, got {index}");
    assert!(
        !ids(&index).contains(&"bench-power".to_string()),
        "a deleted memory must not be in the index, got {index}"
    );
}

/// Detects a memory id with slashes in it being split by the route, which is
/// every memory in a session silo: the page that reads one would be answered
/// with nothing, or with another memory.
#[test]
fn a_memory_id_with_slashes_is_read_and_its_history_is_addressable_under_the_same_path() {
    let server = TestServer::start(example_store_files(), |_| {});
    let id = "sessions/alpha/session-1/notes";

    let read = document(&server, id);

    assert_eq!(
        read["id"].as_str(),
        Some(id),
        "the memory must be answered under the id that was asked for, got {read}"
    );
    let (status, commits) = server.api("GET", &format!("/api/memories/{id}/history"), None);
    assert_eq!(
        status, 200,
        "the history of a memory in a silo must be readable, got {commits}"
    );
    assert!(
        !commits.as_array().expect("a history is a list").is_empty(),
        "the memory's history must carry the commit that wrote it, got {commits}"
    );
}

/// Detects a history route that does not report who wrote a commit or why, and
/// a per-commit route that carries neither the change nor the file: those four
/// values are the whole of what the history page shows.
#[test]
fn the_memory_history_routes_answer_the_commits_and_one_commits_diff_and_content() {
    let server = TestServer::start(example_store_files(), |_| {});
    let message = "note where the binder lives";
    let (status, _) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &document(&server, "reading-list"),
            &format!("# Where the workshop references live\n\n{NEW_LINE}\n"),
            message,
        )),
    );
    assert_eq!(status, 200, "the write must land");

    let (status, commits) = server.api("GET", "/api/memories/reading-list/history", None);

    assert_eq!(status, 200, "the history must be readable, got {commits}");
    let newest = &commits.as_array().expect("a history is a list")[0];
    assert_eq!(
        (newest["title"].as_str(), newest["author"].as_str()),
        (Some(message), Some(AUTHOR)),
        "the newest commit must carry the message and the author the write sent, got {commits}"
    );

    let oid = newest["oid"].as_str().expect("a commit has an oid");
    let (status, entry) = server.api(
        "GET",
        &format!("/api/memories/reading-list/history/{oid}"),
        None,
    );
    assert_eq!(status, 200, "the commit must be readable, got {entry}");
    assert!(
        entry["diff"]
            .as_str()
            .is_some_and(|diff| diff.lines().any(|line| line == format!("+{NEW_LINE}"))),
        "the entry must carry the change that commit made, got {entry}"
    );
    assert!(
        entry["content"]
            .as_str()
            .is_some_and(|content| content.contains(NEW_LINE)),
        "the entry must carry the file as it was at that commit, got {entry}"
    );
}

/// Detects a store history route that ignores `limit` or `before`, which would
/// leave every page but the first unreachable, and one that answers a limit it
/// cannot read with a page of some other size. Detects a commit route that says
/// nothing about the files as well.
#[test]
fn the_store_history_routes_parse_limit_and_before_and_answer_a_commits_files() {
    let server = TestServer::start(example_store_files(), |_| {});
    let (status, _) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &document(&server, "reading-list"),
            &format!("# Where the workshop references live\n\n{NEW_LINE}\n"),
            "note where the binder lives",
        )),
    );
    assert_eq!(status, 200, "the write must land");
    let page = |query: &str| {
        let (status, answer) = server.api("GET", &format!("/api/history{query}"), None);
        assert_eq!(
            status, 200,
            "the store's history must be readable, got {answer}"
        );
        answer
    };

    let first = page("?limit=1");
    assert_eq!(
        first["commits"].as_array().map(Vec::len),
        Some(1),
        "a limit of one must answer with one commit, got {first}"
    );
    assert_eq!(
        first["commits"][0]["title"].as_str(),
        Some("note where the binder lives"),
        "the newest commit must come first, got {first}"
    );
    let before = first["next_before"]
        .as_str()
        .unwrap_or_else(|| panic!("a full page must name the next page's start, got {first}"));
    let second = page(&format!("?limit=1&before={before}"));
    assert_ne!(
        second["commits"][0]["oid"], first["commits"][0]["oid"],
        "the next page must start after the commit it was given, got {second}"
    );

    let (status, answer) = server.api("GET", "/api/history?limit=none", None);
    assert_eq!(
        status, 400,
        "a limit this route cannot read must be refused, got {answer}"
    );

    let (status, commit) = server.api("GET", &format!("/api/history/{}", head(&server)), None);
    assert_eq!(status, 200, "the commit must be readable, got {commit}");
    let file = &commit["files"][0];
    assert_eq!(
        (
            file["path"].as_str(),
            file["status"].as_str(),
            file["memory_id"].as_str()
        ),
        (
            Some("memories/reading-list.md"),
            Some("modified"),
            Some("reading-list")
        ),
        "a commit's file must name its path, what happened to it and the memory it holds, got \
         {commit}"
    );
    assert!(
        file["diff"].is_string(),
        "a commit's file must carry its diff, got {commit}"
    );
}

// ---------------------------------------------------------------------------
// The scope routes
// ---------------------------------------------------------------------------

/// Detects a scope write whose body never reaches the file, and a scope index
/// that does not carry the file the edit page reads: the page that edits a scope
/// reads it back from the index row as well as from the scope itself, and a key
/// the store keeps but the API hides cannot be edited or even seen.
#[test]
fn writing_a_scope_answers_200_and_it_reads_back_through_the_scope_and_the_index_routes() {
    let server = TestServer::start(example_store_files(), |_| {});
    let read = scope(&server, "widgets");
    let message = "Part numbers are never renumbered.";
    let triggers = json!([{ "on": "tool_name", "pattern": "Bash", "machine": "alpha" }]);

    let (status, answer) = server.api(
        "PUT",
        "/api/scopes/widgets",
        Some(&json!({
            "implies": read["implies"],
            "triggers": triggers,
            "scope_message": message,
            "forget": { "tokens_since_trigger": 50000 },
            "base_version": read["version"],
            "author": AUTHOR,
            "message": "restrict the widgets trigger and give the scope a message",
        })),
    );

    assert_eq!(
        status, 200,
        "the scope write must be accepted, got {answer}"
    );
    let written = scope(&server, "widgets");
    assert_eq!(
        (
            written["message"].clone(),
            written["triggers"].clone(),
            written["forget"].clone()
        ),
        (
            json!(message),
            triggers.clone(),
            json!({ "tokens_since_trigger": 50000 })
        ),
        "the scope must read back with everything the write sent, got {written}"
    );

    let (status, index) = server.api("GET", "/api/scopes", None);
    assert_eq!(status, 200, "the scopes must be readable, got {index}");
    let row = index
        .as_array()
        .expect("the scope index is a list")
        .iter()
        .find(|row| row["id"] == json!("widgets"))
        .unwrap_or_else(|| panic!("widgets must be listed, got {index}"));
    assert_eq!(
        (row["kind"].clone(), row["file"]["message"].clone()),
        (json!("file"), json!(message)),
        "the index row must report the kind and carry the scope's file, got {row}"
    );
}

/// Detects a scope delete that answers 200 while the scope is still readable, or
/// that leaves it in the index a person picks scopes from: an editor would keep
/// offering a scope that is gone.
///
/// `workshop` is the example store's scope that no memory lists and no other
/// scope implies, so nothing about the store refuses this deletion.
#[test]
fn deleting_a_scope_answers_200_with_the_commit_and_the_scope_is_then_missing() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api(
        "DELETE",
        "/api/scopes/workshop",
        Some(&json!({
            "base_version": scope(&server, "workshop")["version"],
            "author": AUTHOR,
            "message": "the workshop directory moved off this machine",
        })),
    );

    assert_eq!(status, 200, "the delete must be accepted, got {answer}");
    assert_eq!(
        answer["commit_oid"].as_str(),
        Some(head(&server).as_str()),
        "the answer must name the commit the delete made, got {answer}"
    );
    let (status, gone) = server.api("GET", "/api/scopes/workshop", None);
    assert_eq!(
        status, 404,
        "a deleted scope must no longer be readable, got {gone}"
    );
    let (status, index) = server.api("GET", "/api/scopes", None);
    assert_eq!(status, 200, "the scopes must be readable, got {index}");
    assert!(
        !ids(&index).contains(&"workshop".to_string()),
        "a deleted scope must not be in the scope index, got {index}"
    );
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Detects a store with no settings file reported as anything other than the
/// documented defaults, and one reported with a version a write could send back:
/// an editor told the file exists would write against a version that is not
/// there, and one told the wrong defaults would show a behaviour nobody has. The
/// schema is what an editor offers the keys from, so a key it does not describe
/// is a key nobody can set.
///
/// Source: the defaults are the ones the README documents for a store nobody has
/// configured.
#[test]
fn the_settings_of_a_store_without_a_file_are_the_defaults_and_the_schema_describes_every_key() {
    let server = TestServer::start(example_store_without_settings(), |_| {});

    let (status, answer) = server.api("GET", "/api/settings", None);

    assert_eq!(status, 200, "the settings must be readable, got {answer}");
    assert_eq!(
        answer["version"],
        Value::Null,
        "a store with no settings file has no version, got {answer}"
    );
    assert_eq!(
        answer["settings"],
        json!({
            "reminder_tokens": null,
            "interrupt_on_critical": true,
            "interrupt_exempt_tools": [],
            "trigger_exempt_tools": [],
            "subagents_inherit_scopes": true,
            "deliver_knowledge_index": true,
            "tool_result_match_limit": 262_144,
            "answer_file_threshold": 10_000,
            "announce_empty_scopes": false,
            "characters_per_token": 3.5,
        }),
        "the defaults must be the documented ones, got {answer}"
    );
    let described: Vec<&str> = answer["schema"]
        .as_array()
        .expect("the schema is a list")
        .iter()
        .filter_map(|row| row["key"].as_str())
        .collect();
    assert_eq!(
        described.len(),
        answer["settings"]
            .as_object()
            .expect("the settings are an object")
            .len(),
        "the schema must describe every setting, got {answer}"
    );
}

/// Detects a settings write whose key never leaves the path, or whose answer
/// does not name the version: the editor sends that version back as the next
/// write's `base_version`, and without it every second change is a conflict.
#[test]
fn writing_a_setting_answers_200_with_the_version_the_next_write_sends_back() {
    let server = TestServer::start(example_store_without_settings(), |_| {});

    let (status, written) = server.api(
        "PUT",
        "/api/settings/reminder_tokens",
        Some(&json!({
            "value": 150_000,
            "author": AUTHOR,
            "message": "remind the agent of the rules every 150k tokens",
        })),
    );

    assert_eq!(status, 200, "the write must be accepted, got {written}");
    let (status, answer) = server.api("GET", "/api/settings", None);
    assert_eq!(status, 200, "the settings must be readable, got {answer}");
    assert_eq!(
        answer["settings"]["reminder_tokens"],
        json!(150_000),
        "the setting must read back as it was written, got {answer}"
    );
    assert_eq!(
        answer["version"], written["version"],
        "the write must report the version the next one sends back, got {written}"
    );
}

// ---------------------------------------------------------------------------
// The reads that answer a question about the live server
// ---------------------------------------------------------------------------

/// Detects a trigger test that reports nothing for a text a scope's pattern
/// matches, or that does not say which pattern matched: the page exists to show
/// a person why a scope turns on, and a hit without its field and its pattern
/// says only that something matched.
#[test]
fn the_trigger_test_route_names_the_scope_the_field_and_the_pattern_a_text_fires() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api(
        "POST",
        "/api/triggers/test",
        Some(&json!({
            "field": "tool_input",
            "text": "cargo test -p widgets",
            "machine": "beta",
        })),
    );

    assert_eq!(status, 200, "a trigger test must be answered, got {answer}");
    let widgets = answer["fired"]
        .as_array()
        .expect("the answer lists what fired")
        .iter()
        .find(|hit| hit["scope_id"] == json!("widgets"))
        .unwrap_or_else(|| panic!("the widgets trigger must fire on this text, got {answer}"));
    assert_eq!(
        widgets["field"],
        json!("tool_input"),
        "the hit must name the field it matched on, got {widgets}"
    );
    assert!(
        widgets["pattern"]
            .as_str()
            .is_some_and(|pattern| pattern.contains("widget")),
        "the hit must name the pattern that matched, got {widgets}"
    );
}

/// Detects a pattern check that calls a pattern the store would refuse good,
/// which would let an editor save a trigger that can never fire, and one that
/// reports the refusal without the compiler's message, which leaves the author
/// with nothing to fix.
#[test]
fn the_pattern_check_route_says_whether_a_pattern_compiles_and_what_is_wrong_with_it() {
    let server = TestServer::start(example_store_files(), |_| {});
    let check = |pattern: &str| {
        let (status, answer) = server.api(
            "POST",
            "/api/triggers/validate",
            Some(&json!({ "pattern": pattern })),
        );
        assert_eq!(
            status, 200,
            "a pattern check must be answered, got {answer}"
        );
        answer
    };

    let refused = check("[unclosed");
    assert_eq!(
        refused["ok"],
        json!(false),
        "a pattern that does not compile must not be reported as good, got {refused}"
    );
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "the answer must carry what was wrong with the pattern, got {refused}"
    );

    // One of the example store's own patterns, so a check that refuses it would
    // refuse a trigger the store already carries.
    let accepted = check(r"\brocket(s|ry)?\b");
    assert_eq!(
        accepted["ok"],
        json!(true),
        "a pattern that compiles must be reported as good, got {accepted}"
    );
    assert!(
        accepted.get("error").is_none(),
        "a pattern that compiles must carry no error, got {accepted}"
    );
}

/// Detects a machines route that answers something other than a list of names:
/// it is what the frontend offers wherever a machine is picked, so a machine
/// missing from it cannot be chosen at all.
#[test]
fn the_machines_route_answers_every_machine_that_has_sent_an_event_as_a_list_of_names() {
    let server = TestServer::start(example_store_files(), |_| {});
    for machine in ["alpha", "beta"] {
        server.hook(
            machine,
            Some(10_000),
            &common::hook_fixture("session_start"),
        );
    }

    let (status, answer) = server.api("GET", "/api/machines", None);

    assert_eq!(status, 200, "the machines must be readable, got {answer}");
    let machines: Vec<&str> = answer["machines"]
        .as_array()
        .expect("the answer lists the machines")
        .iter()
        .map(|machine| machine.as_str().expect("a machine name is text"))
        .collect();
    assert_eq!(
        machines,
        vec!["alpha", "beta"],
        "every machine that sent an event must be named once, got {answer}"
    );
}

/// Detects a contexts row that leaves out one of the columns the page is built
/// from: the name, the session a subagent runs in, the scopes it works under,
/// how much it holds and when it was last heard from are the whole of the table,
/// and a column the API does not carry is one the page cannot show.
#[test]
fn the_contexts_route_answers_a_row_with_the_name_parent_scopes_and_counts_the_page_shows() {
    let server = TestServer::start(example_store_files(), |_| {});
    let title = "Bracket rework on the vacuum former";
    let task = "Survey the rocketry crate and list its public functions";
    server.hook_named(
        "alpha",
        Some(10_000),
        Some(title),
        None,
        &common::hook_fixture("session_start"),
    );
    server.hook_tasked(
        "alpha",
        Some(10_000),
        Some(task),
        &common::hook_fixture("subagent_start"),
    );

    let (status, answer) = server.api("GET", "/api/contexts", None);

    assert_eq!(status, 200, "the contexts must be readable, got {answer}");
    let session = context_row(&answer, "alpha/session-1");
    assert_eq!(
        session["name"],
        json!(title),
        "a row must carry the name the page lists the context under, got {session}"
    );
    let scopes: Vec<&str> = session["active_scopes"]
        .as_array()
        .expect("the active scopes are a list")
        .iter()
        .map(|scope| scope.as_str().expect("a scope is text"))
        .collect();
    assert_eq!(
        scopes,
        vec!["global", "machine:alpha", "session:alpha/session-1"],
        "a row must carry the scopes the context works in, got {session}"
    );
    assert!(
        session["delivered_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
            && session["last_seen"].is_string(),
        "a row must say how much the context holds and when it was last seen, got {session}"
    );
    assert_eq!(
        context_row(&answer, "alpha/session-1/agent-7f3a")["parent"],
        json!("alpha/session-1"),
        "a subagent's row must report the session it runs in, got {answer}"
    );
}

/// Detects a prompt route that does not report the size of the text it renders,
/// which is the one thing the page is read for, and a mode it has no notion of
/// answered as though it were one it has, which would show the reader the wrong
/// text under the right heading.
#[test]
fn the_prompt_route_answers_the_text_with_its_size_and_refuses_a_mode_it_has_no_notion_of() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook(
        "alpha",
        Some(10_000),
        &common::hook_fixture("session_start"),
    );

    let (status, all) = server.api("GET", "/api/contexts/alpha/session-1/prompt?mode=all", None);

    assert_eq!(status, 200, "the prompt must be readable, got {all}");
    let text = all["text"].as_str().expect("the prompt carries text");
    assert_eq!(
        (
            all["bytes"].as_u64(),
            all["mode"].as_str(),
            all["key"].as_str()
        ),
        (
            Some(text.len() as u64),
            Some("all"),
            Some("alpha/session-1")
        ),
        "the answer must carry the size of the text beside it and name the mode and the \
         context it was rendered for, got {all}"
    );
    assert!(
        all["tokens"].as_u64().is_some_and(|tokens| tokens >= 1),
        "a text that is not empty must cost a token or more, got {all}"
    );

    let (status, answer) = server.api(
        "GET",
        "/api/contexts/alpha/session-1/prompt?mode=everything",
        None,
    );
    assert_eq!(
        status, 400,
        "a mode this route has no notion of must be refused, got {answer}"
    );
}

/// Detects a review route that answers only one of the two things the page
/// shows: what is wrong with the store, and the critical memories that reach
/// every session on every machine.
#[test]
fn the_review_route_answers_the_stores_problems_and_its_global_only_critical_memories() {
    let server = TestServer::start(example_store_files(), |_| {});
    let (status, clean) = server.api("GET", "/api/review", None);
    assert_eq!(status, 200, "the review must be readable, got {clean}");
    assert_eq!(
        clean["errors"],
        json!([]),
        "the example store must start clean, got {clean}"
    );
    let global_only: Vec<&str> = clean["global_only_critical"]
        .as_array()
        .expect("the report lists the global-only critical memories")
        .iter()
        .map(|id| id.as_str().expect("an id is text"))
        .collect();
    assert_eq!(
        global_only,
        vec!["bench-power"],
        "a critical memory scoped only to global must be listed and one with a scope of its \
         own must not, got {clean}"
    );

    server.commit(
        "add a note that links to nothing",
        vec![(
            "memories/bench-checks.md".to_string(),
            Some(
                b"---\nname: bench-checks\ndescription: The checks to run before powering the bench\nmetadata:\n  kind: knowledge\n  scopes:\n  - global\n---\n# Bench checks\n\nSee [[meter-calibration]] for the meter.\n"
                    .to_vec(),
            ),
        )],
    );

    let (status, answer) = server.api("GET", "/api/review", None);
    assert_eq!(status, 200, "the review must be readable, got {answer}");
    assert!(
        answer["errors"]
            .as_array()
            .expect("the report lists errors")
            .iter()
            .any(|error| {
                error["path"] == json!("memories/bench-checks.md") && error["message"].is_string()
            }),
        "a problem must be reported with the file it is in and a message, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// How an operation's failure is answered
// ---------------------------------------------------------------------------

/// Detects a refusal answered as a server failure, or as a plain message: the
/// edit form shows one entry per problem beside the file it is in, so a refusal
/// without `errors`, or with an entry naming no path, leaves the author with
/// nothing to correct.
#[test]
fn a_write_the_store_refuses_is_422_with_an_errors_entry_naming_the_file() {
    let server = TestServer::start(example_store_files(), |_| {});
    let current = document(&server, "reading-list");
    let before = head(&server);
    let refused = |body: &Value| {
        let (status, answer) = server.api("PUT", "/api/memories/reading-list", Some(body));
        assert_eq!(status, 422, "the write must be refused, got {answer}");
        answer
    };

    // A memory outside a session silo may not link into one, which is the rule
    // the silo exists for.
    let linking = refused(&write_of(
        &current,
        "# Where the workshop references live\n\nSee [[sessions/alpha/session-1/notes]].\n",
        "link the session notes",
    ));
    assert!(
        linking["errors"]
            .as_array()
            .expect("the refusal lists problems")
            .iter()
            .any(|error| {
                error["path"] == json!("memories/reading-list.md")
                    && error["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("sessions/alpha/session-1/notes"))
            }),
        "the refusal must name the file and the target it may not link to, got {linking}"
    );

    // A commit message the history cannot show as one line: the store refuses
    // it and the answer has to carry the same shape.
    let blank = refused(&write_of(
        &current,
        "# Where the workshop references live\n\nsome text\n",
        "   ",
    ));
    assert!(
        blank["errors"]
            .as_array()
            .is_some_and(|errors| errors.len() == 1 && errors[0]["message"].is_string()),
        "a blank message must be refused with one problem and its message, got {blank}"
    );

    assert_eq!(
        head(&server),
        before,
        "a refused write must leave the store as it was"
    );
}

/// Detects a stale write answered as a refusal or a server failure: the editor
/// tells the two apart by the status, and the document it shows beside the
/// author's own text is the one the answer carries, so a 409 without `current`
/// leaves the author with no way to save at all.
#[test]
fn a_write_from_a_stale_version_is_409_carrying_the_document_the_store_has_now() {
    let server = TestServer::start(example_store_files(), |_| {});
    let stale = document(&server, "reading-list");
    let first_body = "# Where the workshop references live\n\nthe first writer's text\n";
    let (status, _) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(&stale, first_body, "write from the first editor")),
    );
    assert_eq!(status, 200, "the first write must land");

    let (status, answer) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &stale,
            "# Where the workshop references live\n\nthe second writer's text\n",
            "write from the second editor",
        )),
    );

    assert_eq!(
        status, 409,
        "a write from a stale version must be a conflict, got {answer}"
    );
    assert_eq!(
        answer["current"]["body"].as_str(),
        Some(first_body),
        "the conflict must carry the document as the store has it, got {answer}"
    );
    assert_ne!(
        answer["current"]["version"].as_str(),
        stale["version"].as_str(),
        "the conflict must carry the version that made the write stale, got {answer}"
    );
}

/// Detects a resource that is not there, and a request this API will not act on
/// at all, answered as a server failure: the frontend shows "not found" for the
/// one and the message for the other, and can do neither with a 500. Every one
/// of them carries `error`, which is the only body the frontend reads.
#[test]
fn a_missing_resource_is_404_and_a_request_the_api_refuses_outright_is_400_each_with_a_message() {
    let server = TestServer::start(example_store_files(), |_| {});
    let before = head(&server);

    let not_found = [
        (
            "a memory the store does not have",
            "/api/memories/no-such-memory",
        ),
        (
            "a scope the store does not have",
            "/api/scopes/no-such-scope",
        ),
        (
            "a commit the store does not have",
            &format!("/api/history/{}", "f".repeat(40)),
        ),
        (
            "an oid that is not a commit at all",
            "/api/history/not-a-commit",
        ),
        (
            "a context no session has been seen at",
            "/api/contexts/alpha/nobody/prompt",
        ),
    ];
    for (case, path) in not_found {
        let (status, answer) = server.api("GET", path, None);
        assert_eq!(status, 404, "{case} must be a 404, got {answer}");
        assert!(
            answer["error"].is_string(),
            "{case} must be answered with a message, got {answer}"
        );
    }

    // `global` has no file, so there is nothing to delete; a delete reported as
    // anything but a refusal would suggest a session's own scopes can be taken
    // away from it.
    let (status, answer) = server.api(
        "DELETE",
        "/api/scopes/global",
        Some(&json!({
            "base_version": before,
            "author": AUTHOR,
            "message": "remove the global scope",
        })),
    );
    assert_eq!(
        status, 400,
        "deleting an implicit scope must be a bad request, got {answer}"
    );
    assert!(
        answer["error"].is_string(),
        "a 400 must carry a message, got {answer}"
    );

    // A body this route cannot read at all, which is what a client sending the
    // wrong shape produces.
    let (status, text) = server.post_raw("/api/memories", "{\"id\":");
    assert_eq!(
        status, 400,
        "a body that is not JSON must be a 400, got {text}"
    );
    assert!(
        text.contains("\"error\""),
        "a 400 must carry a message, got {text}"
    );

    assert_eq!(
        head(&server),
        before,
        "none of these may have written anything"
    );
}

/// Detects a failure the caller cannot act on answered as a refusal it could:
/// the frontend would show an editing problem where the store itself is broken,
/// and the author would rewrite the document for nothing. A 500 carries `error`
/// like every other failure, because that is the only body the frontend reads.
#[test]
fn a_failure_the_caller_cannot_act_on_is_500_with_a_message() {
    let server = TestServer::start(example_store_files(), |_| {});
    let request = write_of(
        &document(&server, "reading-list"),
        &format!("# Where the workshop references live\n\n{NEW_LINE}\n"),
        "note where the binder lives",
    );
    // The store is taken away under the running server, which is the shape of
    // every failure of the repository itself: nothing about the request is
    // wrong, and no rewriting of it would help.
    std::fs::remove_dir_all(server.store().path()).expect("the store directory is removable");

    let (status, answer) = server.api("PUT", "/api/memories/reading-list", Some(&request));

    assert_eq!(
        status, 500,
        "a store that cannot be written must be a server failure, got {answer}"
    );
    assert!(
        answer["error"].is_string(),
        "a 500 must carry a message, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// Several writers at once
// ---------------------------------------------------------------------------

/// A memory file with a generated name, for the tests that need more memories
/// than the example store has.
fn generated_memory(index: usize) -> (String, Option<Vec<u8>>) {
    let name = format!("note-{index:02}");
    let text = format!(
        "---\nname: {name}\ndescription: Generated note {index}, written to test concurrent writes\n---\n# {name}\n\nfirst text\n"
    );
    (format!("memories/{name}.md"), Some(text.into_bytes()))
}

/// Every commit reachable from the store's head, with how many parents it has,
/// read with a second handle on the repository.
fn commits_from_head(server: &TestServer) -> Vec<(String, usize)> {
    let repository = git2::Repository::open(server.store().path()).expect("the store opens");
    let mut walk = repository.revwalk().expect("the history can be walked");
    walk.push_head().expect("the store has a head");
    walk.map(|oid| {
        let commit = repository
            .find_commit(oid.expect("an oid in the walk"))
            .expect("a commit in the walk");
        (commit.id().to_string(), commit.parent_count())
    })
    .collect()
}

/// Detects a write path that loses a commit under concurrency: sixteen requests
/// to sixteen different memories, arriving at once over the network, must all
/// land, one commit each, in one line of history with every file present at the
/// end. A lost update or a branch would mean a memory whose edit is nowhere.
#[test]
fn sixteen_concurrent_writes_to_different_memories_all_land_in_one_line_of_history() {
    let writers = 16;
    let mut files = example_store_files();
    files.extend((0..writers).map(generated_memory));
    let server = TestServer::start(files, |_| {});
    let commits_before = commits_from_head(&server);

    let versions: Vec<String> = (0..writers)
        .map(|index| {
            document(&server, &format!("note-{index:02}"))["version"]
                .as_str()
                .expect("a version is text")
                .to_string()
        })
        .collect();

    let url = server.url();
    let statuses: Vec<(usize, u16)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..writers)
            .map(|index| {
                let url = url.clone();
                let base_version = versions[index].clone();
                scope.spawn(move || {
                    let (status, _) = api_request(
                        &url,
                        "PUT",
                        &format!("/api/memories/note-{index:02}"),
                        Some(&json!({
                            "description": format!("Generated note {index}, rewritten by writer {index}"),
                            "kind": "knowledge",
                            "scopes": ["global"],
                            "source": "user",
                            "body": format!("# note-{index:02}\n\ntext from writer {index}\n"),
                            "base_version": base_version,
                            "author": AUTHOR,
                            "message": format!("rewrite note {index:02}"),
                        })),
                    );
                    (index, status)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a writer finished"))
            .collect()
    });

    for (index, status) in &statuses {
        assert_eq!(*status, 200, "the write to note-{index:02} must land");
    }
    for index in 0..writers {
        let written = document(&server, &format!("note-{index:02}"));
        assert!(
            written["body"]
                .as_str()
                .is_some_and(|body| body.contains(&format!("text from writer {index}"))),
            "note-{index:02} must hold the text its writer sent, got {written}"
        );
    }
    let commits_after = commits_from_head(&server);
    assert_eq!(
        commits_after.len(),
        commits_before.len() + writers,
        "each write must add exactly one commit"
    );
    for (oid, parents) in &commits_after {
        assert!(
            *parents <= 1,
            "the history must stay linear; commit {oid} has {parents} parents"
        );
    }
}

/// Detects optimistic concurrency that admits more than one writer: several
/// editors saving the same memory from the same version must produce one winner
/// and a conflict for the rest, and the store must hold the winner's text.
#[test]
fn concurrent_writes_to_one_memory_from_one_version_admit_exactly_one() {
    let writers = 8;
    let server = TestServer::start(example_store_files(), |_| {});
    let base_version = document(&server, "reading-list")["version"]
        .as_str()
        .expect("a version is text")
        .to_string();

    let url = server.url();
    let outcomes: Vec<(usize, u16)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..writers)
            .map(|index| {
                let url = url.clone();
                let base_version = base_version.clone();
                scope.spawn(move || {
                    let (status, _) = api_request(
                        &url,
                        "PUT",
                        "/api/memories/reading-list",
                        Some(&json!({
                            "description": "The workshop references are all on paper in the binder, nothing online",
                            "kind": "knowledge",
                            "scopes": ["global"],
                            "source": "assistant",
                            "body": format!("# Where the workshop references live\n\ntext from writer {index}\n"),
                            "base_version": base_version,
                            "author": AUTHOR,
                            "message": format!("rewrite the reading list as writer {index}"),
                        })),
                    );
                    (index, status)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a writer finished"))
            .collect()
    });

    let accepted: Vec<usize> = outcomes
        .iter()
        .filter(|(_, status)| *status == 200)
        .map(|(index, _)| *index)
        .collect();
    assert_eq!(
        accepted.len(),
        1,
        "exactly one writer may be accepted, got {outcomes:?}"
    );
    for (index, status) in &outcomes {
        if !accepted.contains(index) {
            assert_eq!(
                *status, 409,
                "writer {index} was not accepted, so it must be told its version is stale"
            );
        }
    }
    let stored = document(&server, "reading-list");
    assert!(
        stored["body"]
            .as_str()
            .is_some_and(|body| body.contains(&format!("text from writer {}", accepted[0]))),
        "the store must hold the accepted writer's text, got {stored}"
    );
}

// ---------------------------------------------------------------------------
// The statistics routes the page reads over a window: the summary, the series
// and one session's events.
// ---------------------------------------------------------------------------

/// A prompt naming a widget, which turns the `widgets` scope on in the example
/// store and delivers what that makes due.
fn widget_prompt() -> Value {
    json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "session-1",
        "cwd": "/home/dev/notes",
        "prompt": "check the widget numbering before we continue"
    })
}

/// A server with one session start and one widget prompt behind it, which is
/// the smallest log the statistics routes have something to answer for.
fn server_with_a_session() -> TestServer {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook(
        "alpha",
        Some(10_000),
        &common::hook_fixture("session_start"),
    );
    server.hook("alpha", Some(10_000), &widget_prompt());
    server
}

/// Two sessions on one machine that were delivered different scopes: the first
/// names a widget, so `widgets` and the `rocketry` it implies are on; the second
/// names only a rocket, so `rocketry` alone is. The second session has no notes
/// in the store, so nothing is ever delivered under its own session scope.
fn server_with_two_sessions() -> TestServer {
    let server = server_with_a_session();
    server.hook(
        "alpha",
        Some(10_000),
        &json!({
            "hook_event_name": "SessionStart",
            "session_id": "session-2",
            "cwd": "/home/dev/notes",
            "source": "startup"
        }),
    );
    server.hook(
        "alpha",
        Some(10_000),
        &json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "session-2",
            "cwd": "/home/dev/notes",
            "prompt": "check the rocket fairing before we continue"
        }),
    );
    server
}

/// The rows of the scopes report, by scope id, for the query `search`.
fn scope_rows(server: &TestServer, search: &str) -> BTreeMap<String, Value> {
    let (status, answer) = server.api("GET", &format!("/api/stats/scopes{search}"), None);
    assert_eq!(status, 200, "the scopes must be readable, got {answer}");
    answer
        .as_array()
        .unwrap_or_else(|| panic!("the report is a list of rows: {answer}"))
        .iter()
        .map(|row| {
            let id = row["scope_id"]
                .as_str()
                .unwrap_or_else(|| panic!("a row names its scope: {row}"));
            (id.to_string(), row.clone())
        })
        .collect()
}

/// Detects a per-scope report that ignores the session the query names: the
/// page's session filter reaches every table on it, so a scopes report that
/// answered the whole log would show one session's page the tokens of all of
/// them and no figure on the page would say so.
///
/// Two sessions, two different scopes. The widget session's page charges
/// `widgets`; the rocket session's page charges `rocketry` and leaves `widgets`
/// at nothing, though `widgets` still has a row because the index lists it.
#[test]
fn the_scopes_route_counts_only_the_rows_of_the_session_the_query_names() {
    let server = server_with_two_sessions();

    let whole = scope_rows(&server, "");
    let widget_session = scope_rows(&server, "?session=alpha/session-1");
    let rocket_session = scope_rows(&server, "?session=alpha/session-2");

    let tokens = |rows: &BTreeMap<String, Value>, scope: &str| -> u64 {
        rows.get(scope)
            .unwrap_or_else(|| panic!("{scope} must have a row, got {rows:?}"))["tokens"]
            .as_u64()
            .unwrap_or_else(|| panic!("a scope reports what it cost in tokens, got {rows:?}"))
    };
    let deliveries = |rows: &BTreeMap<String, Value>, scope: &str| -> u64 {
        rows.get(scope)
            .unwrap_or_else(|| panic!("{scope} must have a row, got {rows:?}"))["deliveries"]
            .as_u64()
            .unwrap_or_else(|| panic!("a scope reports its deliveries, got {rows:?}"))
    };

    assert!(
        tokens(&whole, "widgets") > 0 && tokens(&whole, "rocketry") > 0,
        "both scopes were delivered somewhere in the log, got {whole:?}"
    );
    assert_eq!(
        (
            tokens(&widget_session, "widgets"),
            deliveries(&widget_session, "widgets")
        ),
        (tokens(&whole, "widgets"), deliveries(&whole, "widgets")),
        "the widget session is the only one that was ever charged for widgets"
    );
    assert_eq!(
        (
            tokens(&rocket_session, "widgets"),
            deliveries(&rocket_session, "widgets")
        ),
        (0, 0),
        "the rocket session was never delivered widgets, got {rocket_session:?}"
    );
    assert!(
        tokens(&rocket_session, "rocketry") > 0,
        "the rocket session was delivered rocketry, got {rocket_session:?}"
    );
}

/// Detects a per-scope report that lists every session scope it knows of. There
/// is one such scope per session and most of them hold nothing, so a report
/// that keeps the empty ones fills the table with a row per session that says
/// only that the session existed.
///
/// The first session has notes in the store and is charged for them; the second
/// has none, so its scope was never delivered and is not a row. Both sessions
/// are live, so a report that kept a scope for having a live context in it would
/// keep the second one too.
#[test]
fn the_scopes_route_leaves_out_a_session_scope_nothing_was_delivered_under() {
    let server = server_with_two_sessions();

    let rows = scope_rows(&server, "");

    let delivered = rows
        .get("session:alpha/session-1")
        .unwrap_or_else(|| panic!("the session with notes must have a row, got {rows:?}"));
    assert!(
        delivered["deliveries"].as_u64().unwrap_or(0) > 0
            && delivered["tokens"].as_u64().unwrap_or(0) > 0,
        "the session with notes was charged for them, got {delivered}"
    );
    assert!(
        !rows.contains_key("session:alpha/session-2"),
        "a session scope nothing was delivered under is not a row, got {rows:?}"
    );
    assert!(
        rows.contains_key("machine:alpha") && rows.contains_key("global"),
        "only session scopes are dropped when empty, got {rows:?}"
    );
}

/// Detects a summary that answers in a shape the page cannot read: it draws one
/// row of numbers from the four named windows and a live-context count, and a
/// window missing or named differently leaves a card blank. The figures are
/// tokens, because the page converts nothing itself.
#[test]
fn the_summary_route_answers_the_four_named_windows_in_tokens_with_the_live_contexts() {
    let server = server_with_a_session();

    let (status, answer) = server.api("GET", "/api/stats/summary", None);

    assert_eq!(status, 200, "the summary must be readable, got {answer}");
    let windows = answer["windows"]
        .as_array()
        .unwrap_or_else(|| panic!("the summary must carry its windows: {answer}"));
    assert_eq!(
        windows
            .iter()
            .map(|row| row["name"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["5m", "1h", "1d", "7d"],
        "the four windows must be named as the page names them, got {answer}"
    );
    for row in windows {
        for field in ["tokens", "events", "held", "forgettings"] {
            assert!(
                row[field].as_u64().is_some(),
                "every window must report {field} as a number, got {row}"
            );
        }
        assert!(
            row["tokens"].as_u64().unwrap_or(0) > 0,
            "both events of the session delivered text, so every window carries tokens: {row}"
        );
    }
    assert_eq!(
        answer["live_contexts"],
        json!(1),
        "the sequence ran in one context, got {answer}"
    );
}

/// Detects a series route that answers without saying which bucket it used,
/// that takes a bucket it has no notion of, or that reads an unparsable time as
/// "no bound": a chart drawn on the wrong bucket or the wrong range is silently
/// wrong, which is worse than a refusal.
#[test]
fn the_series_route_names_its_bucket_and_refuses_a_bucket_or_a_time_it_cannot_read() {
    let server = server_with_a_session();

    let (status, answer) = server.api("GET", "/api/stats/series?bucket=hour", None);
    assert_eq!(status, 200, "the series must be readable, got {answer}");
    assert_eq!(
        answer["bucket"],
        json!("hour"),
        "the answer must name the bucket its points are in, got {answer}"
    );
    let points = answer["points"]
        .as_array()
        .unwrap_or_else(|| panic!("the series must carry its points: {answer}"));
    assert_eq!(
        points.len(),
        1,
        "the clock stood still, so both events fall in one hour: {answer}"
    );
    assert!(
        points[0]["tokens"].as_u64().unwrap_or(0) > 0
            && points[0]["deliveries"].as_u64().unwrap_or(0) > 0
            && points[0]["t"]
                .as_str()
                .is_some_and(|t| t.ends_with(":00:00Z")),
        "a point is a bucket start with the tokens and the deliveries in it, got {answer}"
    );

    let (status, chosen) = server.api("GET", "/api/stats/series", None);
    assert_eq!(
        status, 200,
        "a series with no bucket must be readable, got {chosen}"
    );
    assert!(
        ["minute", "hour", "day"].contains(&chosen["bucket"].as_str().unwrap_or_default()),
        "the route must choose a bucket and name it, got {chosen}"
    );

    let (status, refused) = server.api("GET", "/api/stats/series?bucket=fortnight", None);
    assert_eq!(
        status, 400,
        "a bucket this route has no notion of must be refused, got {refused}"
    );
    let (status, refused) = server.api("GET", "/api/stats/series?from=yesterday", None);
    assert_eq!(
        status, 400,
        "a bound that is not an RFC 3339 time must be refused, got {refused}"
    );
}

/// Detects a window that narrows nothing, which would draw the whole log
/// whatever range the page asked for, and a filter that silently matches
/// everything.
#[test]
fn the_series_route_honours_the_window_and_the_two_filters_it_is_given() {
    let server = server_with_a_session();

    let (status, empty) = server.api(
        "GET",
        "/api/stats/series?from=2020-01-01T00:00:00Z&to=2020-01-02T00:00:00Z&bucket=day",
        None,
    );
    assert_eq!(
        status, 200,
        "a window with nothing in it must be readable, got {empty}"
    );
    assert_eq!(
        empty["points"],
        json!([]),
        "a window before every event must carry no points, got {empty}"
    );

    let (status, scoped) = server.api("GET", "/api/stats/series?scope=widgets&bucket=hour", None);
    assert_eq!(
        status, 200,
        "a scoped series must be readable, got {scoped}"
    );
    let (_, whole) = server.api("GET", "/api/stats/series?bucket=hour", None);
    let tokens = |answer: &Value| answer["points"][0]["tokens"].as_u64().unwrap_or(0);
    assert!(
        tokens(&scoped) > 0 && tokens(&scoped) < tokens(&whole),
        "one scope's text must be part of the whole and not all of it, got {scoped} against \
         {whole}"
    );

    let (status, elsewhere) = server.api(
        "GET",
        "/api/stats/series?session=alpha/nobody&bucket=hour",
        None,
    );
    assert_eq!(
        status, 200,
        "a session with no events must be readable, got {elsewhere}"
    );
    assert_eq!(
        elsewhere["points"],
        json!([]),
        "a session nothing was delivered into has no points, got {elsewhere}"
    );
}

/// Detects a per-session series answered for a context the log has never seen,
/// which would show the page an empty chart for a mistyped key instead of
/// saying there is no such context, and one that drops either of the two
/// measurements it exists to put side by side.
#[test]
fn the_session_series_route_answers_both_measurements_and_is_a_404_for_a_context_it_has_none_of() {
    let server = server_with_a_session();

    let (status, answer) = server.api("GET", "/api/stats/session/alpha/session-1/series", None);

    assert_eq!(status, 200, "the series must be readable, got {answer}");
    let events = answer
        .as_array()
        .unwrap_or_else(|| panic!("the series is a list of events: {answer}"));
    assert_eq!(
        events
            .iter()
            .map(|row| row["event"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["SessionStart", "UserPromptSubmit"],
        "every event of the context must be there, oldest first, got {answer}"
    );
    for row in events {
        assert_eq!(
            row["context_tokens"],
            json!(10_000),
            "the size Claude Code reported must be carried through as it arrived, got {row}"
        );
        assert!(
            row["tokens"].as_u64().is_some(),
            "the answer's own cost must be a number of tokens, got {row}"
        );
    }

    let (status, missing) = server.api("GET", "/api/stats/session/alpha/nobody/series", None);
    assert_eq!(
        status, 404,
        "a context the log has no event of must be a resource that is not there, got {missing}"
    );
    let (status, unknown) = server.api("GET", "/api/stats/session/alpha/session-1", None);
    assert_eq!(
        status, 404,
        "a path naming no sub-resource must be a 404, got {unknown}"
    );
}

/// Detects a scopes report without the figures the page's table is made of, and
/// a memory count taken from the log rather than from the catalog: a memory
/// moved out of a scope stops being one of its memories at once, however often
/// it was delivered under it.
#[test]
fn the_scopes_route_reports_what_a_scope_cost_beside_the_memories_it_holds_now() {
    let server = server_with_a_session();

    let (status, answer) = server.api("GET", "/api/stats/scopes", None);

    assert_eq!(status, 200, "the scopes must be readable, got {answer}");
    let rows = answer
        .as_array()
        .unwrap_or_else(|| panic!("the report is a list of rows: {answer}"));
    let widgets = rows
        .iter()
        .find(|row| row["scope_id"] == json!("widgets"))
        .unwrap_or_else(|| panic!("the widgets scope must have a row: {answer}"));
    assert_eq!(
        (widgets["deliveries"].as_u64(), widgets["memories"].as_u64()),
        (Some(1), Some(1)),
        "the prompt printed one section for the scope and the store gives it one memory: {widgets}"
    );
    let tokens = widgets["tokens"]
        .as_u64()
        .unwrap_or_else(|| panic!("a scope reports what it cost in tokens: {widgets}"));
    assert!(
        tokens > 0,
        "the scope delivered its memory in full: {widgets}"
    );
    assert_eq!(
        widgets["tokens_per_delivery"].as_f64(),
        Some(tokens as f64),
        "one delivery cost the whole of it: {widgets}"
    );
}

/// Detects a per-scope report whose rows are whatever id the log and the live
/// contexts name. A scope deleted while a context still works in it keeps a row
/// of zeros for as long as that context lives, and a scope deleted after its
/// memory was delivered loses the cost it was charged.
///
/// The expectation is the rule the page is drawn against: a row is a scope the
/// index lists or a scope with a delivery, an activation or a forgetting in the
/// window, and a context's active set only raises the live-context count of the
/// rows that are there.
#[test]
fn the_scopes_route_drops_an_id_only_a_context_names_and_keeps_a_deleted_scope_that_was_delivered()
{
    let server = server_with_a_session();
    let (status, answer) = server.api(
        "POST",
        "/api/scopes",
        Some(&json!({
            "id": "sandbox",
            "implies": [],
            "triggers": [],
            "author": AUTHOR,
            "message": "add the sandbox scope",
        })),
    );
    assert_eq!(status, 200, "the scope must be created, got {answer}");
    let (status, answer) = server.api(
        "POST",
        "/api/memories",
        Some(&json!({
            "id": "sandbox-rule",
            "description": "Nothing made in the sandbox outlives the session that made it",
            "kind": "critical",
            "scopes": ["sandbox"],
            "source": "user",
            "body": "# The sandbox is temporary\n\nNothing in it outlives the session.\n",
            "author": AUTHOR,
            "message": "record the sandbox rule",
        })),
    );
    assert_eq!(status, 200, "the memory must be created, got {answer}");

    // The session works in both scopes; only `sandbox` has anything to deliver,
    // so `workshop` is charged nothing while it is on.
    let session = server.mcp();
    let turned_on = tool_json(&session.call(
        "session_scope_on",
        json!({ "session_key": "alpha/session-1", "scopes": ["sandbox", "workshop"] }),
    ));
    assert!(
        turned_on["active"]
            .as_array()
            .is_some_and(|scopes| scopes.contains(&json!("sandbox"))),
        "the session must be working in the scope, got {turned_on}"
    );
    server.hook("alpha", Some(12_000), &widget_prompt());

    // Both scopes are then deleted, with the delivered memory, so nothing but
    // the context's active set and the log still names them.
    let (status, answer) = server.api(
        "DELETE",
        "/api/memories/sandbox-rule",
        Some(&json!({
            "base_version": document(&server, "sandbox-rule")["version"],
            "author": AUTHOR,
            "message": "the sandbox rule is written down elsewhere",
        })),
    );
    assert_eq!(status, 200, "the memory must be deleted, got {answer}");
    for id in ["sandbox", "workshop"] {
        let (status, answer) = server.api(
            "DELETE",
            &format!("/api/scopes/{id}"),
            Some(&json!({
                "base_version": scope(&server, id)["version"],
                "author": AUTHOR,
                "message": format!("the {id} scope is no longer in use"),
            })),
        );
        assert_eq!(status, 200, "the scope {id} must be deleted, got {answer}");
    }
    let (status, contexts) = server.api("GET", "/api/contexts", None);
    assert_eq!(status, 200, "the contexts must be readable, got {contexts}");
    let active = contexts
        .as_array()
        .unwrap_or_else(|| panic!("the contexts are a list: {contexts}"))
        .iter()
        .find(|row| row["key"] == json!("alpha/session-1"))
        .map(|row| row["active_scopes"].clone())
        .unwrap_or_else(|| panic!("the session must still be live: {contexts}"));
    assert!(
        active.as_array().is_some_and(
            |scopes| scopes.contains(&json!("sandbox")) && scopes.contains(&json!("workshop"))
        ),
        "the context must still name both deleted scopes, or this proves nothing: {active}"
    );

    let (status, answer) = server.api("GET", "/api/stats/scopes", None);

    assert_eq!(status, 200, "the scopes must be readable, got {answer}");
    let rows = answer
        .as_array()
        .unwrap_or_else(|| panic!("the report is a list of rows: {answer}"));
    assert!(
        !rows.iter().any(|row| row["scope_id"] == json!("workshop")),
        "a scope that was deleted and did nothing in the window must be no row, got {answer}"
    );
    let sandbox = rows
        .iter()
        .find(|row| row["scope_id"] == json!("sandbox"))
        .unwrap_or_else(|| panic!("a deleted scope with deliveries must be a row: {answer}"));
    assert!(
        sandbox["tokens"].as_u64().unwrap_or(0) > 0 && sandbox["deliveries"].as_u64() == Some(1),
        "the deleted scope must keep the one delivery it was charged: {sandbox}"
    );
}

/// Detects a memories report that loses the scope a memory was printed under,
/// which is what says where a memory's cost is charged, or that reports no cost
/// at all.
#[test]
fn the_memories_route_reports_what_each_memory_cost_and_the_scope_it_was_printed_under() {
    let server = server_with_a_session();

    let (status, answer) = server.api("GET", "/api/stats/memories", None);

    assert_eq!(status, 200, "the memories must be readable, got {answer}");
    let row = answer
        .as_array()
        .unwrap_or_else(|| panic!("the report is a list of rows: {answer}"))
        .iter()
        .find(|row| row["memory"] == json!("widget-naming"))
        .unwrap_or_else(|| panic!("the delivered memory must have a row: {answer}"))
        .clone();
    assert_eq!(
        row["most_under"],
        json!("widgets"),
        "the memory's only scope is the section it was printed in: {row}"
    );
    assert!(
        row["tokens"].as_u64().unwrap_or(0) > 0,
        "a memory delivered in full cost tokens: {row}"
    );
}
