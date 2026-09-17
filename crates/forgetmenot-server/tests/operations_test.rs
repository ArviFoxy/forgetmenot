//! Tests of the session operations and of fetching a memory for a context.
//!
//! These are the operations the MCP tools will be adapters over; they have no
//! HTTP route of their own, so they are called at the library interface on a
//! running server, and what they did is asserted through the next hook event,
//! which is where a session sees the effect.
//!
//! The expectations come from the plan's session-management tools and state
//! machine, and from the example store committed in this repository.

mod common;

use std::collections::BTreeSet;

use forgetmenot_server::context::ContextKey;
use forgetmenot_server::operations;
use forgetmenot_server::stats::Table;
use forgetmenot_server::store::{MemoryId, ScopeId};
use serde_json::{Value, json};

use common::{TestServer, additional_context, example_store_files, hook_fixture};

/// The context size reported with the hook events used here, which are not about
/// staleness.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// The machine every session in these tests runs on.
const MACHINE: &str = "alpha";

// Lines that appear in one memory's body of the example store and nowhere else,
// so that "delivered in full" can be told apart from "named in an index line".
const WIDGET_NAMING_BODY: &str = "Downstream drawings cite part numbers by value";
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";
/// The index entry of the session memory in the example store's silo.
const SESSION_NOTES_DESCRIPTION: &str =
    "Working notes for the bracket rework, measured in millimetres";
/// The index entry of the memory in the `rocketry` scope, which the `widgets`
/// scope of the example store implies.
const ROCKET_STAGES_DESCRIPTION: &str =
    "Stage one lights on the pad and later stages count upwards in firing order";

/// The text a hook answer injects, or the empty string when it injects nothing.
fn context_of(answer: &Value) -> &str {
    additional_context(answer).unwrap_or_default()
}

/// One hook payload with some fields replaced, so that a fixture can be sent for
/// another session than the one it was recorded for.
fn event_with(fixture: &str, fields: &[(&str, Value)]) -> Value {
    let mut event = hook_fixture(fixture);
    let object = event.as_object_mut().expect("a hook payload is an object");
    for (key, value) in fields {
        object.insert((*key).to_string(), value.clone());
    }
    event
}

/// An event that matches no trigger in the example store, used to ask a context
/// what it is owed without changing which scopes it works in.
fn neutral_event(session_id: &str) -> Value {
    event_with(
        "pre_tool_use_read",
        &[
            ("session_id", json!(session_id)),
            ("tool_name", json!("Read")),
            (
                "tool_input",
                json!({ "file_path": "/home/dev/notes/README.md" }),
            ),
            ("cwd", json!("/home/dev/notes")),
        ],
    )
}

/// Whether `text` carries a line saying `id` was withdrawn for `reason`.
fn has_retracted_line(text: &str, id: &str, reason: &str) -> bool {
    text.lines()
        .any(|line| line.contains(id) && line.contains(reason))
}

/// Whether `text` carries an index line for `id` and its description.
fn has_index_line(text: &str, id: &str, description: &str) -> bool {
    text.lines()
        .any(|line| line.contains(id) && line.contains(description))
}

/// Detects a scope turned on for a session that never reaches delivery: the
/// model asked to work in a scope and would get none of its memories.
#[test]
fn a_scope_turned_on_for_a_session_delivers_that_scopes_memories_at_the_next_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));

    let scopes = server
        .run(operations::session_scope_on(
            &state,
            &key,
            &[ScopeId::new("widgets")],
        ))
        .expect("widgets is a scope of the example store");

    assert!(
        scopes.active.contains(&ScopeId::new("widgets")),
        "the scope must be reported as active, got {:?}",
        scopes.active
    );
    assert!(
        scopes.active.contains(&ScopeId::new("rocketry")),
        "the scopes widgets implies must be active too, got {:?}",
        scopes.active
    );
    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    assert!(
        context_of(&answer).contains(WIDGET_NAMING_BODY),
        "the scope's critical memory must be delivered in full, got {answer}"
    );
}

/// Detects a scope that cannot be turned off, or one turned off without the
/// session being told: the model would go on acting on a rule it was told to
/// stop working under.
#[test]
fn a_scope_turned_off_is_reported_as_withdrawn_at_the_next_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");
    let widgets = ScopeId::new("widgets");
    server
        .run(operations::session_scope_on(
            &state,
            &key,
            std::slice::from_ref(&widgets),
        ))
        .expect("widgets is a scope of the example store");
    let delivered = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert!(
        context_of(&delivered.1).contains(WIDGET_NAMING_BODY),
        "the memory has to have been delivered before it can be withdrawn, got {delivered:?}"
    );

    let scopes = server
        .run(operations::session_scope_off(
            &state,
            &key,
            std::slice::from_ref(&widgets),
        ))
        .expect("a scope with a file can be turned off");

    assert!(
        !scopes.active.contains(&widgets),
        "the scope must no longer be active, got {:?}",
        scopes.active
    );
    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&answer);
    assert!(
        has_retracted_line(text, "widget-naming", "no longer in an active scope"),
        "the memory must be reported as withdrawn because the scope was turned off, got {text:?}"
    );
    assert!(
        !text.contains(WIDGET_NAMING_BODY),
        "a withdrawn memory must not be delivered again, got {text:?}"
    );
}

/// Detects a list of scopes of which only one is turned off: the session would
/// go on receiving the memories of a scope it asked to be taken out of, and the
/// answer would report it as off. Source: the ticket for the list form of the
/// session scope tools.
#[test]
fn two_scopes_turned_off_in_one_call_are_both_withdrawn_at_the_next_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");
    let widgets = ScopeId::new("widgets");
    let rocketry = ScopeId::new("rocketry");
    // `widgets` implies `rocketry`, so both scopes are on and both have a memory
    // delivered before anything is turned off.
    server
        .run(operations::session_scope_on(
            &state,
            &key,
            std::slice::from_ref(&widgets),
        ))
        .expect("widgets is a scope of the example store");
    let delivered = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    let before = context_of(&delivered.1);
    assert!(
        before.contains(WIDGET_NAMING_BODY),
        "the widgets memory has to have been delivered before it can be withdrawn, got {before:?}"
    );
    assert!(
        has_index_line(before, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "the rocketry memory has to have been delivered before it can be withdrawn, got {before:?}"
    );

    let scopes = server
        .run(operations::session_scope_off(
            &state,
            &key,
            &[widgets.clone(), rocketry.clone()],
        ))
        .expect("both scopes have a file and can be turned off");

    assert!(
        !scopes.active.contains(&widgets),
        "widgets must no longer be active, got {:?}",
        scopes.active
    );
    assert!(
        !scopes.active.contains(&rocketry),
        "rocketry must no longer be active, got {:?}",
        scopes.active
    );
    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&answer);
    assert!(
        has_retracted_line(text, "widget-naming", "no longer in an active scope"),
        "the widgets memory must be reported as withdrawn, got {text:?}"
    );
    assert!(
        has_retracted_line(text, "rocket-stages", "no longer in an active scope"),
        "the rocketry memory must be reported as withdrawn, got {text:?}"
    );
}

/// Detects an implicit scope that can be turned off: a session without `global`,
/// its machine or its own session scope could not be delivered to at all, and
/// nothing would ever turn those back on.
#[test]
fn turning_off_a_scope_that_is_always_on_is_refused() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));

    for scope in [
        ScopeId::global(),
        ScopeId::machine(MACHINE),
        ScopeId::session(MACHINE, "session-1"),
    ] {
        let refused = server.run(operations::session_scope_off(
            &state,
            &key,
            std::slice::from_ref(&scope),
        ));
        assert!(
            refused.is_err(),
            "turning off {scope} must be refused, got {:?}",
            refused.map(|scopes| scopes.active)
        );
    }

    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    assert!(
        !has_retracted_line(
            context_of(&answer),
            "bench-power",
            "no longer in an active scope"
        ),
        "nothing may be withdrawn by a refused request, got {answer}"
    );
}

/// Detects a list that turns off the scopes it may before refusing the one it may
/// not: the session would be told the call failed while having lost scopes it is
/// never told about. Source: the ticket for the list form of the session scope
/// tools.
#[test]
fn a_list_naming_a_scope_that_is_always_on_turns_none_of_the_others_off() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");
    let widgets = ScopeId::new("widgets");
    server
        .run(operations::session_scope_on(
            &state,
            &key,
            std::slice::from_ref(&widgets),
        ))
        .expect("widgets is a scope of the example store");

    let refused = server.run(operations::session_scope_off(
        &state,
        &key,
        &[widgets.clone(), ScopeId::global()],
    ));

    assert!(
        refused.is_err(),
        "a list naming the global scope must be refused, got {:?}",
        refused.map(|scopes| scopes.active)
    );
    let after = server
        .run(operations::session_scopes(&state, &key))
        .expect("the store is readable");
    assert!(
        after.active.contains(&widgets),
        "the scope named beside it must still be active, got {:?}",
        after.active
    );
    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    assert!(
        context_of(&answer).contains(WIDGET_NAMING_BODY),
        "the memory of the scope that is still on must be delivered, got {answer}"
    );
}

/// Detects an empty list taken as a change of nothing: the call would be answered
/// as done, and the model would read the answer as the scopes it asked for.
/// Source: the ticket for the list form of the session scope tools.
#[test]
fn a_scope_change_that_names_no_scope_is_refused() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");

    let refused_on = server.run(operations::session_scope_on(&state, &key, &[]));
    let refused_on = refused_on
        .err()
        .unwrap_or_else(|| panic!("turning on no scope at all must be refused"));
    assert!(
        refused_on.to_string().contains("names no scope"),
        "the refusal must say that the list names no scope, got {refused_on}"
    );

    let refused_off = server.run(operations::session_scope_off(&state, &key, &[]));
    let refused_off = refused_off
        .err()
        .unwrap_or_else(|| panic!("turning off no scope at all must be refused"));
    assert!(
        refused_off.to_string().contains("names no scope"),
        "the refusal must say that the list names no scope, got {refused_off}"
    );
}

/// Detects inheritance that copies nothing, or that copies the scopes without the
/// other session's own scope: the point of inheriting is that the other session's
/// notes become readable here, and those live in its session scope alone.
#[test]
fn inheriting_another_session_makes_its_own_memories_due_here() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    // The silo in the example store belongs to this session, so this is the
    // session whose notes another one can inherit.
    let source = ContextKey::main(MACHINE, "session-1");
    let caller = ContextKey::main(MACHINE, "session-2");
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    server
        .run(operations::session_scope_on(
            &state,
            &source,
            &[ScopeId::new("widgets")],
        ))
        .expect("widgets is a scope of the example store");

    let scopes = server
        .run(operations::session_inherit(&state, &caller, &source))
        .expect("the source session has been seen by this server");

    assert!(
        scopes.active.contains(&ScopeId::new("widgets")),
        "the caller must take over the source's scopes, got {:?}",
        scopes.active
    );
    assert!(
        scopes.active.contains(&source.session_scope()),
        "the caller must take over the source's own session scope, got {:?}",
        scopes.active
    );
    let (status, answer) = server.hook(
        MACHINE,
        SOME_TOKENS,
        &event_with(
            "session_start",
            &[
                ("session_id", json!("session-2")),
                ("source", json!("resume")),
            ],
        ),
    );
    assert_eq!(status, 200, "the event must be answered");
    assert!(
        has_index_line(
            context_of(&answer),
            "sessions/alpha/session-1/notes",
            SESSION_NOTES_DESCRIPTION
        ),
        "the source session's notes must be due in the caller, got {answer}"
    );
}

/// Detects inheriting from a session nobody has seen: a mistyped session key
/// would otherwise create an empty context and quietly inherit nothing.
#[test]
fn inheriting_a_session_this_server_has_never_seen_is_refused() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let caller = ContextKey::main(MACHINE, "session-2");

    let refused = server.run(operations::session_inherit(
        &state,
        &caller,
        &ContextKey::main(MACHINE, "no-such-session"),
    ));

    assert!(
        refused.is_err(),
        "inheriting from an unknown session must be refused, got {:?}",
        refused.map(|scopes| scopes.active)
    );
}

/// Detects an offer of scopes the model cannot act on: the implicit scopes are on
/// by construction, and a scope already active is not something to turn on.
#[test]
fn the_scopes_offered_to_a_session_are_the_ones_it_is_not_already_working_in() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");

    let before = server
        .run(operations::session_scopes(&state, &key))
        .expect("the store is readable");
    // Compared as a set: which scopes are offered is the guarantee, the order
    // they are listed in is not.
    assert_eq!(
        before.available.iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            &ScopeId::new("rocketry"),
            &ScopeId::new("widgets"),
            &ScopeId::new("workshop")
        ]),
        "every scope with a file that is not active must be offered"
    );
    assert!(
        before.active.contains(&ScopeId::global()),
        "a session works in the global scope from the start, got {:?}",
        before.active
    );

    server
        .run(operations::session_scope_on(
            &state,
            &key,
            &[ScopeId::new("workshop")],
        ))
        .expect("workshop is a scope of the example store");

    let after = server
        .run(operations::session_scopes(&state, &key))
        .expect("the store is readable");
    assert!(
        !after.available.contains(&ScopeId::new("workshop")),
        "a scope that is now active must not be offered again, got {:?}",
        after.available
    );
}

/// Detects a fetch that is not recorded as delivered: the memory the model just
/// read in full would be delivered to it again at the very next event, and the
/// only thing that should bring it back is a change to it.
#[test]
fn a_memory_fetched_for_a_context_comes_back_only_when_it_changes() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");
    let id = MemoryId::new("bench-power");

    let fetched = server
        .run(operations::memory_get(&state, &id, Some(&key)))
        .expect("the memory is in the example store");
    assert!(
        fetched.body.contains(BENCH_POWER_BODY),
        "the fetch must return the body, got {fetched:?}"
    );

    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the event must be answered");
    assert!(
        !context_of(&answer).contains(BENCH_POWER_BODY),
        "a memory the context has just fetched must not be delivered again, got {answer}"
    );

    server.commit(
        "tighten the bench power rule",
        vec![(
            "memories/bench-power.md".to_string(),
            Some(
                b"---\nname: bench-power\ndescription: Cut bench power at the wall before rewiring and confirm with the meter\nmetadata:\n  kind: critical\n  scopes:\n  - global\n  source: user\n---\n# Cut bench power before rewiring\n\nThe rule now also covers the charger bench.\n"
                    .to_vec(),
            ),
        )],
    );
    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the event must be answered");
    assert!(
        context_of(&answer).contains("The rule now also covers the charger bench."),
        "the changed memory must be delivered again, got {answer}"
    );
}

/// Detects a session operation that is not recorded in the statistics: the
/// statistics answer which session turned what on, and a tool call that leaves no
/// row is invisible there. Also detects a row per scope id of a call that names
/// several, which would count one call as several.
#[test]
fn every_session_operation_records_one_tool_call() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let source = ContextKey::main(MACHINE, "session-1");
    let caller = ContextKey::main(MACHINE, "session-2");
    // Two scopes in the one call, because a call that names several is still one
    // tool call.
    let two_scopes = [ScopeId::new("widgets"), ScopeId::new("workshop")];
    assert_eq!(
        server.stats().count_rows(Table::ToolCalls),
        0,
        "no tool call has been made yet"
    );

    server
        .run(operations::session_scopes(&state, &source))
        .expect("the store is readable");
    server
        .run(operations::session_scope_on(&state, &source, &two_scopes))
        .expect("both are scopes of the example store");
    server
        .run(operations::session_scope_off(&state, &source, &two_scopes))
        .expect("scopes with a file can be turned off");
    server
        .run(operations::session_inherit(&state, &caller, &source))
        .expect("the source session has been seen by this server");

    assert_eq!(
        server.stats().count_rows(Table::ToolCalls),
        4,
        "each of the four session operations must record exactly one tool call"
    );
}
