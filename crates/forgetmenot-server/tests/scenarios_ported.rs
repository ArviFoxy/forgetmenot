//! The scenarios that were replayed from YAML files, each now one test on the
//! scenario library.
//!
//! Every test here is a sequence no single event can show: what a session is
//! given after a store changed behind it, after a compaction, after a scope was
//! turned off, after a subagent started. The expectations are the ones the YAML
//! files carried, written from the state machine and the example store; each
//! test's doc comment says which failure it detects and why the answer is what
//! it is, and says where a step was dropped because a test lower down covers it.

mod common;
mod scenario;

use scenario::{Store, World, bash, read};

/// The machine most of these scenarios run on, which is the machine the example
/// store's `workshop` trigger and its session memories are written for.
const ALPHA: &str = "alpha";

/// Detects a catalog that is not rebuilt when the store's head moves, and a
/// change that reaches one context but not another: an edit made from a session
/// on one machine would never reach the sessions on the others, which is the
/// whole point of one store on the LAN.
///
/// Both sessions are owed `bench-power` in full and `reading-list` as an index
/// line, because both are in `global`. Only `alpha/session-1` is owed the notes,
/// whose single scope is `session:alpha/session-1`. The session on beta starts
/// in a directory that names a rocket, and `rocketry`'s trigger is on every
/// text, so `rocket-stages` is owed there as well.
///
/// The edit is a commit to the store, which changes nothing in any context
/// until each one's next event. At alpha's next tool call the blob of
/// `bench-power` differs from the version it was delivered at, so it is changed;
/// it is critical and this is a `PreToolUse`, so the call is held and carries
/// the new body, and nothing that did not change is sent again.
#[test]
fn an_edit_committed_behind_the_server_reaches_another_machines_session_at_its_next_event() {
    let world = World::new().store(Store::example()).build();
    let alpha = world.claude(ALPHA).session();
    let beta = world.claude("beta").session_named("session-2");

    alpha.start_in("/home/dev/widgets").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the global rule is new"
        );
        assert!(answer.delivers_index("reading-list"));
        assert!(answer.delivers_index("sessions/alpha/session-1/notes"));
    });
    beta.start_in("/home/dev/rocketry").assert(|answer| {
        assert!(answer.delivers_full("bench-power"));
        assert!(answer.delivers_index("reading-list"));
        assert!(
            answer.delivers_index("rocket-stages"),
            "the directory names a rocket, so rocketry is on here"
        );
        assert!(
            !answer.delivers_index("sessions/alpha/session-1/notes"),
            "the notes belong to the other session's scope and must not cross"
        );
    });

    world.rewrite_memory(
        "bench-power",
        "# Cut bench power before rewiring\n\n\
         The rule now also covers the charger bench, which feeds the same rail.\n",
    );

    alpha
        .tool("Read", read("/home/dev/notes/README.md"))
        .assert(|answer| {
            assert!(
                answer.held(),
                "a changed critical memory at a PreToolUse holds the call"
            );
            assert!(
                answer.delivers_full("bench-power"),
                "the held call carries the version the store now holds"
            );
            assert!(
                !answer.delivers_index("reading-list")
                    && !answer.delivers_index("sessions/alpha/session-1/notes"),
                "nothing that did not change is sent again"
            );
        });
}

/// Detects a compaction that leaves the delivery record standing, one that also
/// clears the scopes, and one that delivers everything twice.
///
/// What a compaction rebuilds holds nothing that was delivered before it, so
/// everything due has to arrive once at the session start of the rebuilt
/// conversation, while the session is still working in the same scopes and must
/// not lose them. Claude Code takes no context from the answer to the
/// `PostCompact` itself, and the next prompt is owed nothing, because everything
/// arrived at the rebuilt start and is still in the context.
#[test]
fn a_compaction_delivers_everything_again_at_the_rebuilt_start_and_keeps_the_scopes() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in("/home/dev/widgets");
    session
        .prompt("rename the widget brackets")
        .assert(|answer| {
            assert!(
                answer.delivers_full("widget-naming"),
                "the prompt names a widget, so widgets and the rocketry it implies come on"
            );
            assert!(answer.delivers_index("rocket-stages"));
        });

    let compaction = session.auto_compact();

    compaction.start.assert(|answer| {
        assert!(
            answer.delivers_full("bench-power") && answer.delivers_full("widget-naming"),
            "the record of the rebuilt conversation begins afresh, so both rules arrive again"
        );
        assert!(
            answer.delivers_index("reading-list")
                && answer.delivers_index("rocket-stages")
                && answer.delivers_index("sessions/alpha/session-1/notes"),
            "every knowledge memory due arrives again as its index line"
        );
        assert!(
            !answer.retracts("widget-naming"),
            "the record began afresh; the store did not change, so nothing is withdrawn"
        );
    });
    compaction.post.assert(|answer| {
        assert!(
            answer.is_empty_object(),
            "Claude Code accepts no context in the answer to a compaction"
        );
    });
    assert!(
        session.active_scopes().contains("widgets") && session.active_scopes().contains("rocketry"),
        "the session goes on working in the scopes it had, got {:?}",
        session.active_scopes()
    );

    session
        .prompt("carry on where we left off")
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "everything due arrived at the rebuilt start and is still in the context"
            );
        });
}

/// Detects a deny loop and a deny that carries nothing: the session could not
/// make progress, or would be stopped with no rule to read.
///
/// The `tool_input` trigger of `widgets` fires on the call's command and
/// `widgets` implies `rocketry`, so `widget-naming` and `rocket-stages` are new
/// at the call. A new critical memory at a `PreToolUse` stops it, and the
/// stopped call carries what is new. Issuing the call again is a new
/// `PreToolUse`, and the interrupt rule marks what the held answer carried as
/// delivered, so nothing is new, changed or stale and the call goes ahead with
/// nothing attached.
///
/// That Claude Code sends no `PostToolUse` for a call it never ran is the
/// library's rule rather than an assertion here: `ToolCall::result` refuses the
/// call that was held.
#[test]
fn a_held_tool_call_carries_what_is_new_and_the_call_issued_again_goes_ahead() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in("/home/dev/widgets").assert(|answer| {
        assert!(answer.delivers_full("bench-power"));
        assert!(answer.delivers_index("reading-list"));
        assert!(answer.delivers_index("sessions/alpha/session-1/notes"));
    });
    assert_eq!(
        session.active_scopes(),
        ["global", "machine:alpha", "session:alpha/session-1"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "no trigger fired at the start, so the session is in the implicit scopes alone"
    );

    let call = session
        .tool("Bash", bash("cargo test -p widgets"))
        .assert(|answer| {
            assert!(answer.held(), "a new critical memory stops the call");
            assert!(
                answer.delivers_full("widget-naming"),
                "the stopped call carries the rule it was stopped for"
            );
            assert!(answer.delivers_index("rocket-stages"));
            assert!(
                !answer.delivers_full("bench-power"),
                "what was delivered at the start is not sent again"
            );
            assert!(
                answer
                    .deny_reason()
                    .is_some_and(|reason| !reason.is_empty()),
                "the model is told why its call did not run"
            );
        });

    call.reissue().assert(|answer| {
        assert!(answer.allowed(), "the call must not be stopped twice");
        assert!(
            answer.delivers_nothing(),
            "the held answer counts as delivered, so nothing is owed"
        );
    });
}

/// Detects a machine qualifier that survives compilation but is dropped by the
/// context: the same path means different things on different machines, so a
/// session on beta must not end up working in a scope written for alpha, and
/// must not be owed the session memories of a session on alpha.
///
/// That the qualified pattern itself fires for one machine and not the other is
/// covered by `triggers_test::a_machine_qualified_trigger_fires_only_for_its_machine`
/// and is not asserted again here; what is asserted is what the two contexts end
/// up holding.
#[test]
fn two_machines_in_the_same_directory_end_up_in_different_scopes() {
    let world = World::new().store(Store::example()).build();
    let on_alpha = world.claude(ALPHA).session();
    let on_beta = world.claude("beta").session_named("session-2");

    on_alpha.start_in("/workshop/bench").assert(|answer| {
        assert!(answer.delivers_full("bench-power"));
        assert!(answer.delivers_index("sessions/alpha/session-1/notes"));
    });
    on_beta.start_in("/workshop/bench").assert(|answer| {
        assert!(answer.delivers_full("bench-power"));
        assert!(
            !answer.delivers_index("sessions/alpha/session-1/notes"),
            "the notes are in the other session's scope, which nothing here turns on"
        );
    });

    assert!(
        on_alpha.active_scopes().contains("workshop"),
        "the trigger is qualified for alpha, got {:?}",
        on_alpha.active_scopes()
    );
    assert!(
        !on_beta.active_scopes().contains("workshop"),
        "the same directory on beta is a different place, got {:?}",
        on_beta.active_scopes()
    );
}

/// Detects a deleted memory that goes on being delivered, and one that is
/// dropped silently: a session that was given a rule in full has to be told the
/// rule is gone, because nothing else in its context says so.
///
/// A deletion is a write to the store like any other, so it changes nothing in
/// the session until its next event. At that event `widget-naming` is no longer
/// due, because it is no longer in the store, and it is in the delivery record,
/// so it is withdrawn as `deleted`, which tells a retired rule apart from one
/// the session merely stopped working under. Deleting one memory says nothing
/// about any other, so `rocket-stages` is neither sent again nor withdrawn.
#[test]
fn a_deleted_memory_is_withdrawn_from_the_session_it_was_given_to() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in("/home/dev/widgets");
    session
        .prompt("rename the widget brackets")
        .assert(|answer| assert!(answer.delivers_full("widget-naming")));

    world.delete_file("memories/widget-naming.md");

    session
        .says("The brackets are renamed and the build is green.")
        .assert(|answer| {
            assert_eq!(
                answer.retraction_reason("widget-naming"),
                Some("deleted"),
                "the session is told the rule is out of the store"
            );
            assert!(
                !answer.delivers_full("widget-naming"),
                "a withdrawn memory's body must not arrive with the withdrawal"
            );
            assert!(
                !answer.retracts("rocket-stages") && !answer.delivers_index("rocket-stages"),
                "deleting one memory says nothing about any other"
            );
        });
}

/// Detects a scope that cannot be turned off, a withdrawal reported as a
/// deletion, and an implied scope turned off with the scope that implied it.
///
/// The first two would leave the model acting on a rule it was told to stop
/// working under, or tell it a rule was taken out of the store when it was not.
/// The third would silently withdraw memories nobody asked to stop working with:
/// a scope is a flag, and the state does not record which trigger or implication
/// set it.
///
/// Each session tool is called the way Claude Code calls it, wrapped in a
/// `PreToolUse` and a `PostToolUse`, so the effect of the call arrives at the
/// call's own `PostToolUse`: that event is the first one after the scopes moved.
/// The YAML this was ported from called the operations layer directly and saw
/// the same deliveries one event later.
#[test]
fn a_scope_turned_off_withdraws_its_memories_and_leaves_the_scopes_it_implied_on() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();
    session.start_in("/home/dev/notes");
    let mcp = session.mcp();

    mcp.session_scope_on(["widgets"])
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_full("widget-naming"),
                "the scope is on from the call, so its rule is due at the next event"
            );
            assert!(
                answer.delivers_index("rocket-stages"),
                "turning widgets on turns the scopes it implies on with it"
            );
        });

    mcp.session_scope_off(["widgets"])
        .expect_ok()
        .assert_post(|answer| {
            assert_eq!(
                answer.retraction_reason("widget-naming"),
                Some("no longer in an active scope"),
                "the memory is still in the store and still in force everywhere else"
            );
            assert!(
                !answer.retracts("rocket-stages") && !answer.delivers_index("rocket-stages"),
                "rocketry was not turned off, so its memory is neither sent again nor withdrawn"
            );
        });

    assert!(
        session.active_scopes().contains("rocketry"),
        "only the scope named was turned off, got {:?}",
        session.active_scopes()
    );
    assert!(!session.active_scopes().contains("widgets"));
}

/// Detects inheritance that copies the other session's scopes without its own
/// session scope: the reason one session takes over another is to read the notes
/// that session wrote, and those live in its session scope alone, which no
/// trigger can ever turn on.
///
/// The second session is owed the global memories and must not be owed the first
/// session's notes. The inherit call changes the calling context's scopes and
/// nothing in the store, so its effect arrives at the next event on that
/// context, which is the call's own `PostToolUse`. The notes are a knowledge
/// memory, so they arrive as an index line and stop nothing.
#[test]
fn a_session_that_inherits_another_is_owed_that_sessions_own_memories() {
    let world = World::new().store(Store::example()).build();
    let claude = world.claude(ALPHA);
    let first = claude.session();
    let second = claude.session();

    first
        .start_in("/home/dev/widgets")
        .assert(|answer| assert!(answer.delivers_index("sessions/alpha/session-1/notes")));
    second.start_in("/home/dev/widgets").assert(|answer| {
        assert!(answer.delivers_full("bench-power"));
        assert!(answer.delivers_index("reading-list"));
        assert!(
            !answer.delivers_index("sessions/alpha/session-1/notes"),
            "no trigger can turn another session's scope on"
        );
    });

    second
        .mcp()
        .session_inherit(&first.key())
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_index("sessions/alpha/session-1/notes"),
                "the other session's own scope comes with its scopes"
            );
            assert!(
                answer.allowed(),
                "a knowledge memory arriving never stops a call"
            );
            assert!(
                !answer.delivers_full("bench-power"),
                "what this session already holds is not sent again"
            );
        });
}

/// Detects a stale threshold off by one in either direction, staleness judged
/// against the wrong context size, and a stale delivery that stops the call.
///
/// A threshold that fires early wastes the context it is trying to protect; one
/// that fires late leaves a critical rule out of the model's reach; a stale
/// delivery that stops a call would interrupt a session at a fixed interval for
/// a rule nothing changed about.
///
/// The session reports its size by hand, which is what the client reads from the
/// transcript: everything is delivered at 40 000, one token short of the
/// threshold nothing is owed, and exactly at it everything shown before is out
/// of the model's reach and arrives again, each in the form its kind gets.
#[test]
fn a_reminder_arrives_exactly_at_the_threshold_and_not_a_token_before() {
    let world = World::new()
        .store(Store::example())
        .settings(|settings| {
            settings.reminder_tokens(Some(1_000));
        })
        .build();
    let session = world.claude(ALPHA).session();

    session.at_tokens(40_000).start_in("/home/dev/notes");

    session.at_tokens(40_999);
    session
        .tool("Read", read("/home/dev/notes/README.md"))
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "40999 - 40000 is under the threshold, so nothing is stale"
            );
        });

    session.at_tokens(41_000);
    session
        .tool("Read", read("/home/dev/notes/README.md"))
        .assert(|answer| {
            assert!(
                answer.delivers_full("bench-power"),
                "a stale critical memory is sent whole again"
            );
            assert!(
                answer.delivers_index("reading-list")
                    && answer.delivers_index("sessions/alpha/session-1/notes"),
                "a stale knowledge memory is sent as its index line again"
            );
            assert!(
                answer.allowed(),
                "staleness is not an arrival, so the call is not stopped"
            );
        });
}

/// Detects a subagent that starts with only the implicit scopes, a subagent that
/// shares its parent's delivery record, and a parent affected by what its
/// children were told.
///
/// The first would hide from the subagent the memories of the work it was
/// spawned for; the second would let it act on a critical rule it was never
/// shown; the third would stop the parent's calls for deliveries that went
/// somewhere else. The child is created with a copy of the parent's active
/// scopes and an empty record, so everything due to the parent is due to it and
/// arrives once at its start.
///
/// The YAML this was ported from also drove a subagent whose `SubagentStart` was
/// never seen, to show a context being created by a later event. The library
/// sends the events Claude Code sends, and Claude Code sends the start, so that
/// step is dropped; a second subagent covers what it was there for, that each
/// child keeps a record of its own.
#[test]
fn a_subagent_starts_with_its_parents_scopes_and_a_record_of_its_own() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();
    session.start_in("/home/dev/widgets");
    session.prompt("rename the widget brackets");

    let child = session.subagent(
        "general-purpose",
        "Survey the crate and list its public functions",
    );

    for critical in ["bench-power", "widget-naming"] {
        assert!(
            child.model_saw_full(critical),
            "{critical} is due to the parent, so it is due to the child that copied its scopes"
        );
    }
    for knowledge in [
        "reading-list",
        "rocket-stages",
        "sessions/alpha/session-1/notes",
    ] {
        assert!(
            child.model_saw_index(knowledge),
            "{knowledge} must reach the child as its index line"
        );
    }

    child
        .tool("Grep", serde_json::json!({ "pattern": "pub fn" }))
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "everything was delivered at the child's start"
            );
        });

    let second = session.subagent("general-purpose", "Check the test suite");
    assert!(
        second.model_saw_full("widget-naming"),
        "the second child has a record of its own, so the rule arrives there too"
    );

    session
        .tool("Read", read("/home/dev/notes/README.md"))
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the parent holds everything already, and neither child's delivery is its own"
            );
        });
}

/// Detects a chain that only works when the test posts the request body itself:
/// the client reads the transcript, builds the body and copies the answer to
/// stdout, and a break anywhere in that is invisible to every scenario that
/// posts to `/hook` directly.
///
/// One event is enough to show the chain carries an answer: the scenarios above
/// cover what the answer says.
#[test]
fn a_session_start_through_the_real_client_is_answered_with_the_global_rule() {
    let world = World::new()
        .store(Store::example())
        .through_client()
        .build();
    let session = world.claude(ALPHA).session();

    session.start_in("/home/dev/widgets").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the answer the client copied to stdout must be the server's"
        );
    });

    assert_eq!(
        world.context(&session.key())["key"],
        serde_json::json!("alpha/session-1"),
        "the machine the client was started with must be the one the context is under"
    );
}
