//! What survives the server being stopped and started again on the same store,
//! state file and statistics database.
//!
//! Nothing here is visible while a server runs: every one of these failures is a
//! piece of state that lives only in memory, and the session that meets it is
//! the one that was open when the server went down. A session is picked up again
//! after a restart by its id, which is what Claude Code does: the harness knows
//! nothing about the server having restarted and goes on sending the same
//! session's events.

mod common;
mod scenario;

use scenario::{Store, World, bash};
use serde_json::json;

/// The machine these scenarios run on, which is the machine the example store's
/// session memories are written for.
const ALPHA: &str = "alpha";

/// The session that is open across the restart, named rather than numbered so
/// that the same context is addressed before and after.
const OPEN_SESSION: &str = "session-1";

/// A second session, which is what a change landed by the first has to reach.
const OTHER_SESSION: &str = "session-2";

/// The memory written on the branch that is open across the restart: global and
/// critical, so that another session is owed it in full at its next event, and
/// named after nothing in the example store.
const BRANCH_MEMORY: &str = "bench-lighting";
const BRANCH_MEMORY_BODY: &str = "# Light the bench before wiring\n\n\
     The wall switch in the corner lights the bench; the overhead lamp is on the \
     same rail as the supply.\n";

/// The name the user gives the open session, distinctive enough that a name read
/// back cannot have come from a prompt, the store or the transcript's padding.
const SESSION_TITLE: &str = "Bracket rework, second pass";

/// Detects a delivery record that lives only in memory: after a restart the
/// session would be given every rule it already holds, at every event, and a
/// critical memory arriving again would stop its next tool call.
///
/// The session is delivered `bench-power` at its start and `widget-naming` at
/// its prompt, and goes on working in `widgets` and the `rocketry` it implies.
/// The server is stopped and started on the same store and state file, and the
/// session carries on: nothing changed in the store, and the context holds what
/// it held, so its next prompt is owed nothing and its next tool call is not
/// stopped. The scopes are asked for separately, because a record that survived
/// while the scopes did not would also deliver nothing here, and for the wrong
/// reason: there would be nothing due at all.
#[test]
fn a_session_continues_across_a_server_restart_without_re_delivery() {
    let mut world = World::new().build();
    {
        let session = world.claude(ALPHA).session_named(OPEN_SESSION);
        session
            .start_in("/home/dev/widgets")
            .assert(|answer| assert!(answer.delivers_full("bench-power")));
        session
            .prompt("rename the widget brackets")
            .assert(|answer| assert!(answer.delivers_full("widget-naming")));
    }

    world.restart();

    let session = world.claude(ALPHA).session_named(OPEN_SESSION);
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
                "the context holds every rule already, and the store did not change"
            );
        });
    session.tool("Bash", bash("cargo build")).assert(|answer| {
        assert!(
            answer.allowed(),
            "nothing is new, so no critical memory stops the call"
        );
    });
}

/// Detects a restart that drops what the contexts page and the statistics page
/// are built from: the pages would come back empty, or a session would come back
/// under its key alone, and the record of what was ever delivered would restart
/// from zero at every server upgrade.
///
/// The name is not in any hook event: the client reads it from the transcript
/// and sends it beside the event, so it reaches the server at the first event
/// after the rename, and it is the name the page shows for the session from
/// then on. Statistics are asked about `bench-power`, which the session start
/// delivered, so there is a row to lose.
#[test]
fn names_and_statistics_survive_a_restart() {
    let mut world = World::new().build();
    let key = format!("{ALPHA}/{OPEN_SESSION}");
    {
        let session = world.claude(ALPHA).session_named(OPEN_SESSION);
        session.start_in("/home/dev/widgets");
        session.rename(SESSION_TITLE);
        session.prompt("carry on where we left off");
        assert_eq!(
            world.context(&key)["name"],
            json!(SESSION_TITLE),
            "the name reaches the page at the first event after the rename"
        );
    }

    world.restart();

    assert_eq!(
        world.context(&key)["name"],
        json!(SESSION_TITLE),
        "the name the page shows must be the one the session was given"
    );
    let deliveries = world.deliveries_of("bench-power");
    assert_eq!(
        deliveries
            .iter()
            .map(|row| row["event"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["SessionStart"],
        "the delivery made before the restart is in the log after it, got {deliveries:?}"
    );
}

/// Detects a branch that exists only in the server's memory: the work on it
/// would be lost by a restart, and the model would be told a branch it opened
/// does not exist. A branch is the one place several writes wait to become one
/// commit, so losing it loses every write on it.
///
/// The branch is opened and written to before the restart and landed after it,
/// by the tool that takes the branch's name. Landing is what makes the writes
/// visible, so the other session's next event delivering the memory in full is
/// the proof that the branch came back whole rather than as an empty name: an
/// empty branch lands without complaint and delivers nothing.
#[test]
fn a_branch_open_before_a_restart_lands_after_it() {
    let mut world = World::new().store(Store::example()).build();
    let branch_name = {
        let claude = world.claude(ALPHA);
        let writer = claude.session_named(OPEN_SESSION);
        let other = claude.session_named(OTHER_SESSION);
        writer.start_in("/home/dev/notes");
        other.start_in("/home/dev/notes");

        let mcp = writer.mcp();
        let branch = mcp.branch();
        let name = branch.name().to_string();
        branch
            .memory_put(BRANCH_MEMORY, |memory| {
                memory
                    .critical()
                    .description("The wall switch in the corner lights the bench")
                    .body(BRANCH_MEMORY_BODY);
            })
            .expect_ok();
        name
    };

    world.restart();

    let claude = world.claude(ALPHA);
    let writer = claude.session_named(OPEN_SESSION);
    writer
        .mcp()
        .call(
            "branch_land",
            json!({
                "session_key": writer.key(),
                "branch": branch_name,
                "message": "light the bench",
            }),
        )
        .expect_ok();

    let other = claude.session_named(OTHER_SESSION);
    other.prompt("carry on where we left off").assert(|answer| {
        assert!(
            answer.delivers_full(BRANCH_MEMORY),
            "the branch came back with the write on it, and landing made it everyone's"
        );
    });
}
