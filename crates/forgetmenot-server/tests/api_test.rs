//! Tests of the JSON API: the write contract, the read side the frontend is
//! built against, and what happens when several writers arrive at once.
//!
//! Everything is asserted through the HTTP answers, and where an answer claims a
//! commit was made, through the store's git history read with a second handle:
//! a write path that answered 200 without committing, or committed twice, would
//! fail here.
//!
//! The expectations come from the plan's data model and write contract, and from
//! the example store committed in this repository.

mod common;

use std::collections::BTreeSet;

use serde_json::{Value, json};

use common::{TestServer, additional_context, api_request, example_store_files, hook_fixture};

/// The author name a write carries; the frontend sends this one.
const AUTHOR: &str = "wiki";

/// The context size reported with the hook events used here, which are not about
/// staleness.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// The longest commit title the history can show on one line. Source: the plan's
/// write contract, which fixes the limit at the width `git log --oneline` shows.
const MAX_MESSAGE_TITLE: usize = 72;

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

/// A write of `body` and `description` to the version `document` was read at.
fn write_of(document: &Value, description: &str, body: &str, message: &str) -> Value {
    json!({
        "description": description,
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

/// The commits that changed one file, read through the test's own handle.
fn commits_touching(server: &TestServer, path: &str) -> Vec<(String, String)> {
    server
        .store()
        .repository()
        .log_for_path(path)
        .expect("the file's history is readable")
        .into_iter()
        .map(|summary| (summary.author, summary.title))
        .collect()
}

/// The ids the index reports for a query.
fn index_ids(server: &TestServer, query: &str) -> Vec<String> {
    let (status, answer) = server.api("GET", &format!("/api/memories{query}"), None);
    assert_eq!(status, 200, "the index must be readable, got {answer}");
    answer
        .as_array()
        .expect("the index is a list")
        .iter()
        .map(|entry| entry["id"].as_str().expect("an id is text").to_string())
        .collect()
}

/// A memory file with a generated name, for the tests that need more memories
/// than the example store has.
fn generated_memory(index: usize) -> (String, Option<Vec<u8>>) {
    let name = format!("note-{index:02}");
    let text = format!(
        "---\nname: {name}\ndescription: Generated note {index}, written to test concurrent writes\n---\n# {name}\n\nfirst text\n"
    );
    (format!("memories/{name}.md"), Some(text.into_bytes()))
}

/// The text a hook answer injects, or the empty string when it injects nothing.
fn context_of(answer: &Value) -> &str {
    additional_context(answer).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// Detects a write that answers 200 without committing, commits under the
/// server's own name instead of the caller's, loses the caller's message, or
/// commits content other than what was sent: the history would then not say who
/// changed what, and the editor would show text the store does not hold.
#[test]
fn a_write_from_the_current_version_makes_one_commit_with_the_given_author_and_message() {
    let server = TestServer::start(example_store_files(), |_| {});
    let message = "note where the binder lives";
    let description = "The workshop references are in the binder on the shelf by the door";
    let body = format!("# Where the workshop references live\n\n{NEW_LINE}\n");
    let before = commits_touching(&server, "memories/reading-list.md").len();

    let (status, answer) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &document(&server, "reading-list"),
            description,
            &body,
            message,
        )),
    );

    assert_eq!(status, 200, "the write must be accepted, got {answer}");
    let commits = commits_touching(&server, "memories/reading-list.md");
    assert_eq!(
        commits.len(),
        before + 1,
        "the write must make exactly one commit, got {commits:?}"
    );
    assert_eq!(
        commits[0],
        (AUTHOR.to_string(), message.to_string()),
        "the commit must carry the author and the message the write gave"
    );
    let written = document(&server, "reading-list");
    assert_eq!(
        written["body"].as_str(),
        Some(body.as_str()),
        "the document must read back with the body that was written"
    );
    assert_eq!(
        written["description"].as_str(),
        Some(description),
        "the document must read back with the description that was written"
    );
    assert_eq!(
        written["version"].as_str(),
        answer["version"].as_str(),
        "the write must report the version the document now has"
    );
}

/// Detects a write path that accepts a commit message the history cannot show as
/// one line: an empty title, a title past the limit, or one spanning lines. Each
/// case must be refused and must leave the store untouched.
#[test]
fn a_commit_message_the_history_cannot_show_is_refused_and_makes_no_commit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let document = document(&server, "reading-list");
    let before = head(&server);

    for (case, message) in [
        ("no message", String::new()),
        ("only spaces", "   ".to_string()),
        ("past the title limit", "a".repeat(MAX_MESSAGE_TITLE + 1)),
        ("two lines", "first line\nsecond line".to_string()),
    ] {
        let (status, answer) = server.api(
            "PUT",
            "/api/memories/reading-list",
            Some(&write_of(
                &document,
                "The workshop references are all on paper in the binder, nothing online",
                "# Where the workshop references live\n\nsome text\n",
                &message,
            )),
        );
        assert_eq!(
            status, 422,
            "a write with {case} must be refused, got {answer}"
        );
        assert!(
            answer["errors"]
                .as_array()
                .is_some_and(|errors| !errors.is_empty()),
            "a write with {case} must say what was wrong, got {answer}"
        );
    }

    assert_eq!(
        head(&server),
        before,
        "a refused write must leave the store as it was"
    );
}

/// Detects a write that ignores the version it was made from: two editors saving
/// from the same version would overwrite each other, and the second would never
/// see the first one's text.
#[test]
fn a_write_from_a_stale_version_is_refused_with_the_current_document_and_makes_no_commit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let stale = document(&server, "reading-list");
    let first_body = "# Where the workshop references live\n\nthe first writer's text\n";
    let (status, _) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &stale,
            "d",
            first_body,
            "write from the first editor",
        )),
    );
    assert_eq!(status, 200, "the first write must land");
    let after_first = head(&server);

    let (status, answer) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &stale,
            "d",
            "# Where the workshop references live\n\nthe second writer's text\n",
            "write from the second editor",
        )),
    );

    assert_eq!(
        status, 409,
        "a write from a stale version must be refused, got {answer}"
    );
    assert_eq!(
        answer["current"]["body"].as_str(),
        Some(first_body),
        "the refusal must carry the document as the store has it, got {answer}"
    );
    assert_ne!(
        answer["current"]["version"].as_str(),
        stale["version"].as_str(),
        "the refusal must carry the version that made it stale, got {answer}"
    );
    assert_eq!(
        head(&server),
        after_first,
        "a refused write must leave the store as it was"
    );
}

/// Detects a write path that skips the store's validation: a memory outside a
/// session silo that links into one would make a session's notes readable from
/// anywhere, which is the rule the silo exists for.
#[test]
fn a_write_whose_body_links_into_a_session_silo_is_refused_and_makes_no_commit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let before = head(&server);

    let (status, answer) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &document(&server, "reading-list"),
            "The workshop references are all on paper in the binder, nothing online",
            "# Where the workshop references live\n\nSee [[sessions/alpha/session-1/notes]].\n",
            "link the session notes",
        )),
    );

    assert_eq!(
        status, 422,
        "a link into a session silo must be refused, got {answer}"
    );
    let errors = answer["errors"]
        .as_array()
        .expect("the refusal lists problems");
    assert!(
        errors.iter().any(|error| {
            error["path"] == json!("memories/reading-list.md")
                && error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("sessions/alpha/session-1/notes"))
        }),
        "the refusal must name the file and the target it may not link to, got {answer}"
    );
    assert_eq!(
        head(&server),
        before,
        "a refused write must leave the store as it was"
    );
}

/// Detects a create that overwrites the memory already at that id, which would
/// silently replace someone else's memory with a new one.
#[test]
fn creating_a_memory_at_an_id_that_exists_is_refused_with_the_current_document() {
    let server = TestServer::start(example_store_files(), |_| {});
    let before = head(&server);

    let (status, answer) = server.api(
        "POST",
        "/api/memories",
        Some(&json!({
            "id": "reading-list",
            "description": "A second memory claiming an id that is taken",
            "kind": "knowledge",
            "scopes": ["global"],
            "source": "user",
            "body": "# Another reading list\n\ntext\n",
            "author": AUTHOR,
            "message": "create a second reading list",
        })),
    );

    assert_eq!(
        status, 409,
        "creating over an existing id must be refused, got {answer}"
    );
    assert_eq!(
        answer["current"]["id"].as_str(),
        Some("reading-list"),
        "the refusal must carry the memory that is already there, got {answer}"
    );
    assert_eq!(
        head(&server),
        before,
        "a refused create must leave the store as it was"
    );
}

/// Detects a create that does not record when the memory was made, and an index
/// filter that ignores the scope it was given: a memory would then appear in
/// every scope's list.
#[test]
fn a_created_memory_is_stamped_and_listed_only_under_its_own_scope() {
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

    let created = document(&server, "bracket-tolerances");
    assert!(
        created["created"].is_string(),
        "a created memory must record when it was made, got {created}"
    );
    assert!(
        index_ids(&server, "").contains(&"bracket-tolerances".to_string()),
        "a created memory must appear in the index"
    );
    assert!(
        index_ids(&server, "?scope=widgets").contains(&"bracket-tolerances".to_string()),
        "a created memory must appear under the scope it names"
    );
    assert!(
        !index_ids(&server, "?scope=rocketry").contains(&"bracket-tolerances".to_string()),
        "a memory must not appear under a scope it does not name"
    );
}

/// Detects an archive that does not take the memory out of the index, or one
/// that hides it so well that it can no longer be found at all: an archived
/// memory stays in the store and must be listable on request.
#[test]
fn an_archived_memory_leaves_the_index_and_is_listed_only_when_asked_for() {
    let server = TestServer::start(example_store_files(), |_| {});
    let document = document(&server, "rocket-stages");

    let (status, answer) = server.api(
        "POST",
        "/api/memories/rocket-stages/archive",
        Some(&json!({
            "base_version": document["version"],
            "author": AUTHOR,
            "message": "archive the stage numbering note",
        })),
    );
    assert_eq!(status, 200, "the archive must be accepted, got {answer}");

    assert!(
        !index_ids(&server, "").contains(&"rocket-stages".to_string()),
        "an archived memory must not be in the index"
    );
    let (status, listed) = server.api("GET", "/api/memories?archived=true", None);
    assert_eq!(
        status, 200,
        "the index with archived memories must be readable"
    );
    let archived = listed
        .as_array()
        .expect("the index is a list")
        .iter()
        .find(|entry| entry["id"] == json!("rocket-stages"))
        .unwrap_or_else(|| panic!("an archived memory must be listed on request, got {listed}"));
    assert_eq!(
        archived["archived"],
        json!(true),
        "the archived memory must be reported as archived, got {archived}"
    );
}

/// Detects a history that does not show the write just made, and a per-commit
/// diff that does not say what changed: the two things the history pages exist
/// for.
#[test]
fn the_history_of_a_write_lists_its_commit_and_its_diff_marks_the_added_line() {
    let server = TestServer::start(example_store_files(), |_| {});
    let message = "note where the binder lives";
    let (status, _) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &document(&server, "reading-list"),
            "The workshop references are all on paper in the binder, nothing online",
            &format!("# Where the workshop references live\n\n{NEW_LINE}\n"),
            message,
        )),
    );
    assert_eq!(status, 200, "the write must land");

    let (status, commits) = server.api("GET", "/api/memories/reading-list/history", None);
    assert_eq!(status, 200, "the history must be readable, got {commits}");
    let newest = &commits.as_array().expect("the history is a list")[0];
    assert_eq!(
        newest["title"].as_str(),
        Some(message),
        "the newest commit must be the write just made, got {commits}"
    );
    assert_eq!(
        newest["author"].as_str(),
        Some(AUTHOR),
        "the commit must name the author the write gave, got {commits}"
    );

    let oid = newest["oid"].as_str().expect("a commit has an oid");
    let (status, entry) = server.api(
        "GET",
        &format!("/api/memories/reading-list/history/{oid}"),
        None,
    );
    assert_eq!(status, 200, "the commit must be readable, got {entry}");
    let diff = entry["diff"].as_str().expect("the entry carries a diff");
    assert!(
        diff.lines().any(|line| line == format!("+{NEW_LINE}")),
        "the diff must show the added line as added, got {diff:?}"
    );
    assert!(
        entry["content"]
            .as_str()
            .is_some_and(|content| content.contains(NEW_LINE)),
        "the entry must carry the file as it was at that commit, got {entry}"
    );
}

/// Detects a write to a memory that is not there being answered as anything
/// other than "no such memory": a mistyped id would otherwise create a memory,
/// or fail as a server error the frontend cannot explain.
#[test]
fn reading_or_writing_a_memory_that_does_not_exist_is_reported_as_missing() {
    let server = TestServer::start(example_store_files(), |_| {});
    let before = head(&server);

    let (status, answer) = server.api("GET", "/api/memories/no-such-memory", None);
    assert_eq!(
        status, 404,
        "reading a missing memory must be a 404, got {answer}"
    );
    assert!(
        answer["error"].is_string(),
        "a 404 must carry a message, got {answer}"
    );

    let (status, answer) = server.api(
        "PUT",
        "/api/memories/no-such-memory",
        Some(&json!({
            "description": "d",
            "kind": "knowledge",
            "scopes": ["global"],
            "source": "user",
            "body": "text\n",
            "base_version": before,
            "author": AUTHOR,
            "message": "write to a memory that does not exist",
        })),
    );
    assert_eq!(
        status, 404,
        "writing to a missing memory must be a 404, got {answer}"
    );
    assert_eq!(
        head(&server),
        before,
        "a refused write must leave the store as it was"
    );
}

/// Detects a scope write that skips validation: a trigger pattern that does not
/// compile would leave the store with a scope no context can ever turn on, and
/// the failure would only show at the next catalog load.
#[test]
fn a_scope_write_with_a_pattern_that_does_not_compile_is_refused_and_makes_no_commit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let (status, scope) = server.api("GET", "/api/scopes/widgets", None);
    assert_eq!(status, 200, "the scope must be readable, got {scope}");
    let before = head(&server);

    let (status, answer) = server.api(
        "PUT",
        "/api/scopes/widgets",
        Some(&json!({
            "type": scope["type"],
            "implies": scope["implies"],
            "triggers": [{ "on": "tool_input", "pattern": "(unclosed" }],
            "base_version": scope["version"],
            "author": AUTHOR,
            "message": "add a pattern that does not compile",
        })),
    );

    assert_eq!(
        status, 422,
        "a pattern that does not compile must be refused, got {answer}"
    );
    assert!(
        answer["errors"].as_array().is_some_and(|errors| errors
            .iter()
            .any(|error| error["path"] == json!("scopes/widgets.yaml"))),
        "the refusal must name the scope file, got {answer}"
    );
    assert_eq!(
        head(&server),
        before,
        "a refused write must leave the store as it was"
    );
}

// ---------------------------------------------------------------------------
// Reads that answer a question about the live server
// ---------------------------------------------------------------------------

/// Detects a trigger test that reports nothing for a text a scope's pattern
/// matches, or that does not say which pattern matched: the page exists to show
/// a person why a scope turns on.
#[test]
fn the_trigger_test_names_the_scope_and_pattern_a_matching_text_fires() {
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
    let fired = answer["fired"]
        .as_array()
        .expect("the answer lists what fired");
    let widgets = fired
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

/// Detects a trigger test that ignores the machine qualifier: a path means
/// different things on different machines, and a directory scope would otherwise
/// look as if it fired everywhere.
#[test]
fn the_trigger_test_applies_the_machine_qualifier_of_a_directory_trigger() {
    let server = TestServer::start(example_store_files(), |_| {});
    let test_on = |machine: &str| {
        let (status, answer) = server.api(
            "POST",
            "/api/triggers/test",
            Some(&json!({
                "field": "working_directory",
                "text": "/home/dev/workshop/bench",
                "machine": machine,
            })),
        );
        assert_eq!(status, 200, "a trigger test must be answered, got {answer}");
        answer
    };

    let own_machine = test_on("alpha");
    assert!(
        own_machine["fired"]
            .as_array()
            .is_some_and(|fired| fired.iter().any(|hit| hit["scope_id"] == json!("workshop"))),
        "the trigger must fire for the machine it names, got {own_machine}"
    );

    let other_machine = test_on("beta");
    assert_eq!(
        other_machine["fired"],
        json!([]),
        "the trigger must not fire for another machine, got {other_machine}"
    );
}

/// Detects a contexts page that cannot see the live state: after a session has
/// been delivered to, it must be listed with the scopes it works in, or nobody
/// can tell what a session is currently working under.
#[test]
fn the_contexts_page_lists_a_session_after_its_first_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, answer) = server.api("GET", "/api/contexts", None);

    assert_eq!(status, 200, "the contexts must be readable, got {answer}");
    let rows = answer.as_array().expect("the contexts are a list");
    let row = rows
        .iter()
        .find(|row| row["key"] == json!("alpha/session-1"))
        .unwrap_or_else(|| panic!("the session that had an event must be listed, got {answer}"));
    let scopes: BTreeSet<&str> = row["active_scopes"]
        .as_array()
        .expect("the active scopes are a list")
        .iter()
        .map(|scope| scope.as_str().expect("a scope is text"))
        .collect();
    for expected in ["global", "machine:alpha", "session:alpha/session-1"] {
        assert!(
            scopes.contains(expected),
            "the session must be listed as working in {expected}, got {row}"
        );
    }
    assert!(
        row["delivered_count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "a session that was delivered to must report what it has, got {row}"
    );
    assert!(
        row["last_seen"].is_string(),
        "a context must report when it was last seen, got {row}"
    );
}

/// Detects a review page computed once at start instead of from the store as it
/// is: a link broken by a commit made outside the server would never be
/// reported, and the page would say the store is clean while it is not.
#[test]
fn the_review_page_reports_a_link_left_unresolved_by_a_later_commit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let (status, clean) = server.api("GET", "/api/review", None);
    assert_eq!(status, 200, "the review must be readable, got {clean}");
    assert_eq!(
        clean["errors"],
        json!([]),
        "the example store must start clean, got {clean}"
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
    let errors = answer["errors"]
        .as_array()
        .expect("the report lists errors");
    assert!(
        errors.iter().any(|error| {
            error["path"] == json!("memories/bench-checks.md")
                && error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("meter-calibration"))
        }),
        "the unresolved link must be reported with its file and target, got {answer}"
    );
}

/// Detects a review page that does not single out the critical memories every
/// session gets: a critical memory whose only scope is `global` interrupts every
/// session on every machine, which is what the page exists to make visible.
#[test]
fn the_review_page_lists_the_critical_memories_that_are_global_only() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api("GET", "/api/review", None);

    assert_eq!(status, 200, "the review must be readable, got {answer}");
    let global_only: Vec<&str> = answer["global_only_critical"]
        .as_array()
        .expect("the report lists the global-only critical memories")
        .iter()
        .map(|id| id.as_str().expect("an id is text"))
        .collect();
    assert!(
        global_only.contains(&"bench-power"),
        "a critical memory scoped only to global must be listed, got {answer}"
    );
    assert!(
        !global_only.contains(&"widget-naming"),
        "a critical memory with a scope of its own must not be listed, got {answer}"
    );
}

/// Detects a catalog that is not rebuilt after a write through the API: a session
/// would keep the version it was given and never see the edit, which is the
/// whole point of delivering changed memories.
#[test]
fn a_hook_event_after_a_write_delivers_the_changed_memory() {
    let server = TestServer::start(example_store_files(), |_| {});
    let first = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        !context_of(&first.1).contains(NEW_LINE),
        "the new line cannot be in the store yet, got {first:?}"
    );

    let (status, answer) = server.api(
        "PUT",
        "/api/memories/bench-power",
        Some(&write_of(
            &document(&server, "bench-power"),
            "Cut bench power at the wall before rewiring and confirm with the meter",
            &format!("# Cut bench power before rewiring\n\n{NEW_LINE}\n"),
            "note where the binder lives",
        )),
    );
    assert_eq!(status, 200, "the write must land, got {answer}");

    let (status, next) = server.hook("alpha", SOME_TOKENS, &hook_fixture("stop"));

    assert_eq!(status, 200, "the next event must be answered");
    assert!(
        context_of(&next).contains(NEW_LINE),
        "the edited memory must be delivered again in full, got {next}"
    );
}

// ---------------------------------------------------------------------------
// Several writers at once
// ---------------------------------------------------------------------------

/// Detects a write path that loses a commit under concurrency: sixteen writes to
/// sixteen different memories must all land, one commit each, in one line of
/// history with every file present at the end. A lost update or a branch would
/// mean a memory whose edit is nowhere.
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
        let path = format!("memories/note-{index:02}.md");
        assert_eq!(
            commits_touching(&server, &path).len(),
            2,
            "note-{index:02} must have its fixture commit and exactly one write"
        );
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
/// and refusals for the rest, and the store must hold the winner's text.
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
    let winner = accepted[0];
    let stored = document(&server, "reading-list");
    assert!(
        stored["body"]
            .as_str()
            .is_some_and(|body| body.contains(&format!("text from writer {winner}"))),
        "the store must hold the accepted writer's text, got {stored}"
    );
    assert_eq!(
        commits_touching(&server, "memories/reading-list.md").len(),
        2,
        "only the accepted write may have been committed"
    );
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
