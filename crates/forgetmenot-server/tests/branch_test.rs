//! Tests of transactions, which are git branches: that a write on a branch
//! reaches nothing until it lands, that landing is one commit, that two branches
//! that touched different text both land, that two that touched the same text are
//! reported as a conflict, and that the housekeeping of open branches survives a
//! restart.
//!
//! Everything is asserted through the HTTP answers and, where an answer claims a
//! commit was made, through the store's git history read with a second handle: a
//! land that answered 200 without moving `main`, or that made two commits instead
//! of one, would fail here.
//!
//! The expectations come from the branch contract: a branch is invisible until it
//! lands, a land is exactly one commit on `main` with the caller's title, a merge
//! is three-way against the merge base, and a conflict leaves both sides alone.

mod common;

use serde_json::{Value, json};

use common::{
    TestServer, additional_context, api_request, example_store_files, hook_fixture,
    permission_decision,
};

/// The author name a write carries.
const AUTHOR: &str = "wiki";

/// The session that opens the branches here.
const SESSION: &str = "alpha/session-9";

/// The context size reported with the hook events used here, which are not about
/// staleness.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// A line that appears in no memory of the example store, so that "the write
/// landed" can be told apart from "the old text is still there".
const NEW_LINE: &str = "The binder lives on the shelf by the door.";

// ---------------------------------------------------------------------------
// Fixtures and helpers
// ---------------------------------------------------------------------------

/// A memory file, built the way the store holds one.
fn memory_file(name: &str, kind: &str, scopes: &[&str], body: &str) -> (String, Option<Vec<u8>>) {
    let scopes: String = scopes
        .iter()
        .map(|scope| format!("  - {scope}\n"))
        .collect();
    let text = format!(
        "---\nname: {name}\ndescription: the index entry of {name}\nmetadata:\n  kind: {kind}\n  \
         scopes:\n{scopes}  source: user\n---\n{body}"
    );
    (format!("memories/{name}.md"), Some(text.into_bytes()))
}

/// The three paragraphs of `bracket-order`, each on its own, so that two branches
/// can edit different ones and the merge has unchanged lines between them.
const FIRST_PARAGRAPH: &str = "The first bracket is measured from the left edge.";
const SECOND_PARAGRAPH: &str = "The second bracket is measured from the centre line.";
const THIRD_PARAGRAPH: &str = "The third bracket is measured from the right edge.";

fn bracket_order_body(first: &str, second: &str, third: &str) -> String {
    format!("# Bracket order\n\n{first}\n\n{second}\n\n{third}\n")
}

/// The example store, plus a memory with three paragraphs and a second memory
/// that links to `rocket-stages`, so that a rename has two linkers to rewrite.
fn store_files() -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = example_store_files();
    files.push(memory_file(
        "bracket-order",
        "knowledge",
        &["global"],
        &bracket_order_body(FIRST_PARAGRAPH, SECOND_PARAGRAPH, THIRD_PARAGRAPH),
    ));
    files.push(memory_file(
        "stage-report",
        "knowledge",
        &["rocketry"],
        "# Stage reports\n\nA stage report files its numbers under [[rocket-stages]].\n",
    ));
    files
}

/// Open a branch for `session_key`.
fn open_branch(server: &TestServer, session_key: &str) -> String {
    let (status, answer) = server.api(
        "POST",
        "/api/branches",
        Some(&json!({ "session_key": session_key })),
    );
    assert_eq!(status, 200, "opening a branch must succeed, got {answer}");
    answer["branch"]
        .as_str()
        .expect("the answer names the branch")
        .to_string()
}

/// One memory as the API reports it from `main`.
fn document(server: &TestServer, id: &str) -> Value {
    let (status, answer) = server.api("GET", &format!("/api/memories/{id}"), None);
    assert_eq!(status, 200, "reading {id} must succeed, got {answer}");
    answer
}

/// The version field of a document or of a write's answer.
fn version(answer: &Value) -> String {
    answer["version"]
        .as_str()
        .expect("a version is text")
        .to_string()
}

/// The body of one memory as `main` has it.
fn body(server: &TestServer, id: &str) -> String {
    document(server, id)["body"]
        .as_str()
        .expect("a body is text")
        .to_string()
}

/// A path with the branch a write goes to, or without one for a write to `main`.
fn with_branch(path: &str, branch: Option<&str>) -> String {
    match branch {
        Some(branch) => format!("{path}?branch={branch}"),
        None => path.to_string(),
    }
}

/// Replace one memory's body, from the version given, on `branch` when given.
fn put_body(
    server: &TestServer,
    id: &str,
    base_version: &str,
    new_body: &str,
    message: &str,
    branch: Option<&str>,
) -> (u16, Value) {
    let current = document(server, id);
    server.api(
        "PUT",
        &with_branch(&format!("/api/memories/{id}"), branch),
        Some(&json!({
            "description": current["description"],
            "kind": current["kind"],
            "scopes": current["scopes"],
            "source": current["source"],
            "body": new_body,
            "base_version": base_version,
            "author": AUTHOR,
            "message": message,
        })),
    )
}

/// Replace one memory's body and fail the test if the write is refused.
fn put_body_ok(
    server: &TestServer,
    id: &str,
    base_version: &str,
    new_body: &str,
    message: &str,
    branch: Option<&str>,
) -> String {
    let (status, answer) = put_body(server, id, base_version, new_body, message, branch);
    assert_eq!(status, 200, "the write of {id} must land, got {answer}");
    version(&answer)
}

/// Land one branch as one commit titled `message`.
fn land(server: &TestServer, branch: &str, message: &str) -> (u16, Value) {
    server.api(
        "POST",
        &format!("/api/branches/{branch}/land"),
        Some(&json!({ "message": message, "author": AUTHOR })),
    )
}

/// Land one branch and fail the test if it does not land.
fn land_ok(server: &TestServer, branch: &str, message: &str) -> Value {
    let (status, answer) = land(server, branch, message);
    assert_eq!(status, 200, "{branch} must land, got {answer}");
    answer
}

/// The whole branch list, as the API reports it.
fn branch_list(server: &TestServer) -> Vec<Value> {
    let (status, answer) = server.api("GET", "/api/branches", None);
    assert_eq!(status, 200, "the branches must be listable, got {answer}");
    answer
        .as_array()
        .expect("the branch list is an array")
        .clone()
}

/// The open branches, by name.
fn open_branches(server: &TestServer) -> Vec<String> {
    let (status, answer) = server.api("GET", "/api/branches", None);
    assert_eq!(status, 200, "the branches must be listable, got {answer}");
    answer
        .as_array()
        .expect("the branch list is an array")
        .iter()
        .map(|row| {
            row["branch"]
                .as_str()
                .expect("a branch name is text")
                .to_string()
        })
        .collect()
}

/// The row the branch list reports for one branch.
fn branch_row(server: &TestServer, branch: &str) -> Value {
    let (status, answer) = server.api("GET", "/api/branches", None);
    assert_eq!(status, 200, "the branches must be listable, got {answer}");
    answer
        .as_array()
        .expect("the branch list is an array")
        .iter()
        .find(|row| row["branch"] == json!(branch))
        .unwrap_or_else(|| panic!("{branch} must be listed, got {answer}"))
        .clone()
}

/// Every commit reachable from the store's head, newest first, with its title
/// and how many parents it has, read with a second handle on the repository.
fn commits_from_head(server: &TestServer) -> Vec<(String, String, usize)> {
    let repository = git2::Repository::open(server.store().path()).expect("the store opens");
    let mut walk = repository.revwalk().expect("the history can be walked");
    walk.push_head().expect("the store has a head");
    walk.map(|oid| {
        let commit = repository
            .find_commit(oid.expect("an oid in the walk"))
            .expect("a commit in the walk");
        (
            commit.id().to_string(),
            commit
                .message()
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
            commit.parent_count(),
        )
    })
    .collect()
}

/// The store's head, read through the test's own handle.
fn head(server: &TestServer) -> String {
    server
        .store()
        .repository()
        .head_oid()
        .expect("the store has a head")
        .to_string()
}

/// The text the answer injects, or the empty string when it injects nothing.
fn context_of(answer: &Value) -> &str {
    additional_context(answer).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Landing
// ---------------------------------------------------------------------------

/// Detects a branch write that reaches `main` or a session before it is landed,
/// which would make a transaction no transaction at all: half a change would be
/// delivered while the rest was still being written.
#[test]
fn a_write_on_a_branch_is_invisible_to_main_and_to_hook_deliveries_until_it_lands() {
    let server = TestServer::start(store_files(), |_| {});
    let first = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        !context_of(&first.1).contains(NEW_LINE),
        "the new line cannot be in the store yet, got {first:?}"
    );
    let before = head(&server);
    let branch = open_branch(&server, SESSION);

    put_body_ok(
        &server,
        "bench-power",
        &version(&document(&server, "bench-power")),
        &format!("# Cut bench power before rewiring\n\n{NEW_LINE}\n"),
        "note where the binder lives",
        Some(&branch),
    );

    assert_eq!(
        head(&server),
        before,
        "a write on a branch must not move main"
    );
    assert!(
        !body(&server, "bench-power").contains(NEW_LINE),
        "the memory on main must still be the version main had"
    );
    let (status, next) = server.hook("alpha", SOME_TOKENS, &hook_fixture("stop"));
    assert_eq!(status, 200, "the next event must be answered");
    assert!(
        !context_of(&next).contains(NEW_LINE),
        "a branch write must not be delivered to any context, got {next}"
    );
}

/// Detects a land that is not one commit: three writes on a branch arriving as
/// three commits, or as a merge commit with two parents, would make the history
/// the reason to use a branch worthless, and a land that loses one of the three
/// would publish half a change.
#[test]
fn landing_three_writes_on_a_branch_makes_one_commit_on_main_with_all_three_changes() {
    let server = TestServer::start(store_files(), |_| {});
    let before = commits_from_head(&server);
    let branch = open_branch(&server, SESSION);
    let title = "record the three bracket rules";

    put_body_ok(
        &server,
        "bench-power",
        &version(&document(&server, "bench-power")),
        &format!("# Cut bench power before rewiring\n\n{NEW_LINE}\n"),
        "note where the binder lives",
        Some(&branch),
    );
    put_body_ok(
        &server,
        "bracket-order",
        &version(&document(&server, "bracket-order")),
        &bracket_order_body("The first bracket moved", SECOND_PARAGRAPH, THIRD_PARAGRAPH),
        "move the first bracket",
        Some(&branch),
    );
    let (status, created) = server.api(
        "POST",
        &with_branch("/api/memories", Some(&branch)),
        Some(&json!({
            "id": "bracket-jig",
            "description": "The bracket jig lives in the second drawer",
            "kind": "knowledge",
            "scopes": ["global"],
            "source": "user",
            "body": "# The bracket jig\n\nThe jig lives in the second drawer.\n",
            "author": AUTHOR,
            "message": "record where the bracket jig lives",
        })),
    );
    assert_eq!(
        status, 200,
        "the create on the branch must land, got {created}"
    );

    let landed = land_ok(&server, &branch, title);

    let after = commits_from_head(&server);
    assert_eq!(
        after.len(),
        before.len() + 1,
        "landing three writes must add exactly one commit"
    );
    let (oid, message, parents) = after
        .first()
        .cloned()
        .expect("the store has a newest commit");
    assert_eq!(
        oid,
        landed["commit_oid"].as_str().unwrap_or_default(),
        "the answer must name the commit that was made"
    );
    assert_eq!(
        message, title,
        "the commit's title must be the given message"
    );
    assert_eq!(
        parents, 1,
        "the squashed commit must have main as its only parent"
    );
    assert!(
        body(&server, "bench-power").contains(NEW_LINE),
        "the first write must be in the landed tree"
    );
    assert!(
        body(&server, "bracket-order").contains("The first bracket moved"),
        "the second write must be in the landed tree"
    );
    assert!(
        body(&server, "bracket-jig").contains("second drawer"),
        "the third write must be in the landed tree"
    );
    assert!(
        !open_branches(&server).contains(&branch),
        "the branch must be gone once it has landed"
    );
}

/// Detects a land that delivers its writes one by one: three writes to one
/// critical memory on a branch must interrupt the session once, not three times,
/// and must not interrupt it again afterwards.
#[test]
fn a_hook_event_after_a_land_delivers_the_changed_critical_memory_once() {
    let server = TestServer::start(store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    let branch = open_branch(&server, SESSION);

    let mut version_on_branch = version(&document(&server, "bench-power"));
    for step in 1..=3 {
        version_on_branch = put_body_ok(
            &server,
            "bench-power",
            &version_on_branch,
            &format!("# Cut bench power before rewiring\n\n{NEW_LINE} Step {step}.\n"),
            &format!("rewrite the bench power rule, step {step}"),
            Some(&branch),
        );
    }
    land_ok(&server, &branch, "rewrite the bench power rule");

    let (first_status, first) =
        server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_read"));
    let (second_status, second) =
        server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_read"));

    assert_eq!(first_status, 200, "the first event must be answered");
    assert_eq!(second_status, 200, "the second event must be answered");
    assert!(
        context_of(&first).contains("Step 3."),
        "the landed version must be delivered in full, got {first}"
    );
    assert_eq!(
        permission_decision(&first),
        Some("deny"),
        "a changed critical memory must stop the call once, got {first}"
    );
    assert!(
        !context_of(&second).contains(NEW_LINE),
        "the landed memory must not be delivered again, got {second}"
    );
    assert_ne!(
        permission_decision(&second),
        Some("deny"),
        "one land must not stop the next call as well, got {second}"
    );
}

// ---------------------------------------------------------------------------
// Merges
// ---------------------------------------------------------------------------

/// Detects a land that only works against the head the branch was cut from: two
/// branches that touched different memories must both land whichever goes first,
/// or the second agent's whole transaction would be lost to the first.
#[test]
fn two_branches_editing_different_memories_both_land_in_either_order() {
    for order in [["first", "second"], ["second", "first"]] {
        let server = TestServer::start(store_files(), |_| {});
        let first = open_branch(&server, "alpha/session-1");
        let second = open_branch(&server, "alpha/session-2");
        put_body_ok(
            &server,
            "bench-power",
            &version(&document(&server, "bench-power")),
            "# Cut bench power before rewiring\n\nEdited by the first branch.\n",
            "rewrite the bench power rule",
            Some(&first),
        );
        put_body_ok(
            &server,
            "reading-list",
            &version(&document(&server, "reading-list")),
            "# Where the workshop references live\n\nEdited by the second branch.\n",
            "rewrite the reading list",
            Some(&second),
        );

        for which in order {
            let branch = if which == "first" { &first } else { &second };
            land_ok(&server, branch, &format!("land the {which} branch"));
        }

        assert!(
            body(&server, "bench-power").contains("Edited by the first branch."),
            "the first branch's edit must survive landing in the order {order:?}"
        );
        assert!(
            body(&server, "reading-list").contains("Edited by the second branch."),
            "the second branch's edit must survive landing in the order {order:?}"
        );
    }
}

/// Detects a land that replaces the file instead of merging it: two branches that
/// edited different paragraphs of one memory must both land with both edits, which
/// is the whole reason the land is a three-way merge and not a checkout.
#[test]
fn two_branches_editing_different_paragraphs_of_one_memory_both_land_with_both_edits() {
    let server = TestServer::start(store_files(), |_| {});
    let base = version(&document(&server, "bracket-order"));
    let first_branch = open_branch(&server, "alpha/session-1");
    let second_branch = open_branch(&server, "alpha/session-2");
    let first_edit = "The first bracket is measured from the jig face.";
    let third_edit = "The third bracket is measured from the jig heel.";

    put_body_ok(
        &server,
        "bracket-order",
        &base,
        &bracket_order_body(first_edit, SECOND_PARAGRAPH, THIRD_PARAGRAPH),
        "measure the first bracket from the jig",
        Some(&first_branch),
    );
    put_body_ok(
        &server,
        "bracket-order",
        &base,
        &bracket_order_body(FIRST_PARAGRAPH, SECOND_PARAGRAPH, third_edit),
        "measure the third bracket from the jig",
        Some(&second_branch),
    );

    land_ok(
        &server,
        &first_branch,
        "measure the first bracket from the jig",
    );
    land_ok(
        &server,
        &second_branch,
        "measure the third bracket from the jig",
    );

    let landed = body(&server, "bracket-order");
    assert!(
        landed.contains(first_edit),
        "the first branch's paragraph must be in the result, got {landed:?}"
    );
    assert!(
        landed.contains(third_edit),
        "the second branch's paragraph must be in the result, got {landed:?}"
    );
    assert!(
        landed.contains(SECOND_PARAGRAPH),
        "the paragraph neither branch touched must be unchanged, got {landed:?}"
    );
}

/// Detects a land that overwrites `main` with the branch's tree: a write made
/// directly to `main` while the branch was open must be merged, not dropped, and
/// the land must retry against the head it lost to rather than fail.
#[test]
fn a_direct_write_to_main_between_branch_creation_and_landing_is_merged_not_lost() {
    let server = TestServer::start(store_files(), |_| {});
    let before = commits_from_head(&server);
    let branch = open_branch(&server, SESSION);

    put_body_ok(
        &server,
        "bench-power",
        &version(&document(&server, "bench-power")),
        "# Cut bench power before rewiring\n\nEdited on the branch.\n",
        "rewrite the bench power rule",
        Some(&branch),
    );
    put_body_ok(
        &server,
        "reading-list",
        &version(&document(&server, "reading-list")),
        "# Where the workshop references live\n\nEdited straight on main.\n",
        "rewrite the reading list",
        None,
    );

    land_ok(&server, &branch, "rewrite the bench power rule");

    assert!(
        body(&server, "reading-list").contains("Edited straight on main."),
        "the write made on main must survive the land"
    );
    assert!(
        body(&server, "bench-power").contains("Edited on the branch."),
        "the write made on the branch must land"
    );
    let after = commits_from_head(&server);
    assert_eq!(
        after.len(),
        before.len() + 2,
        "the direct write and the land must be one commit each"
    );
    for (oid, _, parents) in &after {
        assert!(
            *parents <= 1,
            "the history must stay linear; commit {oid} has {parents} parents"
        );
    }
}

// ---------------------------------------------------------------------------
// Conflicts
// ---------------------------------------------------------------------------

/// Two branches that both rewrote the first paragraph of `bracket-order`, and the
/// version each branch holds it at.
fn branches_editing_the_same_line(server: &TestServer) -> (String, String, String) {
    let base = version(&document(server, "bracket-order"));
    let first_branch = open_branch(server, "alpha/session-1");
    let second_branch = open_branch(server, "alpha/session-2");
    put_body_ok(
        server,
        "bracket-order",
        &base,
        &bracket_order_body(
            "The first bracket is measured from the jig face.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "measure the first bracket from the jig face",
        Some(&first_branch),
    );
    let second_version = put_body_ok(
        server,
        "bracket-order",
        &base,
        &bracket_order_body(
            "The first bracket is measured from the table edge.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "measure the first bracket from the table edge",
        Some(&second_branch),
    );
    (first_branch, second_branch, second_version)
}

/// Detects a land that resolves a real conflict by picking a side: two branches
/// that rewrote the same line must not silently lose one of them. The answer has
/// to carry the file and all three texts, or the agent cannot resolve it, and
/// both `main` and the branch have to be left as they were.
#[test]
fn landing_a_branch_that_changed_a_line_main_also_changed_is_a_conflict_naming_both_texts() {
    let server = TestServer::start(store_files(), |_| {});
    let (first_branch, second_branch, _) = branches_editing_the_same_line(&server);
    land_ok(&server, &first_branch, "measure from the jig face");
    let head_before = head(&server);

    let (status, answer) = land(&server, &second_branch, "measure from the table edge");

    assert_eq!(
        status, 409,
        "the second land must be a conflict, got {answer}"
    );
    let conflicts = answer["conflicts"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer must list the conflicting files, got {answer}"));
    let conflict = conflicts
        .iter()
        .find(|entry| entry["path"] == json!("memories/bracket-order.md"))
        .unwrap_or_else(|| panic!("the conflict must name the file, got {answer}"));
    assert!(
        conflict["base"]
            .as_str()
            .is_some_and(|text| text.contains(FIRST_PARAGRAPH)),
        "the conflict must carry the text both sides started from, got {conflict}"
    );
    assert!(
        conflict["ours"]
            .as_str()
            .is_some_and(|text| text.contains("from the jig face")),
        "the conflict must carry the text on main, got {conflict}"
    );
    assert!(
        conflict["theirs"]
            .as_str()
            .is_some_and(|text| text.contains("from the table edge")),
        "the conflict must carry the text on the branch, got {conflict}"
    );
    assert_eq!(
        head(&server),
        head_before,
        "a conflicting land must leave main where it was"
    );
    assert!(
        open_branches(&server).contains(&second_branch),
        "a conflicting land must leave the branch to be resolved on"
    );
    assert!(
        body(&server, "bracket-order").contains("from the jig face"),
        "the version that did land must still be the one on main"
    );
}

/// Detects a conflict that cannot be resolved: after a land was refused, writing
/// the file the agent wants on the branch and landing again must succeed, or a
/// conflict would be a dead end and the transaction would have to be thrown away.
#[test]
fn overwriting_the_conflicting_file_on_the_branch_lets_it_land() {
    let server = TestServer::start(store_files(), |_| {});
    let (first_branch, second_branch, version_on_branch) = branches_editing_the_same_line(&server);
    land_ok(&server, &first_branch, "measure from the jig face");
    let (status, _) = land(&server, &second_branch, "measure from the table edge");
    assert_eq!(status, 409, "the setup needs the second land to conflict");

    let resolved = "The first bracket is measured from the jig face, not the table edge.";
    put_body_ok(
        &server,
        "bracket-order",
        &version_on_branch,
        &bracket_order_body(resolved, SECOND_PARAGRAPH, THIRD_PARAGRAPH),
        "resolve the first bracket measurement",
        Some(&second_branch),
    );

    land_ok(
        &server,
        &second_branch,
        "resolve the first bracket measurement",
    );

    assert!(
        body(&server, "bracket-order").contains(resolved),
        "the resolved text must be what lands"
    );
    assert!(
        !open_branches(&server).contains(&second_branch),
        "the resolved branch must be gone once it has landed"
    );
}

/// Detects a delete that is merged as if nothing else had happened: a branch that
/// retires a memory another branch has meanwhile edited must be reported as a
/// conflict, because landing it would throw away an edit nobody reviewed.
#[test]
fn a_branch_that_deletes_a_memory_another_landed_branch_modified_conflicts() {
    let server = TestServer::start(store_files(), |_| {});
    let base = version(&document(&server, "bracket-order"));
    let deleting = open_branch(&server, "alpha/session-1");
    let editing = open_branch(&server, "alpha/session-2");

    let (status, answer) = server.api(
        "DELETE",
        &with_branch("/api/memories/bracket-order", Some(&deleting)),
        Some(&json!({
            "base_version": base,
            "author": AUTHOR,
            "message": "retire the bracket order note",
        })),
    );
    assert_eq!(
        status, 200,
        "the delete on the branch must land, got {answer}"
    );
    put_body_ok(
        &server,
        "bracket-order",
        &base,
        &bracket_order_body(
            "The first bracket is measured from the jig face.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "measure the first bracket from the jig face",
        Some(&editing),
    );
    land_ok(
        &server,
        &editing,
        "measure the first bracket from the jig face",
    );
    let head_before = head(&server);

    let (status, answer) = land(&server, &deleting, "retire the bracket order note");

    assert_eq!(status, 409, "the delete must be a conflict, got {answer}");
    let conflict = answer["conflicts"]
        .as_array()
        .and_then(|conflicts| {
            conflicts
                .iter()
                .find(|entry| entry["path"] == json!("memories/bracket-order.md"))
        })
        .unwrap_or_else(|| panic!("the conflict must name the file, got {answer}"));
    assert!(
        conflict["ours"]
            .as_str()
            .is_some_and(|text| text.contains("from the jig face")),
        "the conflict must carry the edit that landed, got {conflict}"
    );
    assert_eq!(
        conflict["theirs"],
        json!(null),
        "the branch deleted the file, so it has no text there, got {conflict}"
    );
    assert_eq!(
        head(&server),
        head_before,
        "a conflicting land must leave main where it was"
    );
    assert_eq!(
        document(&server, "bracket-order")["id"],
        json!("bracket-order"),
        "the memory must still be in the store"
    );
}

/// Detects a land that publishes a store the validator refuses: a rename made on
/// a branch and a new link to the old name landed on `main` merge without a
/// conflict, and the result is a link to nothing. Nothing may land.
#[test]
fn landing_a_branch_whose_merged_tree_fails_validation_is_refused_and_nothing_lands() {
    let server = TestServer::start(store_files(), |_| {});
    let branch = open_branch(&server, SESSION);
    let (status, renamed) = server.api(
        "POST",
        &with_branch("/api/memories/rocket-stages/rename", Some(&branch)),
        Some(&json!({
            "to": "rocket-stage-numbering",
            "author": AUTHOR,
            "message": "rename the stage numbering memory",
        })),
    );
    assert_eq!(
        status, 200,
        "the rename on the branch must land, got {renamed}"
    );

    let (status, added) = server.api(
        "POST",
        "/api/memories",
        Some(&json!({
            "id": "stage-audit",
            "description": "The stage audit checks the numbering of every stage",
            "kind": "knowledge",
            "scopes": ["rocketry"],
            "source": "user",
            "body": "# Stage audit\n\nThe audit reads [[rocket-stages]] first.\n",
            "author": AUTHOR,
            "message": "record the stage audit",
        })),
    );
    assert_eq!(status, 200, "the write on main must land, got {added}");
    let head_before = head(&server);

    let (status, answer) = land(&server, &branch, "rename the stage numbering memory");

    assert_eq!(
        status, 422,
        "a merged store the validator refuses must not land, got {answer}"
    );
    let errors = answer["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer must list the problems, got {answer}"));
    assert!(
        errors.iter().any(|error| {
            error["path"] == json!("memories/stage-audit.md")
                && error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("rocket-stages"))
        }),
        "the refusal must name the file whose link would resolve to nothing, got {answer}"
    );
    assert_eq!(
        head(&server),
        head_before,
        "nothing may land when the merged store is refused"
    );
    assert_eq!(
        document(&server, "rocket-stages")["id"],
        json!("rocket-stages"),
        "the memory the branch renamed must still be on main"
    );
    assert!(
        open_branches(&server).contains(&branch),
        "the branch must be left as it was"
    );
}

// ---------------------------------------------------------------------------
// The single operations
// ---------------------------------------------------------------------------

/// Detects a snippet replacement that rewrites the rest of the memory: only the
/// snippet may change, and the change must reach `main` when the branch lands.
#[test]
fn replace_text_on_a_branch_changes_only_the_snippet_and_lands_with_the_branch() {
    let server = TestServer::start(store_files(), |_| {});
    let before = body(&server, "bench-power");
    let branch = open_branch(&server, SESSION);

    let (status, answer) = server.api(
        "POST",
        &with_branch("/api/memories/bench-power/replace-text", Some(&branch)),
        Some(&json!({
            "old_string": "at the wall",
            "new_string": "at the wall switch",
            "author": AUTHOR,
            "message": "say which switch cuts the bench supply",
        })),
    );
    assert_eq!(status, 200, "the replacement must be made, got {answer}");
    assert_eq!(
        body(&server, "bench-power"),
        before,
        "a replacement on a branch must not change main"
    );

    land_ok(&server, &branch, "say which switch cuts the bench supply");

    let after = body(&server, "bench-power");
    assert_eq!(
        after,
        before.replace("at the wall", "at the wall switch"),
        "only the snippet may have changed"
    );
}

/// Detects a replacement that picks one of several matches, and one that reports
/// success having changed nothing: a snippet that appears twice has to be
/// refused, because guessing which one the caller meant changes text nobody asked
/// to change, and a snippet that is not there at all has to be refused, because a
/// caller that mistyped it would otherwise believe the edit was made.
#[test]
fn replace_text_refuses_a_snippet_that_is_missing_or_ambiguous_and_changes_nothing() {
    let server = TestServer::start(store_files(), |_| {});
    let base = version(&document(&server, "bracket-order"));
    put_body_ok(
        &server,
        "bracket-order",
        &base,
        &bracket_order_body(
            "The bracket is measured twice.",
            "The bracket is measured twice.",
            THIRD_PARAGRAPH,
        ),
        "say that the bracket is measured twice",
        None,
    );

    let (status, answer) = server.api(
        "POST",
        "/api/memories/bracket-order/replace-text",
        Some(&json!({
            "old_string": "measured by the machine",
            "new_string": "measured by hand",
            "author": AUTHOR,
            "message": "say how the bracket is measured",
        })),
    );
    assert_eq!(
        status, 422,
        "a snippet that is not in the body must be refused, got {answer}"
    );
    assert!(
        !body(&server, "bracket-order").contains("measured by hand"),
        "a refused replacement must change nothing"
    );

    let request = json!({
        "old_string": "measured twice",
        "new_string": "measured three times",
        "author": AUTHOR,
        "message": "say how often the bracket is measured",
    });
    let (status, answer) = server.api(
        "POST",
        "/api/memories/bracket-order/replace-text",
        Some(&request),
    );

    assert_eq!(
        status, 422,
        "a snippet that appears twice must be refused, got {answer}"
    );
    assert!(
        body(&server, "bracket-order")
            .matches("measured twice")
            .count()
            == 2,
        "a refused replacement must change nothing"
    );

    let mut asked = request.clone();
    asked["replace_all"] = json!(true);
    let (status, answer) = server.api(
        "POST",
        "/api/memories/bracket-order/replace-text",
        Some(&asked),
    );
    assert_eq!(
        status, 200,
        "replace_all must replace every match, got {answer}"
    );
    assert_eq!(
        body(&server, "bracket-order")
            .matches("measured three times")
            .count(),
        2,
        "every match must have been replaced"
    );
}

/// Detects a rename that leaves a link behind: the file moves and every
/// `[[link]]` to it is rewritten in the same commit, or the store the rename
/// lands has links that resolve to nothing.
#[test]
fn renaming_a_memory_rewrites_every_linker_and_the_landed_tree_has_no_link_to_the_old_name() {
    let server = TestServer::start(store_files(), |_| {});
    let before = commits_from_head(&server);
    let branch = open_branch(&server, SESSION);

    let (status, answer) = server.api(
        "POST",
        &with_branch("/api/memories/rocket-stages/rename", Some(&branch)),
        Some(&json!({
            "to": "rocket-stage-numbering",
            "author": AUTHOR,
            "message": "rename the stage numbering memory",
        })),
    );
    assert_eq!(status, 200, "the rename must be made, got {answer}");

    land_ok(&server, &branch, "rename the stage numbering memory");

    let after = commits_from_head(&server);
    assert_eq!(
        after.len(),
        before.len() + 1,
        "the rename and every rewrite must land as one commit"
    );
    let (status, gone) = server.api("GET", "/api/memories/rocket-stages", None);
    assert_eq!(status, 404, "the old id must name nothing, got {gone}");
    assert_eq!(
        document(&server, "rocket-stage-numbering")["name"],
        json!("rocket-stage-numbering"),
        "the moved file must declare the name its new id requires"
    );
    for linker in ["reading-list", "stage-report"] {
        let text = body(&server, linker);
        assert!(
            text.contains("[[rocket-stage-numbering]]"),
            "{linker} must link to the new name, got {text:?}"
        );
        assert!(
            !text.contains("[[rocket-stages]]"),
            "{linker} must not link to the old name, got {text:?}"
        );
    }
    let (status, review) = server.api("GET", "/api/review", None);
    assert_eq!(status, 200, "the review must be readable, got {review}");
    assert_eq!(
        review["errors"],
        json!([]),
        "the landed store must have no unresolved links, got {review}"
    );
}

/// Detects a field write that rewrites the body: setting a memory's kind or
/// description must leave every byte after the frontmatter alone, or an edit of
/// one field would reformat text a person wrote.
#[test]
fn set_fields_changes_the_field_and_leaves_the_body_byte_identical() {
    let server = TestServer::start(store_files(), |_| {});
    let before = body(&server, "reading-list");
    let branch = open_branch(&server, SESSION);

    let (status, answer) = server.api(
        "POST",
        &with_branch("/api/memories/reading-list/fields", Some(&branch)),
        Some(&json!({
            "kind": "critical",
            "description": "The workshop references are on paper in the binder only",
            "author": AUTHOR,
            "message": "make the reading list a critical memory",
        })),
    );
    assert_eq!(status, 200, "the field write must be made, got {answer}");

    land_ok(&server, &branch, "make the reading list a critical memory");

    let landed = document(&server, "reading-list");
    assert_eq!(
        landed["body"].as_str().unwrap_or_default(),
        before,
        "the body must be byte for byte what it was"
    );
    assert_eq!(
        landed["kind"],
        json!("critical"),
        "the field that was set must have changed, got {landed}"
    );
    assert_eq!(
        landed["description"],
        json!("The workshop references are on paper in the binder only"),
        "the description that was set must have changed, got {landed}"
    );
}

/// Detects a version check made against `main` instead of the branch: the second
/// write of a transaction builds on the first, so its `base_version` is the one
/// the branch holds, and a check against `main` would refuse every chain of two
/// writes as soon as anything landed on `main`.
#[test]
fn a_base_version_on_a_branch_write_is_checked_against_the_branch_not_main() {
    let server = TestServer::start(store_files(), |_| {});
    let branch = open_branch(&server, SESSION);
    let version_on_branch = put_body_ok(
        &server,
        "bracket-order",
        &version(&document(&server, "bracket-order")),
        &bracket_order_body(
            "Written once on the branch.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "write the bracket order once",
        Some(&branch),
    );
    put_body_ok(
        &server,
        "bracket-order",
        &version(&document(&server, "bracket-order")),
        &bracket_order_body(FIRST_PARAGRAPH, "Written on main.", THIRD_PARAGRAPH),
        "write the bracket order on main",
        None,
    );
    let version_on_main = version(&document(&server, "bracket-order"));
    assert_ne!(
        version_on_main, version_on_branch,
        "the setup needs the branch and main to hold different versions"
    );

    let (stale_status, stale) = put_body(
        &server,
        "bracket-order",
        &version_on_main,
        &bracket_order_body(
            "Written from main's version.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "write the bracket order from main's version",
        Some(&branch),
    );
    let (fresh_status, fresh) = put_body(
        &server,
        "bracket-order",
        &version_on_branch,
        &bracket_order_body(
            "Written twice on the branch.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "write the bracket order twice",
        Some(&branch),
    );

    assert_eq!(
        stale_status, 409,
        "main's version is not a version of the branch, got {stale}"
    );
    assert_eq!(
        fresh_status, 200,
        "the version the branch holds must be accepted, got {fresh}"
    );
}

// ---------------------------------------------------------------------------
// Housekeeping
// ---------------------------------------------------------------------------

/// Detects a branch list that cannot say how far a branch is from `main`: an
/// agent deciding whether to land needs to know what it is ahead by and what has
/// happened on `main` since, and the owner is how a person finds whose it is.
#[test]
fn the_branch_list_reports_the_owner_and_how_far_the_branch_is_ahead_and_behind() {
    let server = TestServer::start(store_files(), |_| {});
    let branch = open_branch(&server, SESSION);
    let first = put_body_ok(
        &server,
        "bracket-order",
        &version(&document(&server, "bracket-order")),
        &bracket_order_body(
            "The first write on the branch.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "write the bracket order once",
        Some(&branch),
    );
    put_body_ok(
        &server,
        "bracket-order",
        &first,
        &bracket_order_body(
            "The second write on the branch.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "write the bracket order twice",
        Some(&branch),
    );
    put_body_ok(
        &server,
        "reading-list",
        &version(&document(&server, "reading-list")),
        "# Where the workshop references live\n\nEdited straight on main.\n",
        "rewrite the reading list",
        None,
    );

    let row = branch_row(&server, &branch);

    assert_eq!(
        row["owner"],
        json!(SESSION),
        "the list must say which session opened the branch, got {row}"
    );
    assert_eq!(
        row["ahead"],
        json!(2),
        "the branch is two commits ahead of main, got {row}"
    );
    assert_eq!(
        row["behind"],
        json!(1),
        "the branch is one commit behind main, got {row}"
    );
}

/// Detects an abandon that lands what the branch held, or that leaves the branch
/// behind: throwing a transaction away must change nothing on `main` and must
/// leave nothing to land later.
#[test]
fn abandoning_a_branch_removes_it_and_leaves_main_unchanged() {
    let server = TestServer::start(store_files(), |_| {});
    let branch = open_branch(&server, SESSION);
    put_body_ok(
        &server,
        "bench-power",
        &version(&document(&server, "bench-power")),
        &format!("# Cut bench power before rewiring\n\n{NEW_LINE}\n"),
        "note where the binder lives",
        Some(&branch),
    );
    let head_before = head(&server);

    let (status, answer) = server.api("DELETE", &format!("/api/branches/{branch}"), None);

    assert_eq!(
        status, 200,
        "abandoning a branch must succeed, got {answer}"
    );
    assert!(
        !open_branches(&server).contains(&branch),
        "the abandoned branch must be gone"
    );
    assert_eq!(
        head(&server),
        head_before,
        "abandoning a branch must not move main"
    );
    assert!(
        !body(&server, "bench-power").contains(NEW_LINE),
        "nothing the branch held may reach main"
    );
    let (status, landed) = land(&server, &branch, "land what was abandoned");
    assert_eq!(
        status, 404,
        "an abandoned branch must not be landable, got {landed}"
    );
}

/// Detects branch state kept anywhere but in the repository: a restart must
/// report exactly the same open transactions, owners and counts, because a branch
/// is a git ref and its commits are everything known about it. State in a file
/// the server writes would be lost, or would disagree with the refs.
#[test]
fn a_restart_reports_the_same_open_branches_owners_and_counts() {
    let mut server = TestServer::start(store_files(), |_| {});
    let first = open_branch(&server, "alpha/session-1");
    let second = open_branch(&server, "alpha/session-2");
    put_body_ok(
        &server,
        "bench-power",
        &version(&document(&server, "bench-power")),
        &format!("# Cut bench power before rewiring\n\n{NEW_LINE}\n"),
        "note where the binder lives",
        Some(&first),
    );
    let before = branch_list(&server);
    assert_eq!(
        before.len(),
        2,
        "the setup needs both branches to be listed, got {before:?}"
    );

    server.restart();

    assert_eq!(
        branch_list(&server),
        before,
        "the restart must report exactly what was open before it"
    );
    assert_eq!(
        branch_row(&server, &first)["owner"],
        json!("alpha/session-1"),
        "the owner of a branch is the session that opened it"
    );
    assert_eq!(
        branch_row(&server, &first)["ahead"],
        json!(1),
        "the write on the branch must still be there"
    );
    assert_eq!(
        branch_row(&server, &second)["ahead"],
        json!(0),
        "a branch nobody wrote on holds no writes"
    );

    land_ok(&server, &first, "note where the binder lives");
    assert!(
        body(&server, "bench-power").contains(NEW_LINE),
        "a branch opened before the restart must still be landable"
    );
}

/// Detects a diff that does not say what a branch would do: an agent about to
/// land needs the file, whether it is added, changed or retired, and the change
/// itself.
#[test]
fn the_branch_diff_names_each_file_with_its_status_and_its_diff() {
    let server = TestServer::start(store_files(), |_| {});
    let branch = open_branch(&server, SESSION);
    put_body_ok(
        &server,
        "bracket-order",
        &version(&document(&server, "bracket-order")),
        &bracket_order_body(
            "The first bracket moved.",
            SECOND_PARAGRAPH,
            THIRD_PARAGRAPH,
        ),
        "move the first bracket",
        Some(&branch),
    );
    let (status, created) = server.api(
        "POST",
        &with_branch("/api/memories", Some(&branch)),
        Some(&json!({
            "id": "bracket-jig",
            "description": "The bracket jig lives in the second drawer",
            "kind": "knowledge",
            "scopes": ["global"],
            "source": "user",
            "body": "# The bracket jig\n\nThe jig lives in the second drawer.\n",
            "author": AUTHOR,
            "message": "record where the bracket jig lives",
        })),
    );
    assert_eq!(
        status, 200,
        "the create on the branch must land, got {created}"
    );

    let (status, diff) = server.api("GET", &format!("/api/branches/{branch}/diff"), None);

    assert_eq!(status, 200, "the diff must be readable, got {diff}");
    assert_eq!(
        diff["ahead"],
        json!(2),
        "the diff must count the commits, got {diff}"
    );
    assert_eq!(
        diff["behind"],
        json!(0),
        "nothing landed on main, got {diff}"
    );
    let files = diff["files"].as_array().expect("the diff lists files");
    let modified = files
        .iter()
        .find(|file| file["path"] == json!("memories/bracket-order.md"))
        .unwrap_or_else(|| panic!("the edited file must be listed, got {diff}"));
    assert_eq!(
        modified["status"],
        json!("modified"),
        "an edited file is modified, got {modified}"
    );
    assert!(
        modified["diff"]
            .as_str()
            .is_some_and(|text| text.contains("+The first bracket moved.")),
        "the diff must show the line the branch added, got {modified}"
    );
    let added = files
        .iter()
        .find(|file| file["path"] == json!("memories/bracket-jig.md"))
        .unwrap_or_else(|| panic!("the new file must be listed, got {diff}"));
    assert_eq!(
        added["status"],
        json!("added"),
        "a new file is added, got {added}"
    );
}

/// Detects a branch name a client can make up: a write or a land against a name
/// no branch has must be refused rather than reaching `main`, and a name outside
/// the transaction prefix must not name a branch at all.
#[test]
fn a_name_that_is_no_open_branch_is_refused_and_reaches_main() {
    let server = TestServer::start(store_files(), |_| {});
    let head_before = head(&server);

    let (missing_status, missing) = put_body(
        &server,
        "bench-power",
        &version(&document(&server, "bench-power")),
        &format!("# Cut bench power before rewiring\n\n{NEW_LINE}\n"),
        "note where the binder lives",
        Some("tx/no-such-branch-1"),
    );
    let (bad_status, bad) = put_body(
        &server,
        "bench-power",
        &version(&document(&server, "bench-power")),
        &format!("# Cut bench power before rewiring\n\n{NEW_LINE}\n"),
        "note where the binder lives",
        Some("main"),
    );

    assert_eq!(
        missing_status, 404,
        "a branch that is not open names nothing, got {missing}"
    );
    assert_eq!(
        bad_status, 400,
        "a name outside the transaction prefix is not a branch, got {bad}"
    );
    assert_eq!(
        head(&server),
        head_before,
        "neither write may have reached main"
    );
}

// ---------------------------------------------------------------------------
// Several landers at once
// ---------------------------------------------------------------------------

/// Detects a land that loses a commit under concurrency: eight branches touching
/// eight different memories must all land, one commit each, in one line of
/// history. A land that read `main` and then wrote without comparing would leave
/// a transaction nowhere.
#[test]
fn eight_concurrent_lands_of_branches_touching_distinct_files_all_succeed() {
    let landers = 8;
    let mut files = store_files();
    files.extend((0..landers).map(|index| {
        memory_file(
            &format!("note-{index:02}"),
            "knowledge",
            &["global"],
            &format!("# note-{index:02}\n\nthe first text of note {index}\n"),
        )
    }));
    let server = TestServer::start(files, |_| {});
    let before = commits_from_head(&server);

    let branches: Vec<(usize, String)> = (0..landers)
        .map(|index| {
            let branch = open_branch(&server, &format!("alpha/session-{index}"));
            let id = format!("note-{index:02}");
            put_body_ok(
                &server,
                &id,
                &version(&document(&server, &id)),
                &format!("# {id}\n\nthe text written by lander {index}\n"),
                &format!("rewrite note {index:02}"),
                Some(&branch),
            );
            (index, branch)
        })
        .collect();

    let url = server.url();
    let statuses: Vec<(usize, u16, Value)> = std::thread::scope(|scope| {
        let handles: Vec<_> = branches
            .iter()
            .map(|(index, branch)| {
                let url = url.clone();
                let index = *index;
                let branch = branch.clone();
                scope.spawn(move || {
                    let (status, answer) = api_request(
                        &url,
                        "POST",
                        &format!("/api/branches/{branch}/land"),
                        Some(&json!({
                            "message": format!("land note {index:02}"),
                            "author": AUTHOR,
                        })),
                    );
                    (index, status, answer)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a lander finished"))
            .collect()
    });

    for (index, status, answer) in &statuses {
        assert_eq!(
            *status, 200,
            "the land of note {index:02} must succeed, got {answer}"
        );
    }
    let after = commits_from_head(&server);
    assert_eq!(
        after.len(),
        before.len() + landers,
        "each land must add exactly one commit"
    );
    for (oid, _, parents) in &after {
        assert!(
            *parents <= 1,
            "the history must stay linear; commit {oid} has {parents} parents"
        );
    }
    for index in 0..landers {
        let id = format!("note-{index:02}");
        assert!(
            body(&server, &id).contains(&format!("the text written by lander {index}")),
            "note {index:02} must hold what its lander wrote"
        );
    }
    assert!(
        open_branches(&server).is_empty(),
        "every landed branch must be gone"
    );
}

/// Detects a rename that takes an id another memory has, or that moves a memory
/// into a session's silo while something outside the session links to it: both
/// would land a store the validator calls invalid, one by having two memories at
/// one id and the other by letting a link reach into a silo.
#[test]
fn renaming_onto_an_id_that_exists_or_into_a_session_silo_is_refused() {
    let server = TestServer::start(store_files(), |_| {});
    let head_before = head(&server);

    let (taken_status, taken) = server.api(
        "POST",
        "/api/memories/rocket-stages/rename",
        Some(&json!({
            "to": "reading-list",
            "author": AUTHOR,
            "message": "rename the stage numbering memory",
        })),
    );
    let (silo_status, silo) = server.api(
        "POST",
        "/api/memories/rocket-stages/rename",
        Some(&json!({
            "to": "sessions/alpha/session-1/stages",
            "author": AUTHOR,
            "message": "move the stage numbering into a session",
        })),
    );

    assert_eq!(
        taken_status, 422,
        "an id another memory has must be refused, got {taken}"
    );
    assert_eq!(
        silo_status, 422,
        "a move into a silo something outside it links to must be refused, got {silo}"
    );
    assert!(
        silo["errors"].as_array().is_some_and(|errors| errors
            .iter()
            .any(|error| { error["path"] == json!("memories/reading-list.md") })),
        "the refusal must name the memory whose link would cross into the silo, got {silo}"
    );
    assert_eq!(
        head(&server),
        head_before,
        "neither refused rename may have committed anything"
    );
    assert_eq!(
        document(&server, "rocket-stages")["id"],
        json!("rocket-stages"),
        "the memory must still be where it was"
    );
}

// ---------------------------------------------------------------------------
// The rules a branch answers only at landing
// ---------------------------------------------------------------------------

/// Create one memory, on `branch` when given.
fn create_memory(
    server: &TestServer,
    id: &str,
    scopes: &[&str],
    body: &str,
    branch: Option<&str>,
) -> (u16, Value) {
    server.api(
        "POST",
        &with_branch("/api/memories", branch),
        Some(&json!({
            "id": id,
            "description": format!("the index entry of {id}"),
            "kind": "knowledge",
            "scopes": scopes,
            "source": "user",
            "body": body,
            "author": AUTHOR,
            "message": format!("record {id}"),
        })),
    )
}

/// The problems a refusal lists, as pairs of file and message.
fn problems(answer: &Value) -> Vec<(String, String)> {
    answer["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("a refusal must list the problems, got {answer}"))
        .iter()
        .map(|error| {
            (
                error["path"].as_str().unwrap_or_default().to_string(),
                error["message"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Detects a branch that checks each write against the branch as it stands so
/// far: two memories that link to each other have no write order in which every
/// intermediate revision resolves both links, so one of the two writes would be
/// refused and a set of memories that cite each other could never be imported.
/// Both orders are run, because a branch that only accepted the second-written
/// half would pass a test that fixed the order.
#[test]
fn two_memories_that_link_to_each_other_are_written_on_a_branch_in_either_order_and_land() {
    let north = (
        "north-shelf",
        "# North shelf\n\nThe north shelf faces [[south-shelf]].\n",
    );
    let south = (
        "south-shelf",
        "# South shelf\n\nThe south shelf faces [[north-shelf]].\n",
    );

    for (first, second) in [(north, south), (south, north)] {
        let server = TestServer::start(store_files(), |_| {});
        let branch = open_branch(&server, SESSION);

        let (first_status, first_answer) =
            create_memory(&server, first.0, &["global"], first.1, Some(&branch));
        let (second_status, second_answer) =
            create_memory(&server, second.0, &["global"], second.1, Some(&branch));

        assert_eq!(
            first_status, 200,
            "writing {} first on a branch must be taken, got {first_answer}",
            first.0
        );
        assert_eq!(
            second_status, 200,
            "writing {} second on a branch must be taken, got {second_answer}",
            second.0
        );

        land_ok(&server, &branch, "record the two shelves");

        assert_eq!(
            body(&server, first.0),
            first.1,
            "the memory written first must reach main with its link"
        );
        assert_eq!(
            body(&server, second.0),
            second.1,
            "the memory written second must reach main with its link"
        );
    }
}

/// Detects a branch that never checks the links it deferred: a link to a name
/// nobody writes has to stop the land, or the deferral would let an invalid
/// store onto main.
#[test]
fn a_branch_link_to_a_name_nobody_writes_is_taken_but_stops_the_land() {
    let server = TestServer::start(store_files(), |_| {});
    let branch = open_branch(&server, SESSION);
    let head_before = head(&server);

    let (status, written) = create_memory(
        &server,
        "north-shelf",
        &["global"],
        "# North shelf\n\nThe north shelf faces [[no-such-shelf]].\n",
        Some(&branch),
    );
    assert_eq!(
        status, 200,
        "a link a branch has not written yet must not stop the write, got {written}"
    );

    let (land_status, refusal) = land(&server, &branch, "record the north shelf");

    assert_eq!(
        land_status, 422,
        "a merged store with a link to nothing must not land, got {refusal}"
    );
    assert!(
        problems(&refusal).iter().any(|(path, message)| {
            path == "memories/north-shelf.md" && message.contains("no-such-shelf")
        }),
        "the refusal must name the file and the target it links to, got {refusal}"
    );
    assert_eq!(
        head(&server),
        head_before,
        "nothing may land when the merged store is refused"
    );
}

/// Detects a branch that defers the scope check and then forgets it: a memory
/// filed under a scope that has no file is delivered to nobody, so the land has
/// to refuse it just as a write to main does.
#[test]
fn a_branch_memory_in_a_scope_that_has_no_file_is_taken_but_stops_the_land() {
    let server = TestServer::start(store_files(), |_| {});
    let branch = open_branch(&server, SESSION);
    let head_before = head(&server);

    let (status, written) = create_memory(
        &server,
        "north-shelf",
        &["no-such-scope"],
        "# North shelf\n\nThe north shelf is by the door.\n",
        Some(&branch),
    );
    assert_eq!(
        status, 200,
        "a scope the branch could still create must not stop the write, got {written}"
    );

    let (land_status, refusal) = land(&server, &branch, "record the north shelf");

    assert_eq!(
        land_status, 422,
        "a merged store naming a scope that has no file must not land, got {refusal}"
    );
    assert!(
        problems(&refusal).iter().any(|(path, message)| {
            path == "memories/north-shelf.md" && message.contains("no-such-scope")
        }),
        "the refusal must name the file and the scope it names, got {refusal}"
    );
    assert_eq!(
        head(&server),
        head_before,
        "nothing may land when the merged store is refused"
    );
}

/// Detects the deferral reaching writes that are not on a branch: a commit on
/// main is a revision every session reads, so a link to nothing has to be
/// refused as it is written, not at some later land that will never come.
#[test]
fn the_same_dangling_link_written_straight_to_main_is_still_refused_at_write_time() {
    let server = TestServer::start(store_files(), |_| {});
    let head_before = head(&server);

    let (status, refusal) = create_memory(
        &server,
        "north-shelf",
        &["global"],
        "# North shelf\n\nThe north shelf faces [[no-such-shelf]].\n",
        None,
    );

    assert_eq!(
        status, 422,
        "a write to main that links to nothing must be refused, got {refusal}"
    );
    assert!(
        problems(&refusal).iter().any(|(path, message)| {
            path == "memories/north-shelf.md"
                && message.contains("no-such-shelf")
                && message.contains("not a memory in this store")
        }),
        "the refusal must say which link of which file resolves to nothing, got {refusal}"
    );
    assert_eq!(
        head(&server),
        head_before,
        "a refused write must commit nothing"
    );
}
