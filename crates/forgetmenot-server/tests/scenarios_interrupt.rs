//! What holding a tool call does to the events around it.
//!
//! Holding a call is not one answer: it changes the sequence Claude Code sends
//! afterwards. A held call never runs, so no `PostToolUse` follows it; the model
//! reads what stopped it and issues a fresh call, which is a second
//! `PreToolUse`; and the record the decision was made against belongs to one
//! context, so a subagent's hold says nothing about its parent's next call.
//! Whether a given answer holds is decided at that one event and is tested where
//! that event is.

mod common;
mod scenario;

use scenario::{Store, World, bash, read};
use serde_json::{Value, json};

/// The machine these scenarios run on.
const ALPHA: &str = "alpha";

/// A directory that turns nothing on.
const QUIET: &str = "/home/dev/parts";

/// A `Bash` command whose input names widgets, which is what `widgets`'s
/// tool-input pattern matches.
const WIDGET_COMMAND: &str = "grep -rn widgets ./parts-list";

/// How many events of one name the server has answered, read from the latency
/// report, which counts every hook event it recorded by name.
///
/// There is no per-event listing in the API, so this is the count of the whole
/// world; each scenario below runs one session, so the world's count is that
/// session's.
fn events_named(world: &World, event: &str) -> u64 {
    let (status, body) = world.api("GET", "/api/stats/latency", None);
    assert_eq!(
        status, 200,
        "the latency report must be readable, got {body}"
    );
    body.as_array()
        .expect("the latency report is a JSON array")
        .iter()
        .find(|row| row["event"] == json!(event))
        .map(|row| {
            row["count"]
                .as_u64()
                .expect("a latency row counts its events")
        })
        .unwrap_or(0)
}

/// The one statistics row for a memory, failing when the memory was never sent
/// anywhere.
fn delivery_row(world: &World, memory: &str) -> Value {
    let rows = world.stats_deliveries_of(memory);
    assert_eq!(
        rows.len(),
        1,
        "one memory has one statistics row, got {rows:?}"
    );
    rows.into_iter().next().expect("the row is there")
}

/// Detects a hold that is recorded as nothing delivered, and a reissue answered
/// as a repeat of the call it replaces: a hold whose memory is not recorded
/// would hold every call forever, and a reissue counted as the same call would
/// either be denied again or leave the second delivery unrecorded.
///
/// The `Bash` input names widgets, so `widgets` comes on at the `PreToolUse`,
/// `widget-naming` is new and critical there, and the call is stopped. The
/// memory is recorded against the context at that moment, which is why the
/// context holds one more memory than before and why the call issued again goes
/// ahead.
///
/// Claude Code sends no `PostToolUse` for a call that never ran, and the reissue
/// is a new call from the model, so the session's events are two `PreToolUse`
/// and one `PostToolUse`. `widget-naming` was sent once, as new, across all
/// three: the second `PreToolUse` and the `PostToolUse` were owed nothing.
#[test]
fn a_held_call_is_followed_by_no_result_and_its_reissue_is_a_new_call() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    let before = session.delivered_count();

    let held = session.tool("Bash", bash(WIDGET_COMMAND)).assert(|answer| {
        assert!(
            answer.held(),
            "a critical memory new at a PreToolUse stops the call"
        );
    });
    assert!(
        session.delivered_count() > before,
        "the memory the hold carried is recorded at the hold, got {} after {before}",
        session.delivered_count()
    );

    held.reissue()
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "the rule was recorded at the hold, so the call issued again goes ahead"
            );
        })
        .run();

    assert_eq!(
        events_named(&world, "PreToolUse"),
        2,
        "the reissue is a second call from the model, not a repeat of the first"
    );
    assert_eq!(
        events_named(&world, "PostToolUse"),
        1,
        "the held call never ran, so only the call that went ahead reported a result"
    );

    let row = delivery_row(&world, "widget-naming");
    assert_eq!(
        row["shown_full_new"], 1,
        "the rule was sent once, at the hold: {row}"
    );
    assert_eq!(
        row["shown_full_changed"].as_u64().unwrap_or(0)
            + row["shown_full_stale"].as_u64().unwrap_or(0),
        0,
        "nothing changed and nothing went stale between the hold and the reissue: {row}"
    );
}

/// The body `widget-naming` is rewritten to between the hold and the reissue.
/// No line of it appeared in the body the context was given, so it is a change
/// and not a text that only shrank.
const REWRITTEN: &str = "# Part numbers are allocated by the parts office\n\n\
                         Only the parts office allocates a widget part number, and it allocates \
                         the next free one.\n\n\
                         A number that has shipped is closed: reissuing it is a change to every \
                         drawing that cited it.\n";

/// Detects a reissue answered from the version the hold carried: the model was
/// stopped, read a rule, and is issuing the call again, so if the rule moved in
/// between it must be stopped again with the new text. Answering the reissue
/// from the recorded version would let the call run under a rule the store no
/// longer holds.
///
/// The first call is held with the rule as the store held it then. The rewrite
/// is a commit the server is not told about, so the context meets it at its next
/// event: at the reissue the version differs from the one recorded, the memory
/// is critical and changed, and a `PreToolUse` with a critical arrival is
/// stopped. The text it carries is the new body. The second reissue is owed
/// nothing and goes ahead.
#[test]
fn an_edit_between_the_hold_and_the_reissue_holds_the_call_again_with_the_new_text() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    let held = session.tool("Bash", bash(WIDGET_COMMAND)).assert(|answer| {
        assert!(answer.held(), "the rule is new at this call");
    });

    world.rewrite_memory("widget-naming", REWRITTEN);

    let held_again = held.reissue().assert(|answer| {
        assert!(
            answer.held(),
            "the rule moved since the hold, so the call is stopped under the version it moved to"
        );
        assert!(
            answer.delivers_full("widget-naming"),
            "the second hold carries the body the store holds now"
        );
    });

    held_again.reissue().assert(|answer| {
        assert!(
            answer.allowed(),
            "the new version was recorded at the second hold, so the third call goes ahead"
        );
    });
}

/// Detects a hold decided against the wrong context's record: a subagent and the
/// session it runs in each hold their own memories, so a child that was given a
/// rule must not spend its parent's hold, and a parent that was never given it
/// must still be stopped. One record shared between them would silence one side
/// or the other.
///
/// The subagent's task names a widget in the singular, which `widgets`'s
/// user-message pattern matches, and the task is matched once where the subagent
/// begins. A `SubagentStart` cannot stop anything, so the child simply holds the
/// rule from its first event, and its own tool call goes ahead. The parent's
/// context never saw the task: its own call naming widgets turns `widgets` on
/// there for the first time, so the rule is new and the call is stopped. The
/// child's record is untouched by that, so its next call goes ahead too.
#[test]
fn a_subagents_hold_leaves_the_parents_next_call_alone() {
    let world = World::new().store(Store::example()).build();
    let parent = world.claude(ALPHA).session();

    parent.start_in(QUIET);
    let child = parent.subagent(
        "general-purpose",
        "audit the widget part numbers on the brackets",
    );
    assert!(
        child.model_saw_full("widget-naming"),
        "the task named a widget, so the child starts holding the rule"
    );

    child
        .tool("Bash", bash(WIDGET_COMMAND))
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "the child was given the rule at its start, so nothing is new at its first call"
            );
        })
        .run();

    parent.tool("Bash", bash(WIDGET_COMMAND)).assert(|answer| {
        assert!(
            answer.held(),
            "the parent's own record has no widget rule, so its call is stopped"
        );
    });

    child.tool("Bash", bash(WIDGET_COMMAND)).assert(|answer| {
        assert!(
            answer.allowed(),
            "the parent's hold changed nothing in the child's record"
        );
    });
}

/// Detects an exemption read as "deliver nothing": a tool the store exempts is
/// one the user does not want stopped, not one the memories are kept from. The
/// text has to reach the session all the same, or the rule that would have
/// stopped the call is never given and the next call is stopped instead.
///
/// The `Read` input names a widget file, so `widgets` comes on and
/// `widget-naming` is new and critical at the `PreToolUse`. The store exempts
/// `Read`, so the call goes ahead, and the answer still carries the body into
/// the session, which is what the model can read afterwards.
#[test]
fn an_exempt_tool_is_never_held_but_still_gets_the_text() {
    let world = World::new()
        .store(Store::example())
        .settings(|settings| {
            settings.interrupt_exempt_tools(["Read"]);
        })
        .build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session
        .tool("Read", read("/home/dev/parts/widgets.md"))
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "the store exempts Read, so no call of it is ever stopped"
            );
            assert!(
                answer.delivers_full("widget-naming"),
                "the exemption is about stopping the call, not about the text"
            );
        })
        .run();

    assert!(
        session.model_saw_full("widget-naming"),
        "the rule the exempt call carried must be in the text the model can read"
    );
}

/// Detects a hold triggered by any arrival rather than by a critical one: a
/// knowledge memory is a line the model may look up, and stopping the work for
/// one would hold a call at every scope a tool input happens to name.
///
/// The session starts in a directory that turns nothing on. The `Bash` input
/// names a rocket, so `rocketry` comes on at the `PreToolUse`; its only memory
/// is knowledge, so an index line is all that is owed, and the call goes ahead
/// carrying it.
#[test]
fn a_knowledge_arrival_holds_nothing() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session
        .tool("Bash", bash("ls ./rocket-frames"))
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "nothing critical arrived, so there is nothing to stop the call for"
            );
            assert!(
                answer.delivers_index("rocket-stages"),
                "the scope the input turned on still delivers its knowledge memory"
            );
        })
        .run();
}
