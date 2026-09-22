//! What the context size does to a session as it grows: when a delivery goes
//! stale and arrives again, and when a scope with a `forget` rule turns itself
//! off.
//!
//! Both rules are counted in tokens rather than in events, so neither is visible
//! at one event: each needs a sequence with sizes chosen around the threshold.
//! What the rules are at a single event is tested where that event is; what is
//! here is where the count starts, what restarts it, and what a second context
//! counts from.

mod common;
mod scenario;

use scenario::{Field, Store, StoreBuilder, World, bash};

/// The machine these scenarios run on, which is the machine the example store's
/// session memories are written for.
const ALPHA: &str = "alpha";

/// The directory the sessions start in: it matches no trigger of the example
/// store, so every scope a scenario works with was turned on by that scenario.
const NEUTRAL_DIRECTORY: &str = "/home/dev/notes";

/// The context growth after which everything already delivered is delivered
/// again, and the growth after which a scope with no trigger of its own is
/// turned off. One number for both, small enough that a scenario can name sizes
/// around it by hand.
const THRESHOLD: u64 = 1_000;

/// The size the sessions here report at the event that first delivers
/// something, so that every later size is this plus a stated amount.
const START: u64 = 10_000;

/// A scope nothing in the example store turns on, whose trigger is a word no
/// memory of the example store carries, so that a scenario turns it on exactly
/// when it means to.
const PASSING: &str = "passing";

/// The critical memory in [`PASSING`], and a line of its body, so that a test
/// can ask whether the body arrived without repeating the whole of it.
const PASSING_RULE: &str = "passing-rule";
const PASSING_RULE_BODY: &str = "# Label a passing measurement\n\n\
     A measurement taken while the bench is still settling is labelled as such \
     in the log.\n";

/// The line taken out of `bench-power` for the shrink, which is in the example
/// store's body and nowhere else, so a test can ask whether the longer body
/// arrived.
const DROPPED_LINE: &str = "The supply holds a charge for a few seconds after the switch, \
     so the meter check";

/// `bench-power`'s body with the last paragraph taken out and nothing put in,
/// which is what makes the new version a shrink of the old one.
const SHRUNK_BENCH_POWER: &str = "# Cut bench power before rewiring\n\n\
     Switch the bench supply off at the wall before changing any wiring, and confirm\n\
     with the meter that the rail reads zero.\n";

/// The example store with a scope that turns itself off [`THRESHOLD`] tokens
/// after the last text that fired it, holding one critical memory.
///
/// The trigger is on the user's messages alone, so nothing the memory system's
/// own traffic carries can fire it: a scenario that names the scope in a tool
/// call or reads the memory back is not re-triggering it by accident.
fn forgetting_store() -> StoreBuilder {
    Store::example()
        .scope(PASSING, |scope| {
            scope
                .trigger(Field::UserMessage, r"\bpassing\b")
                .forget_after(THRESHOLD);
        })
        .memory(PASSING_RULE, |memory| {
            memory
                .critical()
                .scope(PASSING)
                .description("A measurement taken while the bench settles is labelled as such")
                .body(PASSING_RULE_BODY);
        })
}

/// Detects a reminder threshold counted from the wrong moment: from the session
/// start rather than from the last delivery, or from an event count rather than
/// from the context size.
///
/// Either failure is invisible at one event, because one event has only one
/// delivery to count from. The session is delivered everything at 10 000. At
/// 10 999 the growth since is 999, one short of the threshold, so nothing is
/// owed. At 11 000 it is exactly the threshold: what was shown that long ago is
/// out of the model's reach and arrives again, the critical rule whole and the
/// knowledge memory as its index line.
///
/// The third prompt is at 11 999, which is 1 999 past the first delivery and 999
/// past the second. It is owed nothing only if the reminder moved the count to
/// the second delivery, which is the emergent part: a threshold that keeps
/// counting from the session start would deliver everything again here, and
/// every 1 000 tokens for the rest of the session.
#[test]
fn a_reminder_arrives_at_the_threshold_and_not_a_token_before() {
    let world = World::new()
        .settings(|settings| {
            settings.reminder_tokens(Some(THRESHOLD));
        })
        .build();
    let session = world.claude(ALPHA).session();

    session.at_tokens(START).start_in(NEUTRAL_DIRECTORY);

    session.at_tokens(START + THRESHOLD - 1);
    session
        .prompt("carry on where we left off")
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the context has grown by 999 since the delivery, one token short of the threshold"
            );
        });

    session.at_tokens(START + THRESHOLD);
    session.prompt("carry on again").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the growth since the delivery has reached the threshold, so the rule arrives whole"
        );
        assert!(
            answer.delivers_index("reading-list"),
            "a stale knowledge memory arrives as its index line"
        );
    });

    session.at_tokens(START + 2 * THRESHOLD - 1);
    session.prompt("and once more").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the reminder counts from the reminder: 999 tokens on from it, nothing is stale"
        );
    });
}

/// Detects a `forget` rule counted from the wrong moment or applied to the wrong
/// side of the threshold, and a scope that turns itself off without withdrawing
/// what it was delivering.
///
/// A scope that turns itself off early takes a rule away from a session still
/// working under it; one that never does keeps the rule in every later answer of
/// a session that moved on. Leaving the memory standing after the scope is off
/// is worse than either: the model goes on acting on a rule the session has
/// stopped working under, and nothing in its context says otherwise.
///
/// The prompt at 10 000 fires the trigger, so the scope is counted from there.
/// At 10 999 the growth is 999 and the scope stays on, which is why that event
/// is owed nothing. At 11 000 it is exactly the threshold: the scope goes off
/// before the event is answered, so the memory is no longer due, and it is in
/// the delivery record, which is what makes it a withdrawal rather than silence.
#[test]
fn a_scope_is_forgotten_after_its_tokens_and_its_memory_withdrawn() {
    let world = World::new().store(forgetting_store()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL_DIRECTORY);

    session.at_tokens(START);
    session
        .prompt("a passing remark about the rail")
        .assert(|answer| {
            assert!(
                answer.delivers_full(PASSING_RULE),
                "the prompt fires the trigger, so the scope's rule is due at this event"
            );
        });

    session.at_tokens(START + THRESHOLD - 1);
    session.tool("Bash", bash("cargo build")).assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "999 tokens on, the scope is still on and its rule is still held"
        );
        assert!(
            answer.allowed(),
            "nothing is due, so the call is not stopped"
        );
    });

    session.at_tokens(START + THRESHOLD);
    session.prompt("what is next").assert(|answer| {
        assert_eq!(
            answer.retraction_reason(PASSING_RULE),
            Some("no longer in an active scope"),
            "the scope turned itself off, so the session is told to stop working under its rule"
        );
    });

    assert!(
        !session.active_scopes().contains(PASSING),
        "the scope must be off, got {:?}",
        session.active_scopes()
    );
    assert_eq!(
        world.stats_scope(PASSING)["forgettings"],
        serde_json::json!(1),
        "the scope turning itself off is recorded once, which is what the scopes page counts"
    );
}

/// Detects a `forget` count that is never restarted: a scope whose subject keeps
/// coming up would be dropped at a fixed size whatever the session is doing,
/// which is the opposite of what the rule is for.
///
/// The trigger fires at 10 000 and again at 10 800. The second fire is what this
/// test is about, so the events are chosen so that only a restarted count
/// explains them: at 11 000 the growth since the first fire is 1 000, at the
/// threshold, and the growth since the second is 200. The scope stays on there,
/// and goes off at 11 800, which is 1 000 past the second fire.
#[test]
fn a_re_fire_restarts_the_count() {
    let world = World::new().store(forgetting_store()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL_DIRECTORY);

    session.at_tokens(START);
    session
        .prompt("a passing remark about the rail")
        .assert(|answer| assert!(answer.delivers_full(PASSING_RULE)));

    session.at_tokens(START + 800);
    session
        .prompt("another passing remark")
        .assert(|answer| assert!(answer.delivers_nothing(), "the rule is already held here"));

    session.at_tokens(START + THRESHOLD);
    session.prompt("what is next").assert(|answer| {
        assert!(
            !answer.retracts(PASSING_RULE),
            "the count runs from the second fire, which is 200 tokens back"
        );
    });
    assert!(
        session.active_scopes().contains(PASSING),
        "the scope must still be on, got {:?}",
        session.active_scopes()
    );

    session.at_tokens(START + 800 + THRESHOLD);
    session.prompt("and now").assert(|answer| {
        assert!(
            answer.retracts(PASSING_RULE),
            "1 000 tokens on from the second fire, the scope turns itself off"
        );
    });
}

/// Detects a scope turned on by the model that is never counted at all, and one
/// counted from a size no event reported.
///
/// `session_scope_on` has no hook event of its own: it is a tool call, and the
/// only context size anywhere near it is the one the last event reported. A
/// scope turned on this way that is never counted stays on for the rest of the
/// session; one counted from zero is gone at the next event. Neither is visible
/// in the tool's own answer, which is why it is asserted here.
///
/// The call is made with the last event at 10 000, so the scope is counted from
/// there: at 10 999 it is still on, and at 11 000, exactly the threshold past
/// the size that event reported, it is off and its rule is withdrawn.
#[test]
fn a_scope_turned_on_by_mcp_counts_from_the_last_events_tokens() {
    let world = World::new().store(forgetting_store()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL_DIRECTORY);

    session.at_tokens(START);
    session
        .mcp()
        .session_scope_on([PASSING])
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_full(PASSING_RULE),
                "the scope is on from the call, so its rule is due at the next event"
            );
        });

    session.at_tokens(START + THRESHOLD - 1);
    session.tool("Bash", bash("cargo build")).assert(|answer| {
        assert!(
            !answer.retracts(PASSING_RULE),
            "999 tokens past the event the call was counted from, the scope is still on"
        );
    });

    session.at_tokens(START + THRESHOLD);
    session.tool("Bash", bash("cargo build")).assert(|answer| {
        assert!(
            answer.retracts(PASSING_RULE),
            "the threshold past that size, the scope turns itself off"
        );
    });
}

/// Detects a subagent that counts an inherited forgetting scope from a size
/// that is not its own: from the parent's trigger, or from the size the
/// `SubagentStart` reports, which the client read from the parent's transcript.
/// The child's transcript begins empty, so its own sizes start near the size a
/// session opens at and have nothing to do with the parent's; counting the
/// parent's number against them forgets the scope at an arbitrary moment in the
/// child, or never.
///
/// The parent fires the trigger at 10 000 and spawns the child at 10 900. The
/// child's count starts at its own first event, which reports the size a session
/// opens at, 10 000: at 10 999 the growth is 999, one short of the threshold, and
/// at 11 000 it is exactly the threshold and the scope goes. A count started at
/// the 10 900 the `SubagentStart` reported would keep the scope at 11 000 and
/// drop it at 11 900; one started at the parent's 10 000 trigger would drop it
/// as soon as the child grew past 11 000 whatever the child had done.
///
/// The parent is asked afterwards at 10 999 and 11 000: the child's events are
/// not the parent's, and the parent goes off at the threshold past its own
/// trigger, neither earlier because the child forgot nor later because the child
/// started.
#[test]
fn a_subagent_counts_a_forgetting_scope_from_its_own_first_event() {
    let world = World::new().store(forgetting_store()).build();
    let parent = world.claude(ALPHA).session();

    parent.start_in(NEUTRAL_DIRECTORY);
    parent.at_tokens(START);
    parent
        .prompt("a passing remark about the rail")
        .assert(|answer| assert!(answer.delivers_full(PASSING_RULE)));

    parent.at_tokens(START + 900);
    let child = parent.subagent("general-purpose", "Survey the bench wiring");
    assert!(
        child.start_answer().delivers_full(PASSING_RULE),
        "the child copied the parent's scopes, so the rule is due at its start"
    );

    child.at_tokens(START);
    child.prompt("start with the rail").assert(|answer| {
        assert!(
            !answer.retracts(PASSING_RULE),
            "the child's first own event is where its count starts, not where it ends"
        );
    });

    child.at_tokens(START + THRESHOLD - 1);
    child.prompt("carry on").assert(|answer| {
        assert!(
            !answer.retracts(PASSING_RULE),
            "999 tokens past the child's own first event, the scope is still on for it"
        );
    });

    child.at_tokens(START + THRESHOLD);
    child.prompt("and the rail again").assert(|answer| {
        assert!(
            answer.retracts(PASSING_RULE),
            "the threshold past the child's own first event, the scope goes off for it"
        );
    });

    parent.at_tokens(START + THRESHOLD - 1);
    parent.prompt("what did it find").assert(|answer| {
        assert!(
            !answer.retracts(PASSING_RULE),
            "the child's events are not the parent's, and the parent is 999 past its own trigger"
        );
    });
    parent.at_tokens(START + THRESHOLD);
    parent.prompt("and now").assert(|answer| {
        assert!(
            answer.retracts(PASSING_RULE),
            "the parent goes off at the threshold past its own trigger, not the child's"
        );
    });
}

/// Detects a reminder that repeats the text a memory had when it was delivered
/// rather than the text the store holds now, and a shrink that does not restart
/// the reminder count.
///
/// A memory whose delivered text only lost lines says nothing the context has
/// not been given, so nothing is sent for it; but it is recorded as held at the
/// new version and at that moment, which is the part that only a later event
/// shows. The rewrite is met at 10 100, so the reminder is counted from there:
/// the memory is stale at 11 100, not at 11 000, and what arrives is the body
/// the store holds, with the dropped line absent.
///
/// A reminder that replayed the delivered text would repeat a paragraph the
/// store no longer has, and the model would go on acting on it.
#[test]
fn a_shrunk_memory_reminded_later_arrives_in_its_new_form() {
    let world = World::new()
        .settings(|settings| {
            settings.reminder_tokens(Some(THRESHOLD));
        })
        .build();
    let session = world.claude(ALPHA).session();

    session.at_tokens(START).start_in(NEUTRAL_DIRECTORY);
    world.rewrite_memory("bench-power", SHRUNK_BENCH_POWER);

    session.at_tokens(START + 100);
    session.prompt("carry on").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the new body is the old one with lines taken out, so the context is owed nothing"
        );
    });

    session.at_tokens(START + 100 + THRESHOLD);
    session.prompt("and now").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the threshold past the event that met the shrink, the rule is stale"
        );
        assert!(
            answer.text_lacks(DROPPED_LINE),
            "the reminder must carry the body the store holds, not the one delivered before it"
        );
    });
}
