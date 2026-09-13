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

use common::{
    TestServer, additional_context, api_request, example_store_files, example_store_with_settings,
    example_store_without_settings, hook_fixture,
};

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

/// A line of `bench-power`'s body and of no other memory of the example store,
/// so that "delivered in full" can be told apart from "named in an index line".
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";

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

/// The file at one commit, read through the test's own handle: `None` when that
/// commit's tree has no such path.
fn file_at(server: &TestServer, commit_oid: &str, path: &str) -> Option<String> {
    let oid = commit_oid.parse().expect("the answer names a commit");
    server
        .store()
        .repository()
        .blob_at(oid, path)
        .expect("the commit is readable")
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Whether `text` carries a line saying `id` was withdrawn for `reason`.
fn has_retracted_line(text: &str, id: &str, reason: &str) -> bool {
    text.lines()
        .any(|line| line.contains(id) && line.contains(reason))
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

/// The ids the scope index reports.
fn scope_ids(server: &TestServer) -> Vec<String> {
    let (status, answer) = server.api("GET", "/api/scopes", None);
    assert_eq!(status, 200, "the scopes must be readable, got {answer}");
    answer
        .as_array()
        .expect("the scope index is a list")
        .iter()
        .map(|entry| entry["id"].as_str().expect("an id is text").to_string())
        .collect()
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

/// A memory whose file carries keys forgetmenot does not interpret and a source
/// that is neither of the two conventional values: the shape of a file written
/// by Claude Code and kept by hand. The content is invented.
const KEPT_METADATA_MEMORY: &str = concat!(
    "---\n",
    "name: collet-rack\n",
    "description: Collets go back in the rack by size after every job\n",
    "metadata:\n",
    "  kind: knowledge\n",
    "  scopes:\n",
    "  - global\n",
    "  source: derived\n",
    "  type: feedback\n",
    "  strength: hard\n",
    "---\n",
    "# Collets go back in the rack\n",
    "\n",
    "Every collet goes back in its own slot, by size, before the next job is\n",
    "set up.\n",
);

const KEPT_METADATA_PATH: &str = "memories/collet-rack.md";

/// The example store with that memory in it.
fn store_with_kept_metadata() -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = example_store_files();
    files.push((
        KEPT_METADATA_PATH.to_string(),
        Some(KEPT_METADATA_MEMORY.as_bytes().to_vec()),
    ));
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// The one line of a memory file that starts with `prefix`, as it stands, so
/// that a line a write did not name can be compared byte for byte afterwards.
fn line_with(file: &str, prefix: &str) -> String {
    let mut found = file.lines().filter(|line| line.starts_with(prefix));
    let line = found
        .next()
        .unwrap_or_else(|| panic!("no line starts with {prefix:?} in:\n{file}"));
    assert!(
        found.next().is_none(),
        "more than one line starts with {prefix:?} in:\n{file}"
    );
    format!("\n{line}\n")
}

/// The bytes of a memory file after its frontmatter, which a write that only
/// sets fields must leave exactly as they were.
fn body_of(file: &str) -> &str {
    let after_open = file
        .strip_prefix("---\n")
        .unwrap_or_else(|| panic!("a memory file opens its frontmatter, got:\n{file}"));
    let end = after_open
        .find("\n---\n")
        .unwrap_or_else(|| panic!("a memory file closes its frontmatter, got:\n{file}"));
    &after_open[end + "\n---\n".len()..]
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

/// Detects a field write that rewrites the keys it was not given, or the body:
/// a session adding one key to a memory it did not write would silently drop
/// the keys its keeper put there and reformat text nobody edited.
#[test]
fn setting_one_metadata_key_leaves_the_body_and_the_other_metadata_keys_as_they_were() {
    let server = TestServer::start(store_with_kept_metadata(), |_| {});
    let before = server.store().file_text(KEPT_METADATA_PATH);

    let (status, answer) = server.api(
        "POST",
        "/api/memories/collet-rack/fields",
        Some(&json!({
            "metadata": { "node_type": "memory" },
            "author": AUTHOR,
            "message": "record where the collet rule came from",
        })),
    );

    assert_eq!(
        status, 200,
        "the field write must be accepted, got {answer}"
    );
    let after = server.store().file_text(KEPT_METADATA_PATH);
    assert_eq!(
        body_of(&after),
        body_of(&before),
        "the body must be the bytes it was, got:\n{after}"
    );
    for kept in ["  type:", "  strength:"] {
        let was = line_with(&before, kept);
        assert!(
            after.contains(&was),
            "the line {was:?} was not given and must be the bytes it was, got:\n{after}"
        );
    }
    assert_eq!(
        document(&server, "collet-rack")["metadata"],
        json!({ "type": "feedback", "strength": "hard", "node_type": "memory" }),
        "the memory must read back with the key added and the others kept"
    );
}

/// Detects a `null` written into the file as a value, or one that clears the
/// whole metadata block: the key the write asked to remove is the only one that
/// may go, and a key left behind with a null value is a key still in the file.
#[test]
fn a_metadata_key_set_to_null_is_the_only_key_removed() {
    let server = TestServer::start(store_with_kept_metadata(), |_| {});
    let before = server.store().file_text(KEPT_METADATA_PATH);

    let (status, answer) = server.api(
        "POST",
        "/api/memories/collet-rack/fields",
        Some(&json!({
            "metadata": { "strength": null },
            "author": AUTHOR,
            "message": "the collet rule is no longer a hard rule",
        })),
    );

    assert_eq!(
        status, 200,
        "the field write must be accepted, got {answer}"
    );
    let after = server.store().file_text(KEPT_METADATA_PATH);
    assert!(
        !after.contains("strength"),
        "the key set to null must be out of the file, got:\n{after}"
    );
    let kept = line_with(&before, "  type:");
    assert!(
        after.contains(&kept),
        "the line {kept:?} was not named by the write and must be the bytes it was, got:\n{after}"
    );
    assert_eq!(
        body_of(&after),
        body_of(&before),
        "the body must be the bytes it was, got:\n{after}"
    );
    assert_eq!(
        document(&server, "collet-rack")["metadata"],
        json!({ "type": "feedback" }),
        "only the key set to null may be removed"
    );
}

/// Detects a `source` restricted to the two conventional values: a memory
/// directory in use carries others, and a write that refused one, or stored it
/// as something else, would either fail the import or change what the file
/// says. `forgetmenot check` has to accept the file it leaves behind, because a
/// store it calls invalid is one no session can be delivered from.
#[test]
fn a_source_that_is_neither_user_nor_assistant_is_written_read_back_and_accepted_by_check() {
    let server = TestServer::start(example_store_files(), |_| {});
    let current = document(&server, "bench-power");

    let (status, answer) = server.api(
        "PUT",
        "/api/memories/bench-power",
        Some(&json!({
            "description": current["description"],
            "kind": current["kind"],
            "scopes": current["scopes"],
            "source": "derived",
            "body": current["body"],
            "base_version": current["version"],
            "author": AUTHOR,
            "message": "record that the bench rule was derived",
        })),
    );

    assert_eq!(status, 200, "the write must be accepted, got {answer}");
    assert_eq!(
        document(&server, "bench-power")["source"],
        json!("derived"),
        "the source must read back as it was written"
    );
    let checked = std::process::Command::new(common::binary_path())
        .args(["check", "--store"])
        .arg(server.store().path())
        .output()
        .expect("the forgetmenot binary runs");
    assert!(
        checked.status.success(),
        "check must accept a store whose memory carries this source, got {}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
}

/// Detects a memory moved out of every scope a session works in being withdrawn
/// as a scope the session turned off: no scope was turned off, and a session
/// told otherwise would look for a change it never made. Detects a move that
/// never reaches the contexts the memory was delivered to as well.
#[test]
fn a_memory_moved_out_of_every_active_scope_is_withdrawn_as_one_no_active_scope_covers() {
    let server = TestServer::start(example_store_files(), |_| {});
    // bench-power is the example store's global critical memory, so it arrives
    // in full at a session start, before it is moved anywhere.
    let (_, delivered) = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        context_of(&delivered).contains(BENCH_POWER_BODY),
        "the memory has to have been delivered before it can be withdrawn, got {delivered}"
    );

    // `widgets` is a scope of the example store that this session has not
    // turned on, so the memory is covered by no scope it works in.
    let (status, answer) = server.api(
        "POST",
        "/api/memories/bench-power/fields",
        Some(&json!({
            "scopes": ["widgets"],
            "author": AUTHOR,
            "message": "the bench rule belongs to the widgets work",
        })),
    );
    assert_eq!(status, 200, "the move must be accepted, got {answer}");

    let (status, withdrawn) = server.hook("alpha", SOME_TOKENS, &hook_fixture("stop"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&withdrawn);
    assert!(
        has_retracted_line(text, "bench-power", "no longer in an active scope"),
        "the memory must be reported as one no active scope covers, got {text:?}"
    );
    assert!(
        !has_retracted_line(text, "bench-power", "deleted"),
        "a memory still in the store must not be reported as deleted, got {text:?}"
    );
}

/// Detects a delete that answers 200 without taking the file out of the store,
/// one that removes it in more than one commit, and one that never reaches the
/// contexts the memory was delivered to: a session that was given a rule in full
/// has to be told the rule is gone, because nothing else in its context says so.
#[test]
fn deleting_a_memory_removes_the_file_in_one_commit_and_withdraws_it_where_it_was_delivered() {
    let server = TestServer::start(example_store_files(), |_| {});
    // bench-power is the example store's global critical memory, so it arrives in
    // full at a session start, before anything is deleted.
    let (_, delivered) = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        context_of(&delivered).contains(BENCH_POWER_BODY),
        "the memory has to have been delivered before it can be withdrawn, got {delivered}"
    );
    let path = "memories/bench-power.md";
    let before = commits_touching(&server, path).len();
    let message = "the bench was taken out of the workshop";

    let (status, answer) = server.api(
        "DELETE",
        "/api/memories/bench-power",
        Some(&json!({
            "base_version": document(&server, "bench-power")["version"],
            "author": AUTHOR,
            "message": message,
        })),
    );

    assert_eq!(status, 200, "the delete must be accepted, got {answer}");
    let commits = commits_touching(&server, path);
    assert_eq!(
        commits.len(),
        before + 1,
        "the delete must make exactly one commit, got {commits:?}"
    );
    assert_eq!(
        commits[0],
        (AUTHOR.to_string(), message.to_string()),
        "the commit must carry the author and the message the delete gave"
    );
    assert_eq!(
        file_at(
            &server,
            answer["commit_oid"]
                .as_str()
                .unwrap_or_else(|| panic!("the answer must name the commit, got {answer}")),
            path,
        ),
        None,
        "the commit the delete reports must be one whose tree has no file for the memory"
    );
    assert!(
        !index_ids(&server, "").contains(&"bench-power".to_string()),
        "a deleted memory must not be in the index"
    );
    let (status, gone) = server.api("GET", "/api/memories/bench-power", None);
    assert_eq!(
        status, 404,
        "a deleted memory must no longer be readable, got {gone}"
    );

    let (status, withdrawn) = server.hook("alpha", SOME_TOKENS, &hook_fixture("stop"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&withdrawn);
    assert!(
        has_retracted_line(text, "bench-power", "deleted"),
        "the deleted memory must be reported as withdrawn because it was deleted, got {text:?}"
    );
    assert!(
        !text.contains(BENCH_POWER_BODY),
        "a deleted memory must not be delivered again, got {text:?}"
    );
}

/// Detects a delete that ignores the version it was made from: an editor holding
/// a memory as it was before someone else rewrote it would remove text it never
/// saw, and the person who wrote it would have no way to find out.
#[test]
fn a_delete_from_a_stale_version_is_refused_and_leaves_the_memory_in_the_store() {
    let server = TestServer::start(example_store_files(), |_| {});
    let stale = document(&server, "reading-list");
    let (status, _) = server.api(
        "PUT",
        "/api/memories/reading-list",
        Some(&write_of(
            &stale,
            "The workshop references are all on paper in the binder, nothing online",
            &format!("# Where the workshop references live\n\n{NEW_LINE}\n"),
            "write from the other editor",
        )),
    );
    assert_eq!(status, 200, "the other editor's write must land");
    let after_write = head(&server);

    let (status, answer) = server.api(
        "DELETE",
        "/api/memories/reading-list",
        Some(&json!({
            "base_version": stale["version"],
            "author": AUTHOR,
            "message": "the reading list is not needed any more",
        })),
    );

    assert_eq!(
        status, 409,
        "a delete from a stale version must be refused, got {answer}"
    );
    assert_ne!(
        answer["current"]["version"].as_str(),
        stale["version"].as_str(),
        "the refusal must carry the document as the store has it, got {answer}"
    );
    assert_eq!(
        head(&server),
        after_write,
        "a refused delete must leave the store as it was"
    );
    assert!(
        index_ids(&server, "").contains(&"reading-list".to_string()),
        "a refused delete must leave the memory in the index"
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

/// Detects a write path that refuses `machine` on a trigger that names a text
/// field: `machine` is a plain conjunct on every trigger, so a scope restricted
/// to the tools of one machine must be savable through the API that the scope
/// page writes with.
#[test]
fn a_scope_write_with_a_machine_on_a_tool_name_trigger_is_accepted_and_kept() {
    let server = TestServer::start(example_store_files(), |_| {});
    let (status, scope) = server.api("GET", "/api/scopes/widgets", None);
    assert_eq!(status, 200, "the scope must be readable, got {scope}");

    let (status, answer) = server.api(
        "PUT",
        "/api/scopes/widgets",
        Some(&json!({
            "implies": scope["implies"],
            "triggers": [{ "on": "tool_name", "pattern": "Bash", "machine": "alpha" }],
            "base_version": scope["version"],
            "author": AUTHOR,
            "message": "restrict the tool trigger to alpha",
        })),
    );
    assert_eq!(
        status, 200,
        "a machine on a tool_name trigger must be accepted, got {answer}"
    );

    let (status, saved) = server.api("GET", "/api/scopes/widgets", None);
    assert_eq!(status, 200, "the scope must be readable again, got {saved}");
    assert_eq!(
        saved["triggers"],
        json!([{ "on": "tool_name", "pattern": "Bash", "machine": "alpha" }]),
        "the machine must survive the write, got {saved}"
    );
}

/// Detects a reader that rejects the `type` label scope files used to carry,
/// which would make every scope of a store written before the label was dropped
/// unreadable, and a writer that puts the label back into the file it saves.
#[test]
fn a_scope_file_with_a_legacy_type_key_loads_and_the_key_is_not_written_back() {
    let mut files = example_store_files();
    files.push((
        "scopes/legacy.yaml".to_owned(),
        Some(b"id: legacy\ntype: project\nimplies: [rocketry]\n".to_vec()),
    ));
    let server = TestServer::start(files, |_| {});

    let (status, scope) = server.api("GET", "/api/scopes/legacy", None);
    assert_eq!(
        status, 200,
        "a scope file carrying a legacy type key must still load, got {scope}"
    );
    assert_eq!(
        scope["implies"],
        json!(["rocketry"]),
        "the keys the format defines must survive the legacy key, got {scope}"
    );
    assert!(
        scope.get("type").is_none(),
        "a scope must not report a type, got {scope}"
    );

    let (status, answer) = server.api(
        "PUT",
        "/api/scopes/legacy",
        Some(&json!({
            "implies": ["rocketry"],
            "triggers": [{ "on": "tool_input", "pattern": "legacy" }],
            "base_version": scope["version"],
            "author": AUTHOR,
            "message": "add a trigger to the legacy scope",
        })),
    );
    assert_eq!(status, 200, "the write must succeed, got {answer}");

    let on_disk = std::fs::read_to_string(server.store().path().join("scopes/legacy.yaml"))
        .expect("the saved scope file is readable");
    assert!(
        !on_disk.contains("type"),
        "the saved file must not carry the legacy key, got {on_disk:?}"
    );
    assert!(
        on_disk.contains("legacy"),
        "the saved file must carry the write, got {on_disk:?}"
    );
}

/// Detects a scope delete that answers 200 without taking the file out of the
/// store, one that removes it in more than one commit, and one that leaves the
/// scope in the index a person picks scopes from: an editor would keep offering a
/// scope that is gone.
///
/// `workshop` is the example store's scope that no memory lists and no other
/// scope implies, so nothing about the store refuses this deletion.
#[test]
fn deleting_an_unreferenced_scope_removes_the_file_in_one_commit_and_stops_listing_it() {
    let server = TestServer::start(example_store_files(), |_| {});
    let path = "scopes/workshop.yaml";
    let before = commits_touching(&server, path).len();
    let message = "the workshop directory moved off this machine";

    let (status, answer) = server.api(
        "DELETE",
        "/api/scopes/workshop",
        Some(&json!({
            "base_version": scope(&server, "workshop")["version"],
            "author": AUTHOR,
            "message": message,
        })),
    );

    assert_eq!(status, 200, "the delete must be accepted, got {answer}");
    let commits = commits_touching(&server, path);
    assert_eq!(
        commits.len(),
        before + 1,
        "the delete must make exactly one commit, got {commits:?}"
    );
    assert_eq!(
        commits[0],
        (AUTHOR.to_string(), message.to_string()),
        "the commit must carry the author and the message the delete gave"
    );
    assert_eq!(
        file_at(
            &server,
            answer["commit_oid"]
                .as_str()
                .unwrap_or_else(|| panic!("the answer must name the commit, got {answer}")),
            path,
        ),
        None,
        "the commit the delete reports must be one whose tree has no file for the scope"
    );
    assert!(
        !scope_ids(&server).contains(&"workshop".to_string()),
        "a deleted scope must not be in the scope index"
    );
    let (status, gone) = server.api("GET", "/api/scopes/workshop", None);
    assert_eq!(
        status, 404,
        "a deleted scope must no longer be readable, got {gone}"
    );
}

/// Detects a scope delete that goes ahead while a memory still lists the scope:
/// the store would be left with a memory naming a scope that does not exist,
/// which `forgetmenot check` calls invalid, and the memory would be due in no
/// context with nothing saying why.
#[test]
fn deleting_a_scope_a_memory_still_lists_is_refused_naming_that_memory_and_makes_no_commit() {
    let server = TestServer::start(example_store_files(), |_| {});
    // widget-naming is the example store's memory scoped to `widgets`.
    let version = scope(&server, "widgets")["version"].clone();
    let before = head(&server);

    let (status, answer) = server.api(
        "DELETE",
        "/api/scopes/widgets",
        Some(&json!({
            "base_version": version,
            "author": AUTHOR,
            "message": "widgets are not made here any more",
        })),
    );

    assert_eq!(
        status, 422,
        "deleting a scope a memory lists must be refused, got {answer}"
    );
    assert!(
        answer["errors"].as_array().is_some_and(|errors| errors
            .iter()
            .any(|error| error["path"] == json!("memories/widget-naming.md"))),
        "the refusal must name the memory that has to be edited first, got {answer}"
    );
    assert_eq!(
        head(&server),
        before,
        "a refused delete must leave the store as it was"
    );
    assert!(
        scope_ids(&server).contains(&"widgets".to_string()),
        "a refused delete must leave the scope in the index"
    );
}

/// Detects a scope delete that only looks at the memories: a scope whose
/// `implies` target is gone is just as invalid, and the implication would
/// silently stop turning anything on.
#[test]
fn deleting_a_scope_another_scope_implies_is_refused_naming_that_scope_and_makes_no_commit() {
    let mut files = example_store_files();
    files.push((
        "scopes/bench.yaml".to_owned(),
        Some(b"id: bench\nimplies: [workshop]\n".to_vec()),
    ));
    let server = TestServer::start(files, |_| {});
    let before = head(&server);

    let (status, answer) = server.api(
        "DELETE",
        "/api/scopes/workshop",
        Some(&json!({
            "base_version": scope(&server, "workshop")["version"],
            "author": AUTHOR,
            "message": "the workshop directory moved off this machine",
        })),
    );

    assert_eq!(
        status, 422,
        "deleting a scope another scope implies must be refused, got {answer}"
    );
    assert!(
        answer["errors"].as_array().is_some_and(|errors| errors
            .iter()
            .any(|error| error["path"] == json!("scopes/bench.yaml"))),
        "the refusal must name the scope that has to be edited first, got {answer}"
    );
    assert_eq!(
        head(&server),
        before,
        "a refused delete must leave the store as it was"
    );
    assert!(
        scope_ids(&server).contains(&"workshop".to_string()),
        "a refused delete must leave the scope in the index"
    );
}

/// Detects a scope delete that ignores the version it was made from: an editor
/// holding a scope as it was before someone else changed its triggers would
/// remove work it never saw.
#[test]
fn a_scope_delete_from_a_stale_version_is_refused_and_leaves_the_scope_in_the_store() {
    let server = TestServer::start(example_store_files(), |_| {});
    let stale = scope(&server, "workshop");
    let (status, answer) = server.api(
        "PUT",
        "/api/scopes/workshop",
        Some(&json!({
            "implies": stale["implies"],
            "triggers": [{ "on": "tool_input", "pattern": "bench" }],
            "base_version": stale["version"],
            "author": AUTHOR,
            "message": "match the bench in tool input as well",
        })),
    );
    assert_eq!(
        status, 200,
        "the other editor's write must land, got {answer}"
    );
    let after_write = head(&server);

    let (status, answer) = server.api(
        "DELETE",
        "/api/scopes/workshop",
        Some(&json!({
            "base_version": stale["version"],
            "author": AUTHOR,
            "message": "the workshop directory moved off this machine",
        })),
    );

    assert_eq!(
        status, 409,
        "a delete from a stale version must be refused, got {answer}"
    );
    assert_ne!(
        answer["current"]["version"].as_str(),
        stale["version"].as_str(),
        "the refusal must carry the scope as the store has it, got {answer}"
    );
    assert_eq!(
        head(&server),
        after_write,
        "a refused delete must leave the store as it was"
    );
    assert!(
        scope_ids(&server).contains(&"workshop".to_string()),
        "a refused delete must leave the scope in the index"
    );
}

/// Detects an implicit scope being answered as a document that could be deleted:
/// `global` has no file, and a delete that reported anything but a refusal would
/// suggest a session's own scopes can be taken away from it.
#[test]
fn deleting_an_implicit_scope_is_refused_as_a_bad_request() {
    let server = TestServer::start(example_store_files(), |_| {});
    let before = head(&server);

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
    assert_eq!(
        head(&server),
        before,
        "a refused delete must leave the store as it was"
    );
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// The settings of the store the server is answering from.
fn settings(server: &TestServer) -> Value {
    let (status, answer) = server.api("GET", "/api/settings", None);
    assert_eq!(
        status, 200,
        "reading the settings must succeed, got {answer}"
    );
    answer
}

/// Detects a store with no settings file reported as anything other than the
/// documented defaults, and one reported with a version a write could send back:
/// an editor told the file exists would write against a version that is not
/// there, and one told the wrong defaults would show a behaviour nobody has.
#[test]
fn the_settings_of_a_store_without_a_file_are_the_defaults_at_no_version() {
    let server = TestServer::start(example_store_without_settings(), |_| {});

    let answer = settings(&server);

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
            "subagents_inherit_scopes": true,
            "deliver_knowledge_index": true,
            "tool_result_match_limit": 262_144,
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

/// Detects a write of the first setting that does not create the file, one that
/// answers 200 without committing, and a change that does not reach the settings
/// the server answers from: the whole point of the file is that the next read
/// sees it.
#[test]
fn writing_a_setting_creates_the_file_in_one_commit_and_changes_what_is_read_back() {
    let server = TestServer::start(example_store_without_settings(), |_| {});
    let before = head(&server);

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
    assert_ne!(head(&server), before, "the write must make a commit");
    assert_eq!(
        commits_touching(&server, "config.yml").len(),
        1,
        "the write must make exactly one commit on the settings file"
    );
    let answer = settings(&server);
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

/// Detects a write that leaves the settings the store already had: a change to
/// one key must not quietly take every other one back to its default.
#[test]
fn writing_one_setting_leaves_the_others_as_the_store_had_them() {
    let server = TestServer::start(
        example_store_with_settings("reminder_tokens: 120000\n"),
        |_| {},
    );

    let (status, written) = server.api(
        "PUT",
        "/api/settings/deliver_knowledge_index",
        Some(&json!({
            "value": false,
            "author": AUTHOR,
            "message": "fetch knowledge on demand instead of indexing it",
        })),
    );

    assert_eq!(status, 200, "the write must be accepted, got {written}");
    let answer = settings(&server);
    assert_eq!(
        answer["settings"]["deliver_knowledge_index"],
        json!(false),
        "the written key must change, got {answer}"
    );
    assert_eq!(
        answer["settings"]["reminder_tokens"],
        json!(120_000),
        "the key the write did not name must be left alone, got {answer}"
    );
}

/// Detects a settings write that accepts a key this server does not act on, or a
/// value of the wrong type: either would be a setting written down, committed and
/// silently doing nothing.
#[test]
fn a_setting_this_server_does_not_have_or_a_value_of_the_wrong_type_is_refused() {
    let server = TestServer::start(example_store_files(), |_| {});
    let before = head(&server);

    let (unknown_key, unknown_answer) = server.api(
        "PUT",
        "/api/settings/remind_tokens",
        Some(&json!({
            "value": 1_000,
            "author": AUTHOR,
            "message": "set a reminder threshold",
        })),
    );
    let (wrong_type, wrong_answer) = server.api(
        "PUT",
        "/api/settings/interrupt_on_critical",
        Some(&json!({
            "value": "yes",
            "author": AUTHOR,
            "message": "hold tool calls for critical memories",
        })),
    );

    assert_eq!(
        unknown_key, 422,
        "a key this server does not have must be refused, got {unknown_answer}"
    );
    assert_eq!(
        wrong_type, 422,
        "a value of the wrong type must be refused, got {wrong_answer}"
    );
    assert_eq!(
        head(&server),
        before,
        "a refused settings write must leave the store as it was"
    );
}

/// Detects a settings write that ignores the version it was made from: two
/// people changing the settings at once would overwrite each other, and the
/// second would never see that the first had written anything.
#[test]
fn a_settings_write_from_a_stale_version_is_refused_with_the_settings_as_they_are() {
    let server = TestServer::start(example_store_files(), |_| {});
    let stale = settings(&server);
    let (status, _) = server.api(
        "PUT",
        "/api/settings/reminder_tokens",
        Some(&json!({
            "value": 90_000,
            "base_version": stale["version"],
            "author": AUTHOR,
            "message": "remind the agent of the rules every 90k tokens",
        })),
    );
    assert_eq!(status, 200, "the first write must land");

    let (status, answer) = server.api(
        "PUT",
        "/api/settings/reminder_tokens",
        Some(&json!({
            "value": 10_000,
            "base_version": stale["version"],
            "author": AUTHOR,
            "message": "remind the agent of the rules every 10k tokens",
        })),
    );

    assert_eq!(
        status, 409,
        "a write from a version that is no longer current must be refused, got {answer}"
    );
    assert_eq!(
        answer["current"]["settings"]["reminder_tokens"],
        json!(90_000),
        "the refusal must carry the settings as the store has them, got {answer}"
    );
    assert_eq!(
        settings(&server)["settings"]["reminder_tokens"],
        json!(90_000),
        "the refused write must not have changed anything"
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

/// Detects an `any` trigger that the page cannot show anything about: the
/// example store's `rocketry` trigger names no field, so it is about every text
/// a session produces, and a test of a tool input must report it as firing there
/// like any trigger written for that field.
#[test]
fn the_trigger_test_reports_an_any_trigger_for_the_field_it_was_asked_about() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api(
        "POST",
        "/api/triggers/test",
        Some(&json!({
            "field": "tool_input",
            "text": "cargo test -p rocketry",
            "machine": "beta",
        })),
    );

    assert_eq!(status, 200, "a trigger test must be answered, got {answer}");
    let hit = answer["fired"]
        .as_array()
        .expect("the answer lists what fired")
        .iter()
        .find(|hit| hit["scope_id"] == json!("rocketry"))
        .unwrap_or_else(|| panic!("the any trigger must fire on a tool input, got {answer}"));
    assert_eq!(
        hit["field"],
        json!("tool_input"),
        "the hit must name the field the text was matched as, got {hit}"
    );
    assert!(
        hit["pattern"]
            .as_str()
            .is_some_and(|pattern| pattern.contains("rocket")),
        "the hit must name the pattern that matched, got {hit}"
    );
}

/// Detects a trigger test that refuses `any` as the field it is asked about, or
/// answers it from one field's patterns: `any` is what a person picks when they
/// do not know which text their pattern will meet, so it must report every
/// trigger the text fires, whatever field each trigger's own file names.
#[test]
fn the_trigger_test_accepts_any_as_the_field_and_reports_every_trigger_the_text_fires() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api(
        "POST",
        "/api/triggers/test",
        Some(&json!({
            "field": "any",
            "text": "the widget on the rocket",
            "machine": "beta",
        })),
    );

    assert_eq!(status, 200, "a trigger test must be answered, got {answer}");
    let scopes: BTreeSet<&str> = answer["fired"]
        .as_array()
        .expect("the answer lists what fired")
        .iter()
        .map(|hit| hit["scope_id"].as_str().expect("a scope id is text"))
        .collect();
    assert!(
        scopes.contains("widgets"),
        "a trigger written for one field must be reported when the field is any, got {answer}"
    );
    assert!(
        scopes.contains("rocketry"),
        "an any trigger must be reported when the field is any, got {answer}"
    );
}

/// Detects a pattern check that calls a pattern the store would refuse good,
/// which would let an editor save a trigger that can never fire, and one that
/// reports the refusal without the compiler's message, which leaves the author
/// with nothing to fix.
#[test]
fn the_pattern_check_refuses_a_pattern_that_does_not_compile_and_says_why() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api(
        "POST",
        "/api/triggers/validate",
        Some(&json!({ "pattern": "[unclosed" })),
    );

    assert_eq!(
        status, 200,
        "a pattern check must be answered, got {answer}"
    );
    assert_eq!(
        answer["ok"],
        json!(false),
        "a pattern that does not compile must not be reported as good, got {answer}"
    );
    assert!(
        answer["error"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "the answer must carry what was wrong with the pattern, got {answer}"
    );
}

/// Detects a pattern check that refuses a pattern the store carries, which
/// would stop an editor saving a trigger that works: the pattern here is one of
/// the example store's own.
#[test]
fn the_pattern_check_accepts_a_pattern_the_store_already_carries() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.api(
        "POST",
        "/api/triggers/validate",
        Some(&json!({ "pattern": r"\brocket(s|ry)?\b" })),
    );

    assert_eq!(
        status, 200,
        "a pattern check must be answered, got {answer}"
    );
    assert_eq!(
        answer["ok"],
        json!(true),
        "a pattern that compiles must be reported as good, got {answer}"
    );
    assert!(
        answer.get("error").is_none(),
        "a pattern that compiles must carry no error, got {answer}"
    );
}

/// Detects a machine list read from the live contexts alone, or from the log
/// alone: a restart empties neither, but a machine whose sessions are over is
/// only in the log, and the list is what the frontend offers wherever a machine
/// is picked, so a name missing from it cannot be chosen at all.
#[test]
fn the_machines_page_lists_every_machine_that_has_sent_an_event_across_a_restart() {
    let mut server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("beta", SOME_TOKENS, &hook_fixture("session_start"));
    server.restart();

    let (status, answer) = server.api("GET", "/api/machines", None);

    assert_eq!(status, 200, "the machines must be readable, got {answer}");
    let machines: Vec<&str> = answer["machines"]
        .as_array()
        .expect("the answer lists the machines")
        .iter()
        .map(|machine| machine.as_str().expect("a machine name is text"))
        .collect();
    for expected in ["alpha", "beta"] {
        assert!(
            machines.contains(&expected),
            "{expected} sent an event, so it must be listed, got {answer}"
        );
    }
    let mut sorted_without_repeats = machines.clone();
    sorted_without_repeats.sort_unstable();
    sorted_without_repeats.dedup();
    assert_eq!(
        machines, sorted_without_repeats,
        "the machines must be sorted and named once each, got {answer}"
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
