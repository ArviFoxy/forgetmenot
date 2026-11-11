//! Keeping a scope off for the rest of a session: `session_scope_off` with
//! `keep_off`, held against every way a scope comes on again over a session.
//!
//! The example store's `widgets` comes on when the user names a widget or a tool
//! is called on widgets, and it implies `rocketry`. Its rule `widget-naming` is
//! critical, so whether it is delivered shows whether the scope is on.

mod common;
mod scenario;

use scenario::{Store, World, bash};
use serde_json::json;

/// The machine every scenario here runs on.
const ALPHA: &str = "alpha";

/// A directory no trigger of the example store recognises.
const QUIET: &str = "/home/dev/parts";

/// A prompt naming a widget, which `widgets`'s user-message pattern matches.
const WIDGET_PROMPT: &str = "check the widget part numbers on the bracket";

/// A later prompt naming a widget again.
const WIDGET_AGAIN: &str = "and the widget labels on the shelf";

/// A command `widgets`'s tool-input pattern matches.
const WIDGET_COMMAND: &str = "grep -rn widgets ./parts-list";

/// Detects `keep_off` being ignored, so that a later prompt or tool call that
/// matches the scope's trigger turns it on again and its rule arrives, and a
/// hold that a compaction drops, which would bring the scope back after the
/// conversation is rebuilt. Detects too a held-off match that leaves no trace in
/// the activation log: the log is where the user sees what was held off.
#[test]
fn a_scope_kept_off_stays_off_when_its_triggers_match_again() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session.prompt(WIDGET_PROMPT).assert(|answer| {
        assert!(
            answer.delivers_full("widget-naming"),
            "the prompt turns widgets on"
        );
    });
    let scopes = session
        .mcp()
        .session_scope_keep_off(["widgets"])
        .expect_ok();
    assert_eq!(
        scopes.json()["kept_off"],
        json!(["widgets"]),
        "the session reports the scope it keeps off"
    );

    session.prompt(WIDGET_AGAIN).assert(|answer| {
        assert!(
            !answer.delivers_full("widget-naming"),
            "the prompt matches widgets, which the session keeps off"
        );
    });
    session
        .tool("Bash", bash(WIDGET_COMMAND))
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "nothing new is due, so nothing holds the call"
            );
        })
        .run();
    session.compact();
    session.prompt(WIDGET_AGAIN);
    assert!(
        !session.active_scopes().contains("widgets"),
        "the scope stays off through the prompts, the tool call and the compaction, got {:?}",
        session.active_scopes()
    );

    let listed = session
        .mcp()
        .call(
            "scope_activations",
            json!({ "session": session.key(), "scope": "widgets", "order": "oldest" }),
        )
        .expect_ok();
    let entries = listed.json()["entries"]
        .as_array()
        .expect("the answer lists entries")
        .clone();
    let causes: Vec<&str> = entries
        .iter()
        .map(|entry| entry["cause"].as_str().expect("every entry has a cause"))
        .collect();
    assert_eq!(
        causes[0], "trigger",
        "the first prompt turned the scope on, got {causes:?}"
    );
    assert!(
        causes.len() > 1 && causes[1..].iter().all(|cause| *cause == "kept_off"),
        "every later match is logged as held off, got {causes:?}"
    );
    assert!(
        entries[1..]
            .iter()
            .any(|entry| entry["field"] == json!("user_message")
                && entry["evidence"]["matched"] == json!("widget")),
        "a held-off entry carries the field and the text its trigger matched, got {entries:?}"
    );
}

/// Detects `keep_off` applied when it was not asked for: without it a scope
/// turned off comes on again at the next match, as it always has.
#[test]
fn a_scope_turned_off_without_keep_off_comes_on_at_the_next_match() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session.prompt(WIDGET_PROMPT);
    let scopes = session.mcp().session_scope_off(["widgets"]).expect_ok();
    assert_eq!(scopes.json()["kept_off"], json!([]), "nothing is kept off");
    session.prompt(WIDGET_AGAIN).assert(|answer| {
        assert!(
            answer.delivers_full("widget-naming"),
            "the scope was only turned off, so the next match turns it on and its rule arrives"
        );
    });
}

/// Detects a kept-off scope that a scope implying it brings back: implication
/// is applied after the triggers, so a hold checked only against trigger
/// matches would be undone by the closure.
#[test]
fn a_scope_kept_off_stays_off_when_a_scope_that_implies_it_comes_on() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session
        .mcp()
        .session_scope_keep_off(["rocketry"])
        .expect_ok();
    session.prompt(WIDGET_PROMPT).assert(|answer| {
        assert!(
            answer.delivers_full("widget-naming"),
            "widgets itself comes on"
        );
        assert!(
            !answer.delivers_index("rocket-stages") && !answer.delivers_full("rocket-stages"),
            "rocketry, which widgets implies, is kept off, so its memory is not delivered"
        );
    });
    let active = session.active_scopes();
    assert!(
        active.contains("widgets") && !active.contains("rocketry"),
        "widgets is on and rocketry is not, got {active:?}"
    );
}

/// Detects a hold that `session_scope_on` cannot lift, which would leave the
/// session no way back to the scope, and a hold that outlives it, which would
/// keep the scope from coming on by its triggers after the session took it up
/// again.
#[test]
fn naming_a_kept_off_scope_in_session_scope_on_ends_the_hold() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session
        .mcp()
        .session_scope_keep_off(["widgets"])
        .expect_ok();
    let scopes = session.mcp().session_scope_on(["widgets"]).expect_ok();
    assert_eq!(scopes.json()["kept_off"], json!([]), "the hold is lifted");
    assert!(
        session.active_scopes().contains("widgets"),
        "the scope is on"
    );

    session.mcp().session_scope_off(["widgets"]).expect_ok();
    session.prompt(WIDGET_AGAIN).assert(|answer| {
        assert!(
            answer.delivers_full("widget-naming"),
            "with the hold gone, a plain turn-off lasts only until the next match"
        );
    });
}

/// Detects a subagent that turns a scope its session keeps off back on, by
/// inheriting it or with its own triggers: its report would then carry what
/// the scope's memories said into the session that put the scope down.
#[test]
fn a_subagent_started_after_the_hold_keeps_the_scope_off() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session.prompt(WIDGET_PROMPT);
    session
        .mcp()
        .session_scope_keep_off(["widgets"])
        .expect_ok();

    let child = session.subagent("general-purpose", "list the parts on the bench");
    child.tool("Bash", bash(WIDGET_COMMAND)).run();
    assert!(
        !child.active_scopes().contains("widgets"),
        "the child neither inherits nor turns on the scope its session keeps off, got {:?}",
        child.active_scopes()
    );
    let scopes = child.mcp().session_scopes().expect_ok();
    assert_eq!(
        scopes.json()["kept_off"],
        json!(["widgets"]),
        "the child keeps it off too"
    );
}

/// Detects `session_inherit` bringing in a scope the inheriting session keeps
/// off, which would undo the hold through the other session's scopes.
#[test]
fn inheriting_another_sessions_scopes_leaves_a_kept_off_scope_off() {
    let world = World::new().store(Store::example()).build();
    let claude = world.claude(ALPHA);
    let source = claude.session();
    source.start_in(QUIET);
    source.prompt(WIDGET_PROMPT);
    assert!(
        source.active_scopes().contains("widgets"),
        "the other session works in widgets"
    );

    let session = claude.session();
    assert_ne!(session.key(), source.key(), "two sessions, not one");
    session.start_in(QUIET);
    session
        .mcp()
        .session_scope_keep_off(["widgets"])
        .expect_ok();
    assert!(
        source.active_scopes().contains("widgets"),
        "the other session still works in widgets"
    );
    let inherited = session.mcp().session_inherit(&source.key()).expect_ok();
    assert!(
        !inherited.json()["active"]
            .as_array()
            .expect("the answer lists the active scopes")
            .contains(&json!("widgets")),
        "the call does not turn on the scope this session keeps off, got {}",
        inherited.json()
    );
    let active = session.active_scopes();
    assert!(
        !active.contains("widgets") && active.contains(&format!("session:{}", source.key())),
        "the other session's own scope is taken over and widgets is not, got {active:?}"
    );
}
