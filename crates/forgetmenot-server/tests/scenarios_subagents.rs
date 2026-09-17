//! What a subagent is, seen from outside a single event: where its scopes come
//! from, that its delivery record is its own, and where its writes land.
//!
//! A subagent is the one context nobody starts: Claude Code opens it, the server
//! creates it from its parent, and from then on it is a context like any other,
//! with its own record, its own events and its own writes. None of that is
//! visible in one event, so all of it lives here. What one `SubagentStart`
//! contributes — that it acts on the child's key and matches the task against
//! the user-message triggers — is `hook::events`'s own unit test and is not
//! asserted again.

mod common;
mod scenario;

use scenario::{Store, World, read};
use serde_json::json;

/// The machine these scenarios run on, which is the machine the example store's
/// `workshop` trigger and its session memories are written for.
const ALPHA: &str = "alpha";

/// A directory no trigger of the example store fires on, so that a scenario's
/// scopes are the ones it asked for.
const NEUTRAL: &str = "/home/dev/notes";

/// The three scopes every context is in by construction: the store's `global`,
/// the machine's, and the session's own.
fn implicit_scopes(machine: &str, session_id: &str) -> std::collections::BTreeSet<String> {
    [
        "global".to_string(),
        format!("machine:{machine}"),
        format!("session:{machine}/{session_id}"),
    ]
    .into_iter()
    .collect()
}

/// Detects a child created from the implicit scopes while the store asks for
/// inheritance, and a child created with a copy of its parent's delivery record.
///
/// The parent's prompt names a widget, so it works in `widgets` and in the
/// `rocketry` that `widgets` implies. The child is created from that set, so the
/// same memories are due to it; and it holds nothing yet, so everything due
/// arrives at its start, `bench-power` included, which the parent was given long
/// before the child existed. A child that copied the record would be sent none
/// of it and would act on a critical rule it was never shown.
///
/// The scopes are read back from the server rather than inferred from the
/// deliveries: a child could be given the right texts once by matching the
/// task and still not be working in the scope afterwards.
#[test]
fn a_subagent_inherits_the_parents_scopes_and_not_its_record() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    session
        .prompt("rename the widget brackets")
        .assert(|answer| {
            assert!(
                answer.delivers_full("widget-naming"),
                "the prompt names a widget, so widgets comes on for the parent"
            );
        });

    let child = session.subagent("general-purpose", "list the files");

    assert!(
        child.model_saw_full("widget-naming"),
        "the child copied the scopes, so the widget rule is due to it"
    );
    assert!(
        child.model_saw_full("bench-power"),
        "the child's record is its own, so the global rule the parent already holds \
         arrives here too"
    );
    let scopes = child.active_scopes();
    assert!(
        scopes.contains("widgets") && scopes.contains("rocketry"),
        "the child works in the scopes its parent was in, implications included, got {scopes:?}"
    );
}

/// Detects a store setting that reaches the operations layer but not the place a
/// subagent's context is created: a session that asked for children to start
/// clean would go on handing them everything it had turned on.
///
/// With `subagents_inherit_scopes` off the child starts from the three scopes
/// every context has by construction, so the parent's `widgets` is not among
/// them and `widget-naming` is not due. What is in those three is still due:
/// `bench-power` is in `global`, and the session notes are in the session scope
/// the child shares with its parent, so they arrive as an index line. The scopes
/// are the assertion that pins this; the deliveries only show it from the
/// model's side.
#[test]
fn a_subagent_starts_from_the_implicit_scopes_when_the_store_says_so() {
    let world = World::new()
        .store(Store::example())
        .settings(|settings| {
            settings.subagents_inherit_scopes(false);
        })
        .build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    session
        .prompt("rename the widget brackets")
        .assert(|answer| assert!(answer.delivers_full("widget-naming")));

    let child = session.subagent("general-purpose", "list the files");

    assert!(
        child.model_saw_full("bench-power"),
        "the global rule is in a scope every context has, so it is due to the child"
    );
    assert!(
        !child.model_saw_full("widget-naming"),
        "the store asked for children to start clean, so the parent's widgets is not on here"
    );
    assert_eq!(
        child.active_scopes(),
        implicit_scopes(ALPHA, session.session_id()),
        "a child that starts clean works in the three scopes every context has"
    );
    assert!(
        session.active_scopes().contains("widgets"),
        "what the child starts with says nothing about the parent, got {:?}",
        session.active_scopes()
    );
}

/// Detects a task that is matched into the parent's context instead of the
/// child's: the parent would end up working in scopes nobody in it asked for,
/// and would be delivered their memories for the rest of the session.
///
/// The parent is in no project scope: it started in a neutral directory and has
/// said nothing about widgets. The task names one, and a task is matched against
/// the user-message triggers at the child's start, so `widget-naming` is due in
/// the child. The parent's next prompt names no widget, and the parent's scopes
/// never moved, so it is delivered nothing.
#[test]
fn a_subagents_task_activates_scopes_at_its_start() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    assert!(
        !session.active_scopes().contains("widgets"),
        "the parent starts in no project scope, got {:?}",
        session.active_scopes()
    );

    let child = session.subagent("general-purpose", "rename the widget brackets");

    assert!(
        child.model_saw_full("widget-naming"),
        "the task names a widget, so the child starts working in widgets"
    );
    assert!(
        child.active_scopes().contains("widgets"),
        "the scope the task turned on is the child's, got {:?}",
        child.active_scopes()
    );

    session
        .prompt("carry on with the bench supply")
        .assert(|answer| {
            assert!(
                !answer.delivers_full("widget-naming"),
                "the child's task is not the parent's, so the parent is owed nothing new"
            );
            assert!(
                answer.delivers_nothing(),
                "the parent's scopes never moved and it holds everything they cover"
            );
        });
    assert!(
        !session.active_scopes().contains("widgets"),
        "the parent's scopes are unchanged, got {:?}",
        session.active_scopes()
    );
}

/// Detects a write whose author is recorded as the session rather than the
/// context that made it, which is the shape bug #11 took inside a subagent: the
/// parent would be treated as having seen a memory it was never shown, and would
/// act on the old version for the rest of its life.
///
/// The child writes `bench-power`, so the child is the context that holds it and
/// its next event carries nothing. Every other context meets the new version at
/// its next event: the parent as a change to a critical memory it holds, at a
/// prompt, which carries text but stops nothing because only a `PreToolUse` can;
/// and a session that has nothing to do with either of them, the same way.
#[test]
fn a_subagents_write_reaches_its_parent_and_another_session_but_not_itself() {
    let world = World::new().store(Store::example()).build();
    let claude = world.claude(ALPHA);
    let parent = claude.session();
    let other = claude.session();

    parent.start_in(NEUTRAL);
    other.start_in(NEUTRAL);
    let child = parent.subagent("general-purpose", "check the bench supply");
    assert!(
        child.model_saw_full("bench-power"),
        "the child holds the rule before it rewrites it"
    );

    child
        .mcp()
        .memory_put("bench-power", |memory| {
            memory.critical();
            memory.scopes(["global"]);
            memory.source("user");
            memory.description("Cut bench power at the wall and confirm the rail with the meter");
            memory.body(
                "# Cut bench power before rewiring\n\n\
                 The rule now also covers the charger bench, which feeds the same rail.\n",
            );
        })
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the child's own next event is the call's PostToolUse, and it wrote this itself"
            );
        });

    child
        .tool("Grep", json!({ "pattern": "pub fn" }))
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "the context that made the write holds it, so nothing is due and nothing stops"
            );
            assert!(
                answer.delivers_nothing(),
                "a session is never delivered its own write"
            );
        });

    parent
        .prompt("what does the bench supply rule say now")
        .assert(|answer| {
            assert!(
                answer.delivers_full("bench-power"),
                "the parent did not make the write, so the new version is a change to it"
            );
        });
    other
        .prompt("anything new about the bench supply")
        .assert(|answer| {
            assert!(
                answer.delivers_full("bench-power"),
                "a write in one session's subagent is in force in every session"
            );
        });
}

/// Detects a delivery record shared between a session's children: the second
/// child would be sent nothing the first was already given, and would work
/// without a rule it was never shown.
///
/// The store changes between the two starts. The second child is created after
/// it, so the version it is given at its start is the one the store now holds.
/// The first child was given the old one, and meets the change at its own next
/// event; it is a `PreToolUse` and the memory is critical, so the call is
/// stopped and carries the new body. The parent, which was given the old version
/// at its start, meets it once in the same way, and the call it issues again
/// goes ahead with nothing.
#[test]
fn two_subagents_keep_separate_records() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();
    session.start_in(NEUTRAL);

    let first = session.subagent("general-purpose", "list the bench files");
    assert!(
        first.model_saw_full("bench-power"),
        "the first child is given the version the store holds at its start"
    );

    world.rewrite_memory(
        "bench-power",
        "# Cut bench power before rewiring\n\n\
         The rule now also covers the charger bench, which feeds the same rail.\n",
    );

    let second = session.subagent("general-purpose", "list the wiring files");
    assert!(
        second.model_saw_full("bench-power"),
        "the second child has a record of its own, so the new version arrives at its start"
    );

    first
        .tool("Grep", json!({ "pattern": "pub fn" }))
        .assert(|answer| {
            assert!(
                answer.held(),
                "the first child holds the old version, so the change stops its call"
            );
            assert!(
                answer.delivers_full("bench-power"),
                "the stopped call carries the version the store now holds"
            );
        });

    let held = session
        .tool("Read", read("/home/dev/notes/README.md"))
        .assert(|answer| {
            assert!(
                answer.held(),
                "neither child's delivery is the parent's, so the parent meets the change too"
            );
            assert!(answer.delivers_full("bench-power"));
        });
    held.reissue().assert(|answer| {
        assert!(answer.allowed(), "the parent meets the change once");
        assert!(answer.delivers_nothing());
    });
}
