//! What the memory system's own traffic does to the session it runs in.
//!
//! Every MCP call is three events: the `PreToolUse` that may stop it, the call,
//! and the `PostToolUse` carrying what it answered. The inputs and answers of
//! this server's own tools are full of scope ids, memory ids and memory bodies,
//! so a session that asks the memory system a question would, without the
//! exemption, turn scopes on from the question and from the answer to it — and
//! would undo with the answer what the call itself had just done. None of that
//! is visible in one event: it takes the call and the event after it. That the
//! exemption keeps a tool's texts out of `EventPlan::texts` is `hook::events`'s
//! own unit test; what these assert is where the session ends up.

mod common;
mod scenario;

use scenario::{Store, World, bash, read};
use serde_json::json;

/// The machine these scenarios run on.
const ALPHA: &str = "alpha";

/// A directory no trigger of the example store fires on, so that a scenario's
/// scopes are the ones it asked for.
const NEUTRAL: &str = "/home/dev/notes";

/// Detects the memory system undoing its own work: the call that turns scopes
/// off names them in its input, and its answer lists them again among the scopes
/// on offer — and the `PostToolUse` carrying that answer arrives after the scopes
/// are off. Matching either text would turn them straight back on, and the model
/// would be told it had stepped out of a scope and be working in it again by the
/// next event, with no way out of it at all.
///
/// The prompt names a widget, so `widgets` and the `rocketry` it implies are on.
/// Both are turned off in one call, so both are on offer in its answer, and
/// `rocketry`'s trigger is on every field, the result of a tool call included.
/// After the whole wrapped call both are off and stay off, and the next call —
/// whose own input names nothing of the store — goes ahead carrying nothing,
/// because there is nothing left that this session does not hold.
#[test]
fn a_scope_turned_off_through_mcp_stays_off_through_the_answer_that_names_it() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    session
        .prompt("rename the widget brackets")
        .assert(|answer| assert!(answer.delivers_full("widget-naming")));
    assert!(
        session.active_scopes().contains("widgets") && session.active_scopes().contains("rocketry"),
        "the prompt turned the scope and what it implies on, got {:?}",
        session.active_scopes()
    );

    session
        .mcp()
        .session_scope_off(["widgets", "rocketry"])
        .expect_ok();

    let after = session.active_scopes();
    assert!(
        !after.contains("widgets") && !after.contains("rocketry"),
        "neither the call naming the scopes nor the answer listing them may turn them on \
         again, got {after:?}"
    );
    session
        .tool("Bash", bash("cargo test -p bench"))
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "nothing is due, so the call after the scope went off is not stopped"
            );
            assert!(
                answer.delivers_nothing(),
                "the scope is off and its memory was withdrawn at the call's own PostToolUse"
            );
        });
}

/// Detects an exemption applied to the tool's input but not to its answer, and
/// one applied to every tool rather than to this server's own: the first would
/// let a session that read one memory be pulled into every scope that memory's
/// text names, the second would stop the work itself from activating anything.
///
/// The same bytes travel twice. Read out of `memory_get`, they are the memory
/// system answering a question about itself, and no scope comes on. Handed back
/// as the result of a `Bash` call, they are what the session's work returned, and
/// `rocketry`, whose trigger is on every field and matches the text of a rocket,
/// comes on with its memory. The call is made without a session key, so the
/// fetch leaves no mark on the delivery record and the index line that arrives
/// afterwards is owed to the scope alone.
#[test]
fn a_memory_get_answer_naming_every_scope_activates_nothing_while_the_same_text_from_bash_does() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    let before = session.active_scopes();
    assert!(
        !before.contains("rocketry"),
        "the session starts outside rocketry, got {before:?}"
    );

    let fetched = session
        .mcp()
        .call("memory_get", json!({ "id": "rocket-stages" }))
        .expect_ok();
    let answered = serde_json::to_string(fetched.json()).expect("the tool's answer is JSON");
    assert!(
        answered.contains("rocket"),
        "the text this scenario is about has to name a rocket, got {answered}"
    );

    assert_eq!(
        session.active_scopes(),
        before,
        "asking the memory system about a memory must move no scope"
    );

    session
        .tool("Bash", bash("cat /home/dev/notes/log.txt"))
        .result(json!({ "stdout": answered }))
        .assert(|answer| {
            assert!(
                answer.delivers_index("rocket-stages"),
                "the same text from the work turns rocketry on, and its memory is due"
            );
        });
    assert!(
        session.active_scopes().contains("rocketry"),
        "the scope the tool result named is on, got {:?}",
        session.active_scopes()
    );
}

/// Detects a `trigger_exempt_tools` setting that is read but never reaches the
/// running server's event handling: a store that exempted a noisy tool would go
/// on being pulled into scopes by everything that tool printed, and nothing in
/// any single answer would say so.
///
/// One result, two tools. Through the tool the store exempts, the text is not
/// matched at all and no scope moves. Through a tool the store says nothing
/// about, the same text turns `rocketry` on and its memory arrives. Neither
/// call's input names a rocket, so the result is the only thing that could have
/// done it.
#[test]
fn a_tool_the_store_exempts_activates_nothing_from_its_result() {
    let world = World::new()
        .store(Store::example())
        .settings(|settings| {
            settings.trigger_exempt_tools(["Read"]);
        })
        .build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    let result = json!({ "stdout": "the rocket ships on friday" });

    session
        .tool("Read", read("/home/dev/notes/log.txt"))
        .result(result.clone())
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the store exempts this tool, so its result is matched against nothing"
            );
        });
    assert!(
        !session.active_scopes().contains("rocketry"),
        "no scope may come on from an exempt tool's result, got {:?}",
        session.active_scopes()
    );

    session
        .tool("Grep", json!({ "pattern": "pub fn" }))
        .result(result)
        .assert(|answer| {
            assert!(
                answer.delivers_index("rocket-stages"),
                "the same text from a tool the store says nothing about turns rocketry on"
            );
        });
    assert!(
        session.active_scopes().contains("rocketry"),
        "the exemption is for the tools the store named and no others, got {:?}",
        session.active_scopes()
    );
}

/// Detects a read of the session's scopes that is not a read: a context created
/// or touched by the listing would answer for a session that never asked
/// anything, and a listing that marked its own answer as delivered would leave
/// the session owed nothing it had not seen.
///
/// The session is in the three scopes every context has by construction and the
/// store's three file-backed scopes are all on offer, which is what the model
/// reads before choosing one. Nothing moves: the scopes after the call are the
/// scopes it reported, and the next prompt is delivered nothing, because
/// everything due arrived at the start and the listing changed neither the store
/// nor the record.
#[test]
fn a_session_scope_list_call_changes_nothing_and_answers_the_scopes() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    let before = session.active_scopes();

    let listed = session.mcp().session_scopes().expect_ok();
    let reported = |key: &str| -> std::collections::BTreeSet<String> {
        listed.json()[key]
            .as_array()
            .unwrap_or_else(|| panic!("the answer lists {key}, got {}", listed.json()))
            .iter()
            .map(|id| id.as_str().expect("a scope id is a string").to_string())
            .collect()
    };

    assert_eq!(
        reported("active"),
        before,
        "the tool must answer the scopes the server has this context working in"
    );
    assert_eq!(
        reported("available"),
        ["rocketry", "widgets", "workshop"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "every file-backed scope this session is not in is on offer"
    );
    assert_eq!(
        session.active_scopes(),
        before,
        "a listing changes no scope of the session that asked for it"
    );

    session
        .prompt("carry on with the bench supply")
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "everything due arrived at the start, and asking for the scopes moved nothing"
            );
        });
}
