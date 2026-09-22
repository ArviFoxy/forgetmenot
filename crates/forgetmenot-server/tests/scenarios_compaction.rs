//! What a compaction does to the session around it.
//!
//! Claude Code sends two events for one compaction, a `SessionStart` for the
//! conversation it rebuilt and a `PostCompact` reporting what it removed, and
//! the session goes on afterwards in the same scopes with none of the text it
//! was given before. What that costs is a property of the sequence: everything
//! due has to arrive once at the rebuilt start, the scopes the session turned on
//! by hand have to survive, and the events after it must be owed nothing. That
//! either answer on its own has the right shape is tested where those events
//! are.

mod common;
mod scenario;

use scenario::{Store, StoreBuilder, World, bash};

/// The machine these scenarios run on.
const ALPHA: &str = "alpha";

/// A directory that turns nothing on. In particular it is not a workshop, so
/// the only way `workshop` comes on here is the session asking for it.
const QUIET: &str = "/home/dev/parts";

/// The example store has no memory in `workshop`, so a scenario about that scope
/// surviving a compaction has to put one there: a scope with nothing in it
/// delivers nothing and would say nothing about whether it survived.
fn store_with_a_workshop_rule() -> StoreBuilder {
    Store::example().memory("workshop-bench", |memory| {
        memory
            .critical()
            .scope("workshop")
            .description("The bench is cleared and wiped before a new job starts")
            .body(
                "# Clear the bench between jobs\n\n\
                 The bench is cleared of the last job's parts and wiped down before the next \
                 job is set out on it.\n",
            );
    })
}

/// Detects a compaction that rebuilds the conversation from the store's scopes
/// rather than the session's: a scope the model turned on by hand is in the
/// registry and in nothing the triggers would find again, so a compaction that
/// recomputed the scopes from the rebuilt transcript would silently drop it and
/// the memories with it.
///
/// `workshop`'s only trigger is a directory on this machine, and the session is
/// not in it, so the scope is on only because the session asked for it through
/// the tool. The compaction's `SessionStart` clears what the context was
/// recorded as holding but not the scopes it works in, so everything due for
/// `global` and `workshop` alike arrives once there. Claude Code takes no
/// context from the answer to the `PostCompact` itself, and the prompt after it
/// is owed nothing because the rebuilt start gave it all.
#[test]
fn a_manual_compaction_delivers_once_and_keeps_a_scope_turned_on_by_mcp() {
    let world = World::new().store(store_with_a_workshop_rule()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session.mcp().session_scope_on(["workshop"]).expect_ok();

    let compaction = session.compact();

    compaction.start.assert(|answer| {
        assert!(
            answer.delivers_full("bench-power") && answer.delivers_full("workshop-bench"),
            "the rebuilt conversation holds neither rule, so both arrive at its start"
        );
        assert!(
            answer.delivers_index("reading-list")
                && answer.delivers_index("sessions/alpha/session-1/notes"),
            "every knowledge memory due arrives again as its index line"
        );
    });
    compaction.post.assert(|answer| {
        assert!(
            answer.is_empty_object(),
            "Claude Code takes no context from the answer to a compaction"
        );
    });

    session
        .prompt("carry on with the bench job")
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the rebuilt start gave everything, so the next prompt is owed nothing"
            );
        });

    assert!(
        session.active_scopes().contains("workshop"),
        "the scope the session turned on by hand survives the compaction, got {:?}",
        session.active_scopes()
    );
}

/// Detects a compaction that leaves the next tool call holding a critical
/// arrival: Claude Code compacts in the middle of a run of tool calls, and the
/// rebuilt start has just delivered every critical memory due. A call stopped
/// straight after would stop the work for a rule the model was given one event
/// ago, which is the interrupt firing on the compaction rather than on anything
/// the session did.
///
/// The session is working in the implicit scopes alone and everything due
/// arrived at its start, so the first `Bash` call is owed nothing and runs. The
/// automatic compaction delivers everything again at its rebuilt start, which is
/// not a `PreToolUse` and stops nothing. The next `Bash` call finds the record
/// of the rebuilt conversation complete, so it is owed nothing and goes ahead.
#[test]
fn an_auto_compaction_in_the_middle_of_tool_calls_does_not_hold_the_next_call() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session
        .tool("Bash", bash("ls ./parts-list"))
        .assert(|answer| {
            assert!(answer.allowed(), "everything due arrived at the start");
        })
        .run();

    session.auto_compact().start.assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the rebuilt conversation holds no rule, so the rule arrives at its start"
        );
    });

    session
        .tool("Bash", bash("ls ./parts-list"))
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "everything was delivered one event ago, so nothing is new to stop this call for"
            );
            assert!(
                answer.delivers_nothing(),
                "and nothing is owed, so the call carries no text either"
            );
        })
        .run();
}

/// Detects a second compaction answered from the first one's record: the record
/// is cleared at each rebuilt start, so two compactions in a row are two
/// deliveries, and a session that compacted twice without being given its rules
/// the second time is working without them.
///
/// Each `SessionStart` clears what the context holds and everything due arrives
/// there. Nothing happens between the two compactions but the first
/// `PostCompact`, which Claude Code takes no context from, and the prompt after
/// the second is owed nothing.
#[test]
fn two_compactions_in_a_row_deliver_once_each() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);

    let first = session.compact();
    first.start.assert(|answer| {
        assert!(
            answer.delivers_full("bench-power") && answer.delivers_index("reading-list"),
            "the first rebuilt start is owed the whole store"
        );
    });
    first.post.assert(|answer| {
        assert!(
            answer.is_empty_object(),
            "nothing arrives between the two compactions"
        );
    });

    let second = session.compact();
    second.start.assert(|answer| {
        assert!(
            answer.delivers_full("bench-power") && answer.delivers_index("reading-list"),
            "the second rebuilt start holds none of what the first one delivered"
        );
    });
    second.post.assert(|answer| {
        assert!(answer.is_empty_object(), "nothing arrives after it either");
    });

    session.prompt("carry on").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the second rebuilt start gave everything, so the next prompt is owed nothing"
        );
    });
}
