//! One scenario per bug that was reported and fixed, each the shortest sequence
//! that reproduced it.
//!
//! Every one of these was invisible at a single event and showed only when a
//! session ran: the store arriving twice around a compaction, a scope turned off
//! by the tool whose own answer turned it back on, a memory delivered back to
//! the session that wrote it. The test is named for the issue and for what it
//! detects, and the doc comment carries the issue's title so that a failure here
//! says which bug came back.

mod common;
mod scenario;

use scenario::{Store, StoreBuilder, World};

/// The machine these scenarios run on.
const ALPHA: &str = "alpha";

/// A directory that turns nothing on.
const QUIET: &str = "/home/dev/parts";

/// Issue 20: "After a compaction the whole store is delivered twice: at the
/// compact SessionStart and again at the first prompt."
///
/// Detects a compaction whose rebuilt `SessionStart` delivers everything without
/// recording it: the next event then finds the same memories undelivered and
/// sends them all over again, costing the rebuilt conversation the whole store
/// twice in two events.
///
/// The prompt after the compaction is owed nothing at all, so the server acts on
/// it no further and answers with the bare object.
#[test]
fn issue_20_a_compaction_does_not_deliver_the_store_twice() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session.compact();

    session
        .prompt("carry on with the bracket")
        .assert(|answer| {
            assert!(
                answer.is_empty_object(),
                "the rebuilt start delivered and recorded the store, so the prompt is owed nothing"
            );
        });
}

/// Issue 17: "Never match triggers against the memory store's own traffic."
///
/// Detects triggers matched against the memory system's own tool calls: the
/// index lists every memory with its scopes, so a session that reads it would
/// turn on every scope the store has and be delivered the whole store, from one
/// call that was only meant to look at what exists.
///
/// The call is made through the real tool, so the text the `PostToolUse` carries
/// is the index the tool actually answered with, naming `widget-naming`,
/// `widgets` and `rocketry`. None of them turns on, and the event is owed
/// nothing.
#[test]
fn issue_17_the_stores_own_tool_answer_activates_no_scope() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    let before = session.active_scopes();

    let listing = session.mcp().memory_index().expect_ok();
    let text = listing.json().to_string();
    assert!(
        text.contains("widget-naming") && text.contains("widgets") && text.contains("rocketry"),
        "the index has to name the memories and scopes for this to be a test at all, got {text}"
    );
    listing.assert_post(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the index named every scope and turned none of them on, so nothing is owed"
        );
    });

    assert_eq!(
        session.active_scopes(),
        before,
        "reading the index must leave the session working where it was"
    );
}

/// Issue 15: "session_scope_off is undone by the next event: forgetmenot's own
/// tool results re-trigger the scope."
///
/// Detects a scope turned off by a tool whose own answer names it: the answer
/// reaches the server as the `PostToolUse` of that very call, so a server that
/// matched it would turn the scope back on one event after turning it off and
/// the model could never put a scope down.
///
/// The prompt names a widget in the singular, which is what `widgets`'s
/// user-message pattern matches, so the scope is on and its rule has been given.
/// Turning it off leaves `widget-naming` covered by no active scope, so the
/// `PostToolUse` of the same call withdraws it, which is the opposite of
/// re-delivering it, and the scope stays off.
#[test]
fn issue_15_a_scope_turned_off_stays_off_through_its_own_answer() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session
        .prompt("check the widget part numbers on the bracket")
        .assert(|answer| {
            assert!(
                answer.delivers_full("widget-naming"),
                "the prompt names a widget, so widgets comes on and its rule arrives"
            );
        });

    session
        .mcp()
        .session_scope_off(["widgets"])
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.retracts("widget-naming"),
                "no active scope covers the rule any more, so it is withdrawn here"
            );
        });

    assert!(
        !session.active_scopes().contains("widgets"),
        "the scope the session put down stays down, got {:?}",
        session.active_scopes()
    );
}

/// The body the writing session gives `bench-power`. None of its lines appeared
/// in the body the store held, so every other session meets it as a change.
const WRITTEN: &str = "# Cut bench power before rewiring\n\n\
                       Switch the bench supply off at the wall, and switch the charger bench off \
                       with it; the two feed the same rail.\n\n\
                       Confirm on the meter that the rail reads zero before touching any \
                       wiring.\n";

/// Issue 11: "A memory is re-delivered to the very context that just wrote it."
///
/// Detects a write that is not recorded against its writer: the session composed
/// the text, so it holds it already, and delivering it back costs the context the
/// whole body again at the next event and tells the model something it just said.
/// Every other session does have to be told, which is what makes this a bug about
/// the writer alone and not about writes.
///
/// Both sessions start before the write, so both hold the store's version of
/// `bench-power`. The writing session's own `PostToolUse` and its next prompt are
/// owed nothing, because the version it wrote is the version it is recorded as
/// holding. The reading session is recorded at the version before it, so its next
/// event finds the version changed and is given the new body whole.
#[test]
fn issue_11_a_write_is_not_delivered_back_to_its_writer() {
    let world = World::new().store(Store::example()).build();
    let claude = world.claude(ALPHA);
    let writer = claude.session();
    let reader = claude.session();

    writer.start_in(QUIET);
    reader.start_in(QUIET);

    writer
        .mcp()
        .memory_put("bench-power", |memory| {
            memory
                .critical()
                .scopes(["global"])
                .description(
                    "Cut bench power at the wall before rewiring and confirm with the meter",
                )
                .body(WRITTEN);
        })
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the session that wrote the text is not told it"
            );
        });

    writer.prompt("now the charger bench").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "and it is not told at the event after it either"
        );
    });

    reader.prompt("what is the bench rule?").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the session that did not write it holds the old version and meets the change"
        );
    });
}

/// The body `bench-power` starts with in the shrink scenario, written out here
/// so the line that is removed is visible in the test rather than read back out
/// of the store.
const THREE_RULES: &str = "# Cut bench power before rewiring\n\n\
                           Switch the bench supply off at the wall.\n\
                           Confirm on the meter that the rail reads zero.\n\
                           Label the supply before leaving the bench.\n";

/// The same body with the last rule taken out and nothing put in.
const TWO_RULES: &str = "# Cut bench power before rewiring\n\n\
                         Switch the bench supply off at the wall.\n\
                         Confirm on the meter that the rail reads zero.\n";

/// Issue 14: "No re-delivery when a memory's delivered content only shrinks."
///
/// Detects a version whose text only lost lines being delivered again: the
/// session has already been given every line the new text holds, so sending it
/// costs the context the whole body to say nothing new. A memory edited a line at
/// a time would arrive whole at every edit.
///
/// The edit is a commit the server is not told about, so the context meets it at
/// its next event. Every line of the new text appeared in the text the session
/// was given, in order, so nothing is owed and the prompt is answered with
/// nothing. The new version is still recorded as held, which is what stops it
/// arriving at the event after that, and the statistics count the outcome as its
/// own thing rather than as a delivery, so a report cannot read it as context the
/// session was charged for.
#[test]
fn issue_14_a_memory_that_only_shrank_is_not_delivered_again() {
    let world = World::new().store(store_with_three_rules()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET).assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the session is given all three rules to begin with"
        );
    });

    world.rewrite_memory("bench-power", TWO_RULES);

    session.prompt("carry on at the bench").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the session has been given every line the new text holds"
        );
    });

    let rows = world.stats_deliveries_of("bench-power");
    assert_eq!(
        rows.len(),
        1,
        "one memory has one statistics row, got {rows:?}"
    );
    assert_eq!(
        rows[0]["shrunk"], 1,
        "the shrink is recorded as its own outcome and not as a delivery: {}",
        rows[0]
    );
}

/// The example store with `bench-power` at a body whose lines the test names.
fn store_with_three_rules() -> StoreBuilder {
    Store::example().memory("bench-power", |memory| {
        memory
            .critical()
            .scopes(["global"])
            .description("Cut bench power at the wall before rewiring and confirm with the meter")
            .body(THREE_RULES);
    })
}
