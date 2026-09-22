//! Tests of the application rules over the store, the registry and the
//! statistics: write semantics and conflicts, what a write records about its
//! own writer, the scope index's sources, the text a context would be given,
//! the order of the contexts, the store's own history, the machines and the
//! settings, and the session operations.
//!
//! Every one of these is called at the library interface on a running server,
//! with no HTTP and no MCP in the way: what the operation answers is asserted
//! directly, and what it changed is asserted through the next hook event, which
//! is where a session sees the effect. The status codes and JSON shapes the
//! same operations get behind a route are `api_test.rs`, and the store
//! behaviour underneath them is `store_test.rs` and `branch_test.rs`.
//!
//! The expectations come from the plan's data model, write contract and state
//! machine, and from the example store committed in this repository.

mod common;

use std::collections::BTreeSet;

use forgetmenot_server::clock::{Clock, FixedClock};
use forgetmenot_server::context::ContextKey;
use forgetmenot_server::operations::branches::LandRequest;
use forgetmenot_server::operations::settings::{SettingsDoc, SettingsWriteRequest};
use forgetmenot_server::operations::{
    self, ContextPrompt, CurrentDocument, DeleteRequest, MemoryDoc, MemoryFilter,
    MemoryWriteRequest, OperationError, PromptMode, ScopeRow, StoreHistory,
};
use forgetmenot_server::stats::{Bucket, Filter, Table, Window};
use forgetmenot_server::store::memory::{MemoryKind, MemorySource};
use forgetmenot_server::store::validate::WriteMode;
use forgetmenot_server::store::{MemoryId, ScopeId, ScopeKind};
use serde_json::{Value, json};

use common::{
    TestServer, additional_context, example_store_files, example_store_without_settings,
    hook_fixture,
};

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
                b"---\nname: bench-power\ndescription: Cut bench power at the wall before rewiring and confirm with the meter\nmetadata:\n  kind: critical\n  scope: global\n  source: user\n---\n# Cut bench power before rewiring\n\nThe rule now also covers the charger bench.\n"
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

// ---------------------------------------------------------------------------
// Write semantics and conflicts
// ---------------------------------------------------------------------------

/// A line that appears in no memory of the example store, so that "the write
/// landed" can be told apart from "the old text is still there".
const NEW_LINE: &str = "The binder lives on the shelf by the door.";

/// One memory as the store has it, failing the test when it cannot be read.
fn memory(server: &TestServer, id: &str) -> MemoryDoc {
    server
        .run(operations::memory_get(
            &server.state(),
            &MemoryId::new(id),
            None,
        ))
        .unwrap_or_else(|error| panic!("reading {id} must succeed, got {error}"))
}

/// A write of `body` to the version `document` was read at.
fn write_of(document: &MemoryDoc, body: &str, message: &str) -> MemoryWriteRequest {
    MemoryWriteRequest {
        description: document.description.clone(),
        kind: document.kind,
        scope: document.scope.clone(),
        source: document.source.clone(),
        metadata: None,
        body: body.to_string(),
        base_version: Some(document.version.clone()),
        author: "wiki".to_string(),
        message: message.to_string(),
    }
}

/// The store's head, which every commit moves.
fn head(server: &TestServer) -> String {
    server
        .store()
        .repository()
        .head_oid()
        .expect("the store has a head")
        .to_string()
}

/// The memory a conflict carries as the one the store has now.
fn conflicting_memory(error: OperationError) -> MemoryDoc {
    match error {
        OperationError::Conflict {
            current: CurrentDocument::Memory(document),
            ..
        } => *document,
        other => panic!("the refusal must be a conflict carrying the memory, got {other}"),
    }
}

/// Detects a write that ignores the version it was made from: two editors
/// saving from the same version would overwrite each other, and the second
/// would never see the first one's text. The document as the store has it has
/// to come back with the refusal, because that is what the second writer needs
/// in order to write again.
#[test]
fn a_memory_written_from_a_stale_version_is_refused_with_the_current_document_and_makes_no_commit()
{
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let stale = memory(&server, "reading-list");
    let first_body = "# Where the workshop references live\n\nthe first writer's text\n";
    server
        .run(operations::memory_put(
            &state,
            &MemoryId::new("reading-list"),
            &write_of(&stale, first_body, "write from the first editor"),
            WriteMode::Update,
            None,
        ))
        .expect("the first write is made from the version the store has");
    let after_first = head(&server);

    let refused = server
        .run(operations::memory_put(
            &state,
            &MemoryId::new("reading-list"),
            &write_of(
                &stale,
                "# Where the workshop references live\n\nthe second writer's text\n",
                "write from the second editor",
            ),
            WriteMode::Update,
            None,
        ))
        .expect_err("a write from a stale version must be refused");

    let current = conflicting_memory(refused);
    assert_eq!(
        current.body, first_body,
        "the refusal must carry the document as the store has it"
    );
    assert_ne!(
        current.version, stale.version,
        "the refusal must carry the version that made the write stale"
    );
    assert_eq!(
        head(&server),
        after_first,
        "a refused write must leave the store as it was"
    );
}

/// Detects a create that overwrites the memory already at that id, which would
/// silently replace someone else's memory with a new one. A caller that finds
/// the id taken is in the same situation as one holding a stale version, so it
/// is answered the same way: with the document that is there.
#[test]
fn creating_a_memory_at_an_id_that_is_taken_is_refused_with_the_document_that_is_there() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let before = head(&server);

    let refused = server
        .run(operations::memory_put(
            &state,
            &MemoryId::new("reading-list"),
            &MemoryWriteRequest {
                description: "A second memory claiming an id that is taken".to_string(),
                kind: MemoryKind::Knowledge,
                scope: ScopeId::global(),
                source: MemorySource::new("user"),
                metadata: None,
                body: "# Another reading list\n\ntext\n".to_string(),
                base_version: None,
                author: "wiki".to_string(),
                message: "create a second reading list".to_string(),
            },
            WriteMode::Create,
            None,
        ))
        .expect_err("creating over an existing id must be refused");

    assert_eq!(
        conflicting_memory(refused).id,
        MemoryId::new("reading-list"),
        "the refusal must carry the memory that is already there"
    );
    assert_eq!(
        head(&server),
        before,
        "a refused create must leave the store as it was"
    );
}

/// Detects a create that does not record when the memory was made, and an index
/// filter that ignores the scope or the kind it was given: a memory would then
/// be listed under every scope, and the page that offers one scope's memories
/// would offer the whole store.
#[test]
fn a_created_memory_is_stamped_and_the_index_filter_lists_it_only_under_its_own_scope_and_kind() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();

    server
        .run(operations::memory_put(
            &state,
            &MemoryId::new("bracket-tolerances"),
            &MemoryWriteRequest {
                description: "Bracket holes are reamed after welding, not before".to_string(),
                kind: MemoryKind::Knowledge,
                scope: ScopeId::new("widgets"),
                source: MemorySource::new("user"),
                metadata: None,
                body: "# Bracket tolerances\n\nReam the holes after welding.\n".to_string(),
                base_version: None,
                author: "wiki".to_string(),
                message: "record the bracket tolerance".to_string(),
            },
            WriteMode::Create,
            None,
        ))
        .expect("the id is free and the document is valid");

    assert!(
        memory(&server, "bracket-tolerances").created.is_some(),
        "a created memory must record when it was made"
    );
    let catalog = server.store().catalog();
    let listed = |filter: MemoryFilter| -> Vec<MemoryId> {
        operations::memory_index(&catalog, &filter)
            .into_iter()
            .map(|summary| summary.id)
            .collect()
    };
    let created = MemoryId::new("bracket-tolerances");
    assert!(
        listed(MemoryFilter {
            scope: Some(ScopeId::new("widgets")),
            kind: None,
        })
        .contains(&created),
        "a memory must be listed under the scope it names"
    );
    assert!(
        !listed(MemoryFilter {
            scope: Some(ScopeId::new("rocketry")),
            kind: None,
        })
        .contains(&created),
        "a memory must not be listed under a scope it does not name"
    );
    assert!(
        !listed(MemoryFilter {
            scope: None,
            kind: Some(MemoryKind::Critical),
        })
        .contains(&created),
        "a knowledge memory must not be listed under the critical kind"
    );
}

/// Detects a delete that ignores the version it was made from: an editor
/// holding a memory as it was before someone else rewrote it would remove text
/// it never saw, and the person who wrote that text would have no way to find
/// out.
#[test]
fn deleting_a_memory_from_a_stale_version_is_refused_and_leaves_it_in_the_store() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let stale = memory(&server, "reading-list");
    server
        .run(operations::memory_put(
            &state,
            &MemoryId::new("reading-list"),
            &write_of(
                &stale,
                &format!("# Where the workshop references live\n\n{NEW_LINE}\n"),
                "write from the other editor",
            ),
            WriteMode::Update,
            None,
        ))
        .expect("the other editor's write is made from the version the store has");
    let after_write = head(&server);

    let refused = server
        .run(operations::memory_delete(
            &state,
            &MemoryId::new("reading-list"),
            &DeleteRequest {
                base_version: stale.version.clone(),
                author: "wiki".to_string(),
                message: "the reading list is not needed any more".to_string(),
            },
            None,
        ))
        .expect_err("a delete from a stale version must be refused");

    assert_ne!(
        conflicting_memory(refused).version,
        stale.version,
        "the refusal must carry the document as the store has it"
    );
    assert_eq!(
        head(&server),
        after_write,
        "a refused delete must leave the store as it was"
    );
    assert!(
        memory(&server, "reading-list").body.contains(NEW_LINE),
        "a refused delete must leave the other editor's text in the store"
    );
}

/// Detects a write recorded against more than the memories it touched, and one
/// recorded against none: the writer composed what it wrote and already holds
/// it, so delivering it back spends its context twice, while a change the
/// writer did not make must still reach it. A memory that is gone must be
/// forgotten rather than retracted, because the writer is the one that removed
/// it.
///
/// The expectation is the one issue 11 states: what a write records is the
/// memories that write touched.
#[test]
fn note_own_writes_records_the_memories_it_names_and_forgets_one_that_is_gone() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");
    let (_, start) = server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        context_of(&start).contains(BENCH_POWER_BODY),
        "the session start must deliver the memories before any of them change, got {start}"
    );

    // Two changes the session did not make: one memory rewritten, one removed.
    server.commit(
        "rewrite two memories behind the server",
        vec![
            (
                "memories/bench-power.md".to_string(),
                Some(
                    b"---\nname: bench-power\ndescription: Cut bench power at the wall before rewiring and confirm with the meter\nmetadata:\n  kind: critical\n  scope: global\n  source: user\n---\n# Cut bench power before rewiring\n\nThe rule now also covers the charger bench.\n"
                        .to_vec(),
                ),
            ),
            ("memories/reading-list.md".to_string(), None),
        ],
    );

    server.run(operations::note_own_writes(
        &state,
        &key,
        &[MemoryId::new("bench-power"), MemoryId::new("reading-list")],
    ));

    let (status, answer) = server.hook(MACHINE, SOME_TOKENS, &neutral_event("session-1"));
    assert_eq!(status, 200, "the next event must be answered");
    let text = context_of(&answer);
    assert!(
        !text.contains("The rule now also covers the charger bench."),
        "a memory the context is recorded as having written must not be delivered back, got {text:?}"
    );
    assert!(
        !has_retracted_line(text, "reading-list", "deleted"),
        "a memory the context is recorded as having removed must not be retracted to it, got \
         {text:?}"
    );
    assert!(
        text.is_empty(),
        "nothing else may be owed after both changes were recorded as the context's own, got \
         {text:?}"
    );
}

// ---------------------------------------------------------------------------
// The scope index
// ---------------------------------------------------------------------------

/// The scope index of the running server, by id.
fn scope_rows(server: &TestServer) -> Vec<ScopeRow> {
    server
        .run(operations::scope_index(&server.state()))
        .expect("the scope index is readable")
}

/// The ids the scope index reports, in the order it reports them.
fn scope_ids(rows: &[ScopeRow]) -> Vec<String> {
    rows.iter().map(|row| row.id.to_string()).collect()
}

/// The row of one scope in an index, failing the test when it is not there.
fn scope_row<'rows>(rows: &'rows [ScopeRow], id: &str) -> &'rows ScopeRow {
    rows.iter()
        .find(|row| row.id.to_string() == id)
        .unwrap_or_else(|| panic!("{id} must be listed, got {:?}", scope_ids(rows)))
}

/// The name the user gave the session of the scope index tests, which is what
/// the index reports for that session's scope.
const INDEX_SESSION_TITLE: &str = "Bracket rework on the vacuum former";

/// Detects an index that reports one family of scopes only: a page that offers
/// the file-backed scopes alone cannot show what a session is working under,
/// and one that reports no kind cannot tell a scope that has a file to edit
/// from one that has none. It also detects an id invented for a row, which
/// would offer a scope that no file and no context names.
///
/// Source: a scope with a file exists by its file, `global` exists always, and
/// a machine or a session scope exists once the server has seen that machine or
/// that session or a store file names it. The example store has three scope
/// files and a memory in the silo of `alpha/session-1`; the event below is the
/// only one this server has ever had.
#[test]
fn the_scope_index_lists_every_existing_scope_once_with_its_kind_and_its_file() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook_named(
        MACHINE,
        SOME_TOKENS,
        Some(INDEX_SESSION_TITLE),
        None,
        &hook_fixture("session_start"),
    );

    let rows = scope_rows(&server);

    assert_eq!(
        scope_ids(&rows),
        vec![
            "global",
            "machine:alpha",
            "rocketry",
            "session:alpha/session-1",
            "widgets",
            "workshop",
        ],
        "the index must list every scope that exists, each once and in order of id, and \
         nothing else"
    );
    let global = scope_row(&rows, "global");
    assert_eq!(
        (global.kind, global.file.is_some()),
        (ScopeKind::Global, false),
        "the scope every session starts in must be reported as such and carry no file"
    );
    assert_eq!(
        scope_row(&rows, "machine:alpha").kind,
        ScopeKind::Machine,
        "the scope of the machine that sent the event must be reported as a machine"
    );
    let session = scope_row(&rows, "session:alpha/session-1");
    assert_eq!(
        (session.kind, session.name.as_deref()),
        (ScopeKind::Session, Some(INDEX_SESSION_TITLE)),
        "the scope of a session with a live context must be reported as a session and named as \
         the contexts page names it"
    );
    for id in ["rocketry", "widgets", "workshop"] {
        let row = scope_row(&rows, id);
        let file = row
            .file
            .as_ref()
            .unwrap_or_else(|| panic!("{id} has a file and its row must carry it"));
        assert_eq!(row.kind, ScopeKind::File, "{id} must be reported as a file");
        assert!(
            !file.version.is_empty(),
            "{id}'s row must carry the version a write goes against"
        );
    }
    assert_eq!(
        scope_row(&rows, "widgets")
            .file
            .as_ref()
            .expect("widgets has a file")
            .implies,
        vec![ScopeId::new("rocketry")],
        "a file row must carry what the scope file says"
    );
}

/// Detects a machine or a session list read from the events alone: a memory
/// written for a machine that has sent nothing, and the silo of a session that
/// is over, both name scopes that exist, and an index without them would report
/// those memories as belonging to no scope in the store. It also detects a name
/// invented for a session nothing is known about.
///
/// Source: a machine or a session scope exists once the server has seen that
/// machine or that session or a store file names it. No event at all reaches
/// this server, and the example store carries the memory
/// `sessions/alpha/session-1/notes`.
#[test]
fn the_scope_index_lists_a_machine_and_a_session_that_only_a_store_file_names() {
    let mut files = example_store_files();
    files.push((
        "memories/lathe-coolant.md".to_owned(),
        Some(
            concat!(
                "---\n",
                "name: lathe-coolant\n",
                "description: The lathe on gamma runs on neat cutting oil, never emulsion\n",
                "metadata:\n",
                "  kind: knowledge\n",
                "  scope: machine:gamma\n",
                "  source: user\n",
                "---\n",
                "# The lathe on gamma runs on neat cutting oil\n",
                "\n",
                "The lathe takes neat cutting oil; emulsion is for the mill.\n",
            )
            .as_bytes()
            .to_vec(),
        ),
    ));
    let server = TestServer::start(files, |_| {});

    let rows = scope_rows(&server);

    assert_eq!(
        scope_row(&rows, "machine:gamma").kind,
        ScopeKind::Machine,
        "the scope of the machine a memory names must be listed as a machine"
    );
    let session = scope_row(&rows, "session:alpha/session-1");
    assert_eq!(
        (session.kind, session.name.as_deref()),
        (ScopeKind::Session, None),
        "the scope of the session a silo belongs to must be listed, and a session with no live \
         context has nothing to be named by"
    );
}

/// Detects an index that takes a context's active set as a source of scopes: a
/// scope that was deleted has no file and nothing else names it, so it exists
/// no longer, and an index that listed it would offer a scope that cannot be
/// read, edited or deleted.
///
/// Source: a reference never creates a scope of the file kind. `workshop` is
/// the example store's scope that no memory lists and no other scope implies,
/// and its directory trigger turns it on for a session on `alpha`.
#[test]
fn the_scope_index_drops_a_deleted_scope_although_a_context_still_has_it_on() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let workshop = ScopeId::new("workshop");
    server
        .run(operations::session_scope_on(
            &state,
            &ContextKey::main(MACHINE, "session-1"),
            std::slice::from_ref(&workshop),
        ))
        .expect("workshop is a scope of the example store");
    let version = scope_rows(&server)
        .into_iter()
        .find(|row| row.id == workshop)
        .and_then(|row| row.file)
        .expect("workshop has a file")
        .version;

    server
        .run(operations::scope_delete(
            &state,
            &workshop,
            &DeleteRequest {
                base_version: version,
                author: "wiki".to_string(),
                message: "the workshop directory moved off this machine".to_string(),
            },
            None,
        ))
        .expect("no memory lists workshop and no scope implies it");

    assert!(
        !scope_ids(&scope_rows(&server)).contains(&"workshop".to_string()),
        "a scope whose file is gone must leave the index, whatever a context still has on"
    );
    assert!(
        server
            .run(operations::session_scopes(
                &state,
                &ContextKey::main(MACHINE, "session-1")
            ))
            .expect("the store is readable")
            .active
            .contains(&workshop),
        "the context must be reported with the scopes it has on, as they stand"
    );
}

/// Detects a scope delete that goes ahead while the store still names the
/// scope: the store would be left with a memory or a scope naming one that does
/// not exist, which `forgetmenot check` calls invalid, and the memory would be
/// due in no context with nothing saying why.
#[test]
fn deleting_a_scope_the_store_still_names_is_refused_naming_the_file_that_names_it() {
    let mut files = example_store_files();
    files.push((
        "scopes/bench.yaml".to_owned(),
        Some(b"id: bench\nimplies: [workshop]\n".to_vec()),
    ));
    let server = TestServer::start(files, |_| {});
    let state = server.state();
    let catalog = server.store().catalog();
    let before = head(&server);

    // `widget-naming` is the example store's memory scoped to `widgets`, and
    // the scope file added above is the only one that implies `workshop`.
    for (scope, blocking_file) in [
        ("widgets", "memories/widget-naming.md"),
        ("workshop", "scopes/bench.yaml"),
    ] {
        let id = ScopeId::new(scope);
        let version = operations::scope_get(&catalog, &id)
            .expect("the scope has a file")
            .version;
        let refused = server
            .run(operations::scope_delete(
                &state,
                &id,
                &DeleteRequest {
                    base_version: version,
                    author: "wiki".to_string(),
                    message: format!("{scope} is not used here any more"),
                },
                None,
            ))
            .expect_err("deleting a scope the store still names must be refused");
        let OperationError::Invalid { errors } = refused else {
            panic!("deleting {scope} must be refused as an invalid write, got {refused}");
        };
        assert!(
            errors.iter().any(|error| error.path == blocking_file),
            "the refusal must name {blocking_file}, the file that has to be edited first, got \
             {errors:?}"
        );
    }

    assert_eq!(
        head(&server),
        before,
        "a refused delete must leave the store as it was"
    );
    let ids = scope_ids(&scope_rows(&server));
    assert!(
        ids.contains(&"widgets".to_string()) && ids.contains(&"workshop".to_string()),
        "a refused delete must leave the scope in the index, got {ids:?}"
    );
}

// ---------------------------------------------------------------------------
// What a context would be given next
// ---------------------------------------------------------------------------

/// One context's prompt, failing the test when it cannot be rendered.
fn prompt(server: &TestServer, key: &ContextKey, mode: PromptMode) -> ContextPrompt {
    server
        .run(operations::context_prompt(&server.state(), key, mode))
        .unwrap_or_else(|error| panic!("the prompt of {key} must render, got {error}"))
}

/// Detects a prompt page that shows a person other characters than the model
/// was sent: a session start rendered without its own lines, or without the
/// notice the store puts on a long answer. The whole prompt of a context that
/// has just started is that start's answer, character for character.
///
/// Source: the rule that the page shows the text the hook would answer.
#[test]
fn the_whole_prompt_of_a_context_that_just_started_is_its_session_starts_answer() {
    let server = TestServer::start(example_store_files(), |_| {});
    let key = ContextKey::main(MACHINE, "session-1");
    let (_, started) = server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    let answered = additional_context(&started)
        .expect("a session start of the example store is answered with text")
        .to_string();

    let whole = prompt(&server, &key, PromptMode::All);

    assert_eq!(
        whole.text, answered,
        "the whole prompt must be the very text the session start was answered with"
    );
}

/// How many memories one context is recorded as holding.
fn delivered_count(server: &TestServer, key: &ContextKey) -> usize {
    server
        .run(operations::contexts(
            &server.state().contexts,
            server.state().clock.now(),
        ))
        .into_iter()
        .find(|row| row.key == key.to_string())
        .unwrap_or_else(|| panic!("{key} must be a context"))
        .delivered_count
}

/// Detects a due prompt that repeats what the context already holds, and one
/// read from what the last event delivered rather than from the store as it is
/// now: a session that was just given everything is owed nothing, and a rule
/// rewritten outside the server is owed to it before its next event. Either
/// fault makes the page say the next event costs something other than what it
/// costs.
#[test]
fn the_due_prompt_is_empty_until_the_store_changes_and_then_carries_what_changed() {
    let server = TestServer::start(example_store_files(), |_| {});
    let key = ContextKey::main(MACHINE, "session-1");
    let (_, start) = server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        context_of(&start).contains(BENCH_POWER_BODY),
        "the session start must have delivered everything this session is owed, got {start}"
    );

    let owed_nothing = prompt(&server, &key, PromptMode::Due);
    assert_eq!(
        (owed_nothing.text.as_str(), owed_nothing.bytes),
        ("", 0),
        "a context that is owed nothing must render no text and no bytes"
    );

    let path = "memories/bench-power.md";
    let mut text = server.store().file_text(path);
    text.push_str("\nThe key to the bench cupboard hangs by the door.\n");
    server.commit(
        "add the cupboard key to the bench rule",
        vec![(path.to_string(), Some(text.into_bytes()))],
    );

    let owed = prompt(&server, &key, PromptMode::Due);
    assert!(
        owed.text.contains(BENCH_POWER_BODY),
        "the rewritten rule must be due in full before the next event, got {:?}",
        owed.text
    );
    assert_eq!(
        owed.bytes,
        owed.text.len() as u64,
        "the byte count must be the length of the text beside it"
    );
}

/// Detects a whole-prompt mode computed against the context's delivered record
/// instead of against an empty one: it would answer the same as the due prompt
/// and never show what a set of scopes costs in full. Detects a byte count or a
/// token estimate that does not describe the text beside it as well.
#[test]
fn the_whole_prompt_carries_every_critical_memory_of_the_contexts_scopes_with_its_size() {
    let server = TestServer::start(example_store_files(), |_| {});
    let key = ContextKey::main(MACHINE, "session-1");
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));

    let all = prompt(&server, &key, PromptMode::All);

    assert!(
        all.text.contains(BENCH_POWER_BODY),
        "the whole prompt must carry the global critical memory in full although the context \
         already holds it, got {:?}",
        all.text
    );
    assert_eq!(
        all.bytes,
        all.text.len() as u64,
        "the byte count must be the length of the text beside it"
    );
    assert!(
        all.tokens >= 1,
        "a text that is not empty must be estimated at a token or more, got {all:?}"
    );
    assert_eq!(
        (all.mode, all.key.as_str()),
        (PromptMode::All, key.to_string().as_str()),
        "the answer must name the mode it was rendered in and the context it was rendered for"
    );
}

/// Detects a prompt that records what it rendered as delivered: the next hook
/// event would then leave out the rules a person had only looked at, and the
/// session would never be given them.
#[test]
fn rendering_either_prompt_leaves_what_the_context_holds_alone() {
    let server = TestServer::start(example_store_files(), |_| {});
    let key = ContextKey::main(MACHINE, "session-1");
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    let before = delivered_count(&server, &key);
    assert!(
        before > 0,
        "the session start must have recorded what it delivered"
    );

    prompt(&server, &key, PromptMode::Due);
    prompt(&server, &key, PromptMode::All);

    assert_eq!(
        delivered_count(&server, &key),
        before,
        "neither prompt may change what the context is recorded as holding"
    );
}

/// Detects a prompt that creates the context it is asked about: a mistyped key
/// would leave a context nothing ever delivers to in the list, and the missing
/// one would be answered as though it existed.
#[test]
fn a_prompt_for_a_key_no_context_has_been_seen_at_is_refused_and_creates_nothing() {
    let server = TestServer::start(example_store_files(), |_| {});
    let missing = ContextKey::main(MACHINE, "nobody");
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));

    let refused = server
        .run(operations::context_prompt(
            &server.state(),
            &missing,
            PromptMode::Due,
        ))
        .expect_err("a key no context has been seen at must be refused");

    assert!(
        matches!(refused, OperationError::UnknownContext(_)),
        "the refusal must say that no such context exists, got {refused}"
    );
    let keys: Vec<String> = server
        .run(operations::contexts(
            &server.state().contexts,
            server.state().clock.now(),
        ))
        .into_iter()
        .map(|row| row.key)
        .collect();
    assert!(
        !keys.contains(&missing.to_string()),
        "the key must not have been created by asking about it, got {keys:?}"
    );
}

/// Detects contexts listed in key order, or oldest first: the list is read to
/// see what is working now, so the session heard from last leads it and a
/// session nothing has happened in sinks. Source: the contexts page is ordered
/// by "Last seen", most recent first (issue #21).
///
/// Two lists are read, because each wrong order matches the right one on one of
/// them: oldest first differs on the list taken after `session-1` is prompted,
/// key order differs on the one taken after `session-2` is prompted in turn.
#[test]
fn the_contexts_are_listed_with_the_one_heard_from_last_at_the_top() {
    let server = TestServer::start(example_store_files(), |_| {});
    let step = chrono::Duration::minutes(3);
    let keys = |server: &TestServer| -> Vec<String> {
        server
            .run(operations::contexts(
                &server.state().contexts,
                server.state().clock.now(),
            ))
            .into_iter()
            .map(|row| row.key)
            .collect()
    };
    for session_id in ["session-1", "session-2"] {
        server.hook(
            MACHINE,
            SOME_TOKENS,
            &event_with("session_start", &[("session_id", json!(session_id))]),
        );
        server.advance(step);
    }

    server.hook(
        MACHINE,
        SOME_TOKENS,
        &event_with("user_prompt_submit", &[("session_id", json!("session-1"))]),
    );
    let after_first = keys(&server);
    server.advance(step);
    server.hook(
        MACHINE,
        SOME_TOKENS,
        &event_with("user_prompt_submit", &[("session_id", json!("session-2"))]),
    );
    let after_second = keys(&server);

    assert_eq!(
        after_first,
        vec!["alpha/session-1", "alpha/session-2"],
        "the session prompted last must lead the list"
    );
    assert_eq!(
        after_second,
        vec!["alpha/session-2", "alpha/session-1"],
        "the list must follow the last event and not the keys"
    );
}

/// Detects a machine list read from the live contexts alone, or from the
/// statistics log alone: a machine whose sessions are all over is only in the
/// log, a context restored from the state file is in the registry before it
/// sends its next event, and the list is what the frontend offers wherever a
/// machine is picked, so a name missing from it cannot be chosen at all.
#[test]
fn the_machines_are_the_ones_the_contexts_and_the_statistics_log_name_each_once() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));

    let listed = server
        .run(operations::machines(
            &server.state().contexts,
            server.state().clock.now(),
            // `alpha` is in both sources and `gamma` in the log alone, which is
            // a machine whose sessions are over.
            ["gamma".to_string(), MACHINE.to_string()],
        ))
        .machines;

    assert_eq!(
        listed,
        vec!["alpha".to_string(), "gamma".to_string()],
        "every machine of either source must be listed once, in order"
    );
}

// ---------------------------------------------------------------------------
// The store's own history
// ---------------------------------------------------------------------------

/// A write to one memory, reporting the commit title it carries.
fn write_line_to(server: &TestServer, id: &str, message: &str) -> String {
    let document = memory(server, id);
    server
        .run(operations::memory_put(
            &server.state(),
            &MemoryId::new(id),
            &write_of(&document, &format!("# {id}\n\n{NEW_LINE}\n"), message),
            WriteMode::Update,
            None,
        ))
        .unwrap_or_else(|error| panic!("the write to {id} must land, got {error}"));
    message.to_string()
}

/// The titles one page of the store's history lists, in the order it lists them.
fn page_titles(page: &StoreHistory) -> Vec<String> {
    page.commits
        .iter()
        .map(|commit| commit.title.clone())
        .collect()
}

/// Detects a store history that lists the commits in another order, or that
/// cannot be paged through: the list page shows the newest first and reads the
/// rest by asking for the commits older than the last one it was given, so a
/// page that names no next one, or one that starts again where it started,
/// leaves the rest of the history unreachable.
#[test]
fn the_store_history_lists_every_commit_newest_first_and_pages_past_the_one_it_names() {
    let server = TestServer::start(example_store_files(), |_| {});
    let older = write_line_to(&server, "reading-list", "note where the binder lives");
    let newer = write_line_to(&server, "bench-power", "note the wall switch");
    let page = |before: Option<String>, limit: usize| {
        server
            .run(operations::store_history(&server.state(), before, limit))
            .expect("the store's history is readable")
    };

    let whole = page(None, 50);

    let titles = page_titles(&whole);
    assert_eq!(
        titles.first().map(String::as_str),
        Some(newer.as_str()),
        "the newest commit must come first, got {titles:?}"
    );
    assert_eq!(
        titles.get(1).map(String::as_str),
        Some(older.as_str()),
        "the write before it must come second, got {titles:?}"
    );
    assert!(
        titles.contains(&"build the fixture store".to_string()),
        "the commit that seeded the store must be listed too, got {titles:?}"
    );
    assert_eq!(
        whole.next_before, None,
        "a page holding the whole history must offer no next page"
    );

    let first = page(None, 1);
    assert_eq!(
        page_titles(&first),
        vec![newer],
        "a limit of one must answer with one commit"
    );
    let before = first
        .next_before
        .clone()
        .expect("a full page must name the next page's start");
    assert_eq!(
        page_titles(&page(Some(before), 1)),
        vec![older],
        "the next page must start after the commit it was given"
    );
}

/// Detects a commit page that reports files the commit did not change, that
/// carries no diff, or that leaves the reader to work out which document a path
/// holds. Detects the commit that has no parent being reported as having
/// changed nothing as well: the store's first commit is the one every file was
/// added in, and it is reachable from the history list like any other.
#[test]
fn a_store_commit_lists_only_the_files_it_changed_with_their_diffs_and_the_documents_they_hold() {
    let server = TestServer::start(example_store_files(), |_| {});
    write_line_to(&server, "reading-list", "note where the binder lives");
    let commit = |oid: &str| {
        server
            .run(operations::store_commit(&server.state(), oid))
            .unwrap_or_else(|error| panic!("the commit {oid} must be readable, got {error}"))
    };

    let write = commit(&head(&server));

    let files = &write.files;
    assert_eq!(
        files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["memories/reading-list.md"],
        "the commit must list the one file it changed"
    );
    assert_eq!(
        (files[0].status.as_str(), files[0].memory_id.as_ref()),
        ("modified", Some(&MemoryId::new("reading-list"))),
        "a file the commit rewrote must be reported as modified and must name the memory it holds"
    );
    assert!(
        files[0]
            .diff
            .lines()
            .any(|line| line == format!("+{NEW_LINE}")),
        "the file's diff must show the added line as added, got {:?}",
        files[0].diff
    );

    // The commit that wrote every file of the example store, which is the
    // oldest one the history lists with files in it.
    let seed = server
        .run(operations::store_history(&server.state(), None, 50))
        .expect("the store's history is readable")
        .commits
        .into_iter()
        .find(|commit| commit.title == "build the fixture store")
        .expect("the history lists the commit that seeded the store")
        .oid;
    let seeding = commit(&seed);
    let listed: BTreeSet<String> = seeding.files.iter().map(|file| file.path.clone()).collect();
    assert_eq!(
        listed,
        example_store_files()
            .into_iter()
            .map(|(path, _)| path)
            .collect::<BTreeSet<String>>(),
        "the commit with no parent added every file and must list every one of them"
    );
    assert!(
        seeding.files.iter().all(|file| file.status == "added"),
        "every file of that commit must be reported as added"
    );
    assert!(
        seeding
            .files
            .iter()
            .any(|file| file.scope_id == Some(ScopeId::new("widgets"))),
        "a scope's file must name the scope it holds"
    );
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// The settings of the store as they stand.
fn settings(server: &TestServer) -> SettingsDoc {
    server
        .run(operations::settings::settings_get(&server.state(), None))
        .expect("the settings are readable")
}

/// A change to one setting, made by the wiki.
fn settings_write(
    value: Value,
    base_version: Option<String>,
    message: &str,
) -> SettingsWriteRequest {
    SettingsWriteRequest {
        value,
        base_version,
        author: "wiki".to_string(),
        message: message.to_string(),
    }
}

/// Detects a write of the first setting that does not create the file, and one
/// that does not reach the settings the server answers from: the whole point of
/// the file is that the next read sees it. Detects a write that ignores the
/// version it was made from as well, which would let two people changing the
/// settings at once overwrite each other with neither one told.
#[test]
fn writing_a_setting_creates_the_file_and_a_write_from_a_stale_version_is_refused() {
    let server = TestServer::start(example_store_without_settings(), |_| {});
    let state = server.state();
    let before = head(&server);
    assert_eq!(
        settings(&server).version,
        None,
        "a store with no settings file has no version to write against"
    );

    let written = server
        .run(operations::settings::settings_set(
            &state,
            "reminder_tokens",
            &settings_write(
                json!(150_000),
                None,
                "remind the agent of the rules every 150k tokens",
            ),
            None,
        ))
        .expect("the key and the value are ones this server has");

    assert_ne!(head(&server), before, "the write must make a commit");
    let after = settings(&server);
    assert_eq!(
        after.settings["reminder_tokens"],
        json!(150_000),
        "the setting must read back as it was written"
    );
    assert_eq!(
        after.version, written.version,
        "the write must report the version the next one sends back"
    );

    let stale = written.version.clone();
    server
        .run(operations::settings::settings_set(
            &state,
            "reminder_tokens",
            &settings_write(
                json!(90_000),
                stale.clone(),
                "remind the agent of the rules every 90k tokens",
            ),
            None,
        ))
        .expect("the second write is made from the version the store has");

    let refused = server
        .run(operations::settings::settings_set(
            &state,
            "reminder_tokens",
            &settings_write(
                json!(10_000),
                stale,
                "remind the agent of the rules every 10k tokens",
            ),
            None,
        ))
        .expect_err("a write from a version that is no longer current must be refused");

    let OperationError::Conflict {
        current: CurrentDocument::Settings(current),
        ..
    } = refused
    else {
        panic!("the refusal must be a conflict carrying the settings, got {refused}");
    };
    assert_eq!(
        current.settings["reminder_tokens"],
        json!(90_000),
        "the refusal must carry the settings as the store has them"
    );
    assert_eq!(
        settings(&server).settings["reminder_tokens"],
        json!(90_000),
        "the refused write must not have changed anything"
    );
}

/// Detects a settings change on a branch that changes how sessions behave
/// before it lands, and one that does not take effect once it has: the settings
/// are part of the store, so a branch has to hold them back exactly as it holds
/// back a memory, and landing has to put them in force at the next event.
#[test]
fn a_settings_change_on_a_branch_is_not_in_force_until_the_branch_lands() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let owner = "alpha/session-13";
    let branch = server
        .run(operations::branches::branch_create(&state, owner))
        .expect("a branch opens at the head of main")
        .branch;

    server
        .run(operations::settings::settings_set(
            &state,
            "deliver_knowledge_index",
            &settings_write(
                json!(false),
                None,
                "fetch knowledge on demand instead of indexing it",
            ),
            Some(&branch),
        ))
        .expect("the key and the value are ones this server has");

    assert_eq!(
        settings(&server).settings["deliver_knowledge_index"],
        json!(true),
        "main must still carry the setting it had"
    );
    let (_, before) = server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    assert!(
        has_index_line(
            context_of(&before),
            "reading-list",
            "The workshop references are all on paper in the binder, nothing online"
        ),
        "the behaviour must not change before the branch lands, got {before}"
    );

    server
        .run(operations::branches::branch_land(
            &state,
            &branch,
            &LandRequest {
                message: "fetch knowledge on demand instead of indexing it".to_string(),
                author: owner.to_string(),
            },
        ))
        .expect("nothing on main touched the settings file");

    assert_eq!(
        settings(&server).settings["deliver_knowledge_index"],
        json!(false),
        "the landed setting must be the one in force"
    );
    let (_, after) = server.hook(
        MACHINE,
        SOME_TOKENS,
        &event_with("session_start", &[("session_id", json!("session-2"))]),
    );
    assert!(
        !has_index_line(
            context_of(&after),
            "reading-list",
            "The workshop references are all on paper in the binder, nothing online"
        ),
        "the landed setting must be in force at the next event, got {after}"
    );
}

/// Detects a list that turns on the ids it knows and then refuses the one it
/// does not: the model would read the call as failed while the session had
/// moved into scopes it is not told about. Source: the ticket for the list form
/// of the session scope tools.
#[test]
fn a_list_naming_an_id_that_is_no_scope_turns_none_of_the_others_on() {
    let server = TestServer::start(example_store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(MACHINE, "session-1");

    let refused = server
        .run(operations::session_scope_on(
            &state,
            &key,
            &[
                ScopeId::new("widgets"),
                ScopeId::new("spanners"),
                ScopeId::new("workshop"),
            ],
        ))
        .expect_err("a list naming an id the store has no scope for must be refused");

    assert!(
        refused.to_string().contains("spanners"),
        "the refusal must name the id that is not a scope, got {refused}"
    );
    let after = server
        .run(operations::session_scopes(&state, &key))
        .expect("the store is readable");
    for id in ["widgets", "workshop"] {
        assert!(
            !after.active.contains(&ScopeId::new(id)),
            "{id} was named beside an id that is no scope and must not have been turned on, got \
             {:?}",
            after.active
        );
    }
}

// ---------------------------------------------------------------------------
// The statistics read over a span of time: the summary windows, the series and
// the per-session series, against the server's own clock.
// ---------------------------------------------------------------------------

/// The instant the test server's clock starts at, which is when the first event
/// of a sequence below is recorded.
fn started() -> chrono::DateTime<chrono::Utc> {
    FixedClock::at_epoch_day().now()
}

/// A prompt naming a widget, which turns the `widgets` scope on and delivers
/// what that makes due.
fn widget_prompt(session: &str) -> Value {
    json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": session,
        "cwd": "/home/dev/notes",
        "prompt": "check the widget numbering before we continue"
    })
}

/// A prompt that names nothing the example store triggers on.
fn quiet_prompt(session: &str) -> Value {
    json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": session,
        "cwd": "/home/dev/notes",
        "prompt": "carry on where we left off"
    })
}

/// Detects a summary that counts every event whatever its age, or that puts an
/// event in the wrong window: the five-minute figure is what says whether
/// anything is happening now, and it is useless if an hour-old event is in it.
///
/// Expectation source: the sequence below against the server's own clock. A
/// session start, then the clock moves ten minutes, then a widget prompt. The
/// five-minute window therefore holds the prompt alone and the hour window holds
/// both.
#[test]
fn a_summary_window_holds_the_events_inside_it_and_none_of_the_older_ones() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    server.advance(chrono::Duration::minutes(10));
    server.hook(MACHINE, SOME_TOKENS, &widget_prompt("session-1"));

    let now = server.state().clock.now();
    let windows = server
        .stats()
        .summary(now)
        .expect("the summary is readable");
    let named = |name: &str| {
        windows
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("the summary must report the {name} window: {windows:?}"))
            .clone()
    };

    assert_eq!(
        named("5m").events,
        1,
        "the session start is ten minutes old, so only the prompt is in the five minutes: {windows:?}"
    );
    assert_eq!(
        named("1h").events,
        2,
        "both events are inside the hour: {windows:?}"
    );
    assert!(
        named("5m").chars > 0 && named("1h").chars > named("5m").chars,
        "the hour must carry the session start's text as well as the prompt's: {windows:?}"
    );
    assert_eq!(
        named("7d").events,
        2,
        "nothing of the sequence is a week old: {windows:?}"
    );
}

/// Detects a series that ignores the session it was asked for, one that ignores
/// the scope, and one that reports a bucket other than the one it bucketed by:
/// the two filters are the whole point of the chart, and a line narrowed to a
/// scope that is not narrowed at all reads as that scope costing everything.
///
/// Expectation source: the sequence below. Two sessions, one minute apart, each
/// answered at its own minute; `widgets` is delivered only into the second.
#[test]
fn a_series_narrowed_to_a_session_or_a_scope_carries_only_that_sessions_or_scopes_text() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    server.advance(chrono::Duration::minutes(1));
    server.hook(MACHINE, SOME_TOKENS, &quiet_prompt("session-2"));
    server.hook(MACHINE, SOME_TOKENS, &widget_prompt("session-2"));
    let stats = server.stats();

    let whole = stats
        .series(&Filter::all(), Bucket::Minute)
        .expect("the series is readable");
    assert_eq!(
        whole.len(),
        2,
        "the two minutes that delivered something are the two points: {whole:?}"
    );
    let total: u64 = whole.iter().map(|point| point.chars).sum();

    let first_session = Filter {
        session: Some(ContextKey::main(MACHINE, "session-1")),
        ..Filter::all()
    };
    let narrowed = stats
        .series(&first_session, Bucket::Minute)
        .expect("the series is readable");
    assert_eq!(
        narrowed.len(),
        1,
        "the first session was answered in one minute only: {narrowed:?}"
    );
    assert!(
        narrowed[0].chars < total,
        "one session's text must be less than both sessions': {narrowed:?} against {total}"
    );

    let widgets = Filter {
        scope: Some("widgets".to_string()),
        ..Filter::all()
    };
    let scoped = stats
        .series(&widgets, Bucket::Minute)
        .expect("the series is readable");
    assert_eq!(
        scoped.len(),
        1,
        "the widgets scope delivered in one minute only: {scoped:?}"
    );
    assert!(
        scoped[0].chars > 0 && scoped[0].chars < total,
        "the scope's own text must be part of the whole and not all of it: {scoped:?} against \
         {total}"
    );
    assert_eq!(
        stats
            .series(
                &Filter {
                    scope: Some("nothing-was-printed-here".to_string()),
                    ..Filter::all()
                },
                Bucket::Minute
            )
            .expect("the series is readable"),
        Vec::new(),
        "a scope nothing was printed under has no points"
    );
}

/// Detects a series whose window is not honoured, which would draw the whole log
/// whatever range the page asked for.
#[test]
fn a_series_over_a_window_leaves_out_what_falls_outside_it() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    let first = server.state().clock.now();
    server.advance(chrono::Duration::hours(2));
    server.hook(MACHINE, SOME_TOKENS, &widget_prompt("session-1"));
    let second = server.state().clock.now();

    let stats = server.stats();
    let later = stats
        .series(
            &Filter::over(Window::between(first + chrono::Duration::hours(1), second)),
            Bucket::Hour,
        )
        .expect("the series is readable");

    assert_eq!(
        later.len(),
        1,
        "only the second event is inside the window: {later:?}"
    );
    assert_eq!(
        later[0].t,
        second.format("%Y-%m-%dT%H:00:00Z").to_string(),
        "the point must be named by the start of the hour it falls in: {later:?}"
    );
}

/// Detects a per-session series that loses the context size Claude Code
/// reported, or that reports the answer's own length as that size: the two are
/// different measurements and the chart shows them beside each other.
///
/// Expectation source: the sequence below. The session start reports 10 000
/// tokens and carries text; the quiet prompt afterwards is owed nothing and
/// carries none.
#[test]
fn a_sessions_series_reports_the_size_claude_code_measured_beside_the_answers_own_length() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook(MACHINE, SOME_TOKENS, &hook_fixture("session_start"));
    server.hook(MACHINE, SOME_TOKENS, &quiet_prompt("session-1"));

    let key = ContextKey::main(MACHINE, "session-1");
    let events = server
        .stats()
        .session_series(&key, Window::default())
        .expect("the series is readable");

    assert_eq!(
        events
            .iter()
            .map(|row| row.event.as_str())
            .collect::<Vec<_>>(),
        vec!["SessionStart", "UserPromptSubmit"],
        "every event of the context must be there, oldest first: {events:?}"
    );
    assert!(
        events.iter().all(|row| row.context_tokens == Some(10_000)),
        "the size each event reported must be carried through as it arrived: {events:?}"
    );
    assert!(
        events[0].answer_chars > 0 && events[1].answer_chars == 0,
        "the start carried text and the prompt was owed nothing: {events:?}"
    );
    assert!(
        server
            .stats()
            .session_series(&ContextKey::main(MACHINE, "nobody"), Window::default())
            .expect("the series is readable")
            .is_empty(),
        "a context the log has no event of has no series"
    );
    assert_eq!(
        events[0].t,
        started().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "the events must be timestamped by the server's clock"
    );
}
