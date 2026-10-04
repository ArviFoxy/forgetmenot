//! Tests of the scope activation log at the operations interface: what each
//! cause of a scope coming on records, what is not recorded, the log turned
//! off, and the retention window.
//!
//! Every step is a hook event sent to a running server or an operation called
//! on its state, and the log is read back through the operations the MCP tools
//! call. The JSON the tools answer with is `mcp_test.rs`, and following a
//! subagent's scope back to the message that turned it on over a whole session
//! is `scenarios_activation_log.rs`.
//!
//! The expectations come from the rules of the log: one entry per scope that
//! goes from off to on in a context, with its cause; nothing for a scope that
//! was on already.

mod common;

use forgetmenot_server::app;
use forgetmenot_server::context::ContextKey;
use forgetmenot_server::operations::activations::{scope_activation_get, scope_activations};
use forgetmenot_server::operations::{self, OperationError};
use forgetmenot_server::stats::Table;
use forgetmenot_server::stats::activations::{
    ActivationEntry, ActivationFilter, CauseDetails, Order,
};
use forgetmenot_server::store::ScopeId;
use serde_json::{Value, json};

use common::TestServer;

/// The machine every session here runs on.
const ALPHA: &str = "alpha";

/// The session most tests run.
const SESSION: &str = "session-1";

/// The scope a prompt naming the bench supply turns on.
const BENCH: &str = "bench";

/// The scope `bench` implies, which has no trigger of its own.
const WIRING: &str = "wiring";

/// The pattern of `bench`'s one trigger.
const BENCH_PATTERN: &str = r"\bbench supply\b";

/// What a prompt says before it names the bench supply: several characters of
/// it are more than one byte long, so a position counted in bytes is not the
/// position counted in characters.
const BEFORE_THE_MATCH: &str = "Größe der Kabel — prüfe die ";

/// The words of a prompt the trigger matches.
const MATCHED: &str = "bench supply";

/// A prompt that names the bench supply after [`BEFORE_THE_MATCH`].
fn bench_prompt() -> String {
    format!("{BEFORE_THE_MATCH}{MATCHED} vor dem Umbau")
}

/// A store with `bench`, whose trigger is [`BENCH_PATTERN`] on what the user
/// writes and which implies `wiring`, and `wiring`, which nothing triggers.
fn store_files() -> Vec<(String, Option<Vec<u8>>)> {
    vec![
        (
            "scopes/bench.yaml".to_string(),
            Some(
                format!(
                    "id: bench\nimplies:\n- wiring\ntriggers:\n- on: user_message\n  \
                     pattern: '{BENCH_PATTERN}'\n"
                )
                .into_bytes(),
            ),
        ),
        (
            "scopes/wiring.yaml".to_string(),
            Some(b"id: wiring\n".to_vec()),
        ),
    ]
}

/// One event of `session`, with the fields every event carries.
fn event(session: &str, fields: Value) -> Value {
    let mut payload = json!({
        "session_id": session,
        "transcript_path": "/nonexistent/transcript.jsonl",
        "cwd": "/home/dev/notes",
        "permission_mode": "default",
    });
    let object = payload.as_object_mut().expect("a payload is an object");
    for (key, value) in fields.as_object().expect("the fields are an object") {
        object.insert(key.clone(), value.clone());
    }
    payload
}

/// Send the user's prompt `text` in `session`.
fn prompt(server: &TestServer, session: &str, text: &str) {
    let (status, answer) = server.hook(
        ALPHA,
        Some(10_000),
        &event(
            session,
            json!({ "hook_event_name": "UserPromptSubmit", "prompt": text }),
        ),
    );
    assert_eq!(status, 200, "the prompt must be answered, got {answer}");
}

/// Start the subagent `agent_id` in `session`, spawned by the session itself.
fn start_subagent(server: &TestServer, session: &str, agent_id: &str) {
    let (status, answer) = server.hook(
        ALPHA,
        Some(10_000),
        &event(
            session,
            json!({
                "hook_event_name": "SubagentStart",
                "agent_id": agent_id,
                "agent_type": "general-purpose",
            }),
        ),
    );
    assert_eq!(
        status, 200,
        "the subagent start must be answered, got {answer}"
    );
}

/// Every entry of the log, oldest first.
fn all_entries(server: &TestServer) -> Vec<ActivationEntry> {
    let state = server.state();
    server
        .run(scope_activations(
            &state,
            ActivationFilter {
                order: Order::Oldest,
                limit: 200,
                ..ActivationFilter::default()
            },
        ))
        .expect("the log is readable")
        .entries
}

/// The entries of one context and one scope, oldest first.
fn entries_of(entries: &[ActivationEntry], context: &str, scope: &str) -> Vec<ActivationEntry> {
    entries
        .iter()
        .filter(|entry| entry.context == context && entry.scope == scope)
        .cloned()
        .collect()
}

/// The one entry of a context and a scope, failing the test when there is not
/// exactly one.
fn only_entry(entries: &[ActivationEntry], context: &str, scope: &str) -> ActivationEntry {
    let found = entries_of(entries, context, scope);
    assert_eq!(
        found.len(),
        1,
        "{scope} came on once in {context}, so it has one entry, got {entries:?}"
    );
    found.into_iter().next().expect("one entry")
}

/// The key of a session's main context, as the log prints it.
fn session_key(session: &str) -> String {
    ContextKey::main(ALPHA, session).to_string()
}

/// Detects a trigger entry that does not lead back to what the trigger saw: a
/// message stored other than as it was matched, the wrong field or pattern, or
/// a match position counted in bytes, which points into the middle of a
/// character as soon as the text holds one that is longer than a byte.
///
/// Expectation source: the prompt, which names the bench supply after
/// [`BEFORE_THE_MATCH`]; the regex's match is [`MATCHED`], which starts after
/// as many characters as that prefix has.
#[test]
fn a_trigger_entry_records_the_message_the_field_the_pattern_and_the_character_span() {
    let server = TestServer::start(store_files(), |_| {});
    let text = bench_prompt();

    prompt(&server, SESSION, &text);

    let entry = only_entry(&all_entries(&server), &session_key(SESSION), BENCH);
    let state = server.state();
    let detail = server
        .run(scope_activation_get(&state, entry.id))
        .expect("the entry is readable");
    assert_eq!(
        detail.message.as_deref(),
        Some(text.as_str()),
        "the stored message is the text the trigger was matched against"
    );
    let CauseDetails::Trigger {
        field, evidence, ..
    } = &detail.entry.cause
    else {
        panic!(
            "a prompt's match is a trigger entry, got {:?}",
            detail.entry
        );
    };
    assert_eq!(field, "user_message", "the prompt is what the user wrote");
    assert_eq!(
        evidence["pattern"],
        json!(BENCH_PATTERN),
        "the evidence names the pattern that matched"
    );
    let start = evidence["start"]
        .as_u64()
        .expect("a regex match has a start") as usize;
    let end = evidence["end"].as_u64().expect("a regex match has an end") as usize;
    assert_eq!(
        start,
        BEFORE_THE_MATCH.chars().count(),
        "the start counts the characters before the match, not the bytes"
    );
    let spanned: String = text.chars().skip(start).take(end - start).collect();
    assert_eq!(
        spanned, MATCHED,
        "the characters from start to end of the stored message are the matched text"
    );
}

/// Detects an entry written for a trigger that fires on a scope already on: the
/// log answers why a scope came on, and a second entry would claim it came on
/// twice. Those fires are what the trigger statistics count, not the log.
///
/// Expectation source: the rule that the log records off-to-on transitions.
#[test]
fn a_trigger_firing_on_a_scope_already_on_records_nothing() {
    let server = TestServer::start(store_files(), |_| {});

    prompt(&server, SESSION, &bench_prompt());
    prompt(&server, SESSION, "the bench supply hums again");

    let entries = all_entries(&server);
    assert_eq!(
        entries_of(&entries, &session_key(SESSION), BENCH).len(),
        1,
        "the second prompt found bench on, so only the first has an entry, got {entries:?}"
    );
}

/// Detects an implied scope logged without its cause, or naming an entry other
/// than the one of the scope that implied it: following an implied scope back
/// to the message goes through that entry.
///
/// Expectation source: the store, where `bench` implies `wiring` and nothing
/// else turns `wiring` on.
#[test]
fn an_implied_entry_names_the_implying_scope_and_its_entry() {
    let server = TestServer::start(store_files(), |_| {});

    prompt(&server, SESSION, &bench_prompt());

    let entries = all_entries(&server);
    let bench = only_entry(&entries, &session_key(SESSION), BENCH);
    let wiring = only_entry(&entries, &session_key(SESSION), WIRING);
    assert_eq!(
        wiring.cause,
        CauseDetails::Implied {
            implied_by: BENCH.to_string(),
            implied_by_entry: Some(bench.id),
        },
        "wiring came on because bench did, at bench's entry"
    );
}

/// Detects a subagent's inherited scopes logged without the entry of the
/// parent they came from, and the scopes every context starts with logged as
/// inherited: the first breaks following a subagent's scope back to the
/// parent's message, the second fills the log with entries that say nothing.
///
/// Expectation source: the parent turned `bench` on by a prompt and `wiring` by
/// implication before the subagent started, and created it from its own scopes.
#[test]
fn a_subagent_records_each_inherited_scope_with_the_parents_entry() {
    let server = TestServer::start(store_files(), |_| {});
    prompt(&server, SESSION, &bench_prompt());

    start_subagent(&server, SESSION, "agent-1");

    let entries = all_entries(&server);
    let parent = session_key(SESSION);
    let child = ContextKey::subagent(ALPHA, SESSION, "agent-1").to_string();
    let inherited: Vec<(String, CauseDetails)> = entries
        .iter()
        .filter(|entry| entry.context == child)
        .map(|entry| (entry.scope.clone(), entry.cause.clone()))
        .collect();
    assert_eq!(
        inherited,
        vec![
            (
                BENCH.to_string(),
                CauseDetails::Inherited {
                    parent_context: parent.clone(),
                    parent_entry: Some(only_entry(&entries, &parent, BENCH).id),
                },
            ),
            (
                WIRING.to_string(),
                CauseDetails::Inherited {
                    parent_context: parent.clone(),
                    parent_entry: Some(only_entry(&entries, &parent, WIRING).id),
                },
            ),
        ],
        "the child inherited bench and wiring from its parent, each at the parent's entry, \
         and nothing for the scopes it starts with anyway"
    );
}

/// Detects `session_scope_on` turning a scope on without an entry, or the
/// scopes it implies logged as named by the call.
///
/// Expectation source: the call names `bench`, which implies `wiring`.
#[test]
fn session_scope_on_records_the_named_scope_and_what_it_implies() {
    let server = TestServer::start(store_files(), |_| {});
    let state = server.state();
    let key = ContextKey::main(ALPHA, SESSION);

    server
        .run(operations::session_scope_on(
            &state,
            &key,
            &[ScopeId::new(BENCH)],
        ))
        .expect("bench is a scope of the store");

    let entries = all_entries(&server);
    let bench = only_entry(&entries, &key.to_string(), BENCH);
    assert_eq!(
        bench.cause,
        CauseDetails::SessionScopeOn,
        "the tool named bench"
    );
    assert_eq!(
        only_entry(&entries, &key.to_string(), WIRING).cause,
        CauseDetails::Implied {
            implied_by: BENCH.to_string(),
            implied_by_entry: Some(bench.id),
        },
        "wiring came on because the scope the tool named implies it"
    );
}

/// Detects `session_inherit` taking scopes over without entries, or entries
/// that do not say which session they came from.
///
/// Expectation source: the source session has `bench` and `wiring` on, and
/// inheriting takes those and the source's own session scope.
#[test]
fn session_inherit_records_every_scope_taken_with_the_source_session() {
    let server = TestServer::start(store_files(), |_| {});
    let state = server.state();
    prompt(&server, "session-source", &bench_prompt());
    let source = ContextKey::main(ALPHA, "session-source");
    let caller = ContextKey::main(ALPHA, SESSION);

    server
        .run(operations::session_inherit(&state, &caller, &source))
        .expect("the source session has been seen");

    let taken: Vec<(String, CauseDetails)> = all_entries(&server)
        .into_iter()
        .filter(|entry| entry.context == caller.to_string())
        .map(|entry| (entry.scope, entry.cause))
        .collect();
    let from_source = CauseDetails::SessionInherit {
        source_context: source.to_string(),
    };
    assert_eq!(
        taken,
        vec![
            (BENCH.to_string(), from_source.clone()),
            (source.session_scope().to_string(), from_source.clone()),
            (WIRING.to_string(), from_source),
        ],
        "every scope the caller did not have is taken from the source session"
    );
}

/// Detects a log that keeps recording when the server was told not to, which
/// would store the text of every message that turns a scope on against the
/// operator's choice, and tools that answer an empty list instead of saying the
/// log is off, which reads as nothing having come on.
///
/// Expectation source: the `--no-activation-log` flag, which turns recording
/// off.
#[test]
fn with_the_log_off_nothing_is_recorded_and_the_tools_say_it_is_off() {
    let server = TestServer::start(store_files(), |config| config.activation_log = false);
    let state = server.state();
    prompt(&server, SESSION, &bench_prompt());
    start_subagent(&server, SESSION, "agent-1");
    server
        .run(operations::session_scope_on(
            &state,
            &ContextKey::main(ALPHA, "session-2"),
            &[ScopeId::new(BENCH)],
        ))
        .expect("bench is a scope of the store");

    let stats = server.stats();
    assert_eq!(
        stats.count_rows(Table::ScopeActivations),
        0,
        "no entry is written with the log off"
    );
    assert_eq!(
        stats.count_rows(Table::ActivationMessages),
        0,
        "no message is stored with the log off"
    );
    let listed = server.run(scope_activations(&state, ActivationFilter::default()));
    assert!(
        matches!(listed, Err(OperationError::ActivationLogOff)),
        "listing says the log is off, got {listed:?}"
    );
    let read = server.run(scope_activation_get(&state, 1));
    assert!(
        matches!(read, Err(OperationError::ActivationLogOff)),
        "reading one entry says the log is off, got {read:?}"
    );
}

/// Detects a retention window that deletes nothing, deletes entries inside it,
/// or deletes a message an entry inside the window still names, and one that
/// leaves the message of a deleted entry behind: the stored messages are the
/// bulk of the log and the reason it has a window.
///
/// Expectation source: a seven-day window. The first prompt is eight days old
/// when the log is pruned and the second is new.
#[test]
fn the_retention_window_deletes_old_entries_and_their_messages_and_keeps_the_rest() {
    let server = TestServer::start(store_files(), |config| {
        config.activation_log_retention_days = Some(7);
    });
    let state = server.state();
    prompt(&server, "session-old", &bench_prompt());
    let old = only_entry(&all_entries(&server), &session_key("session-old"), BENCH);
    server.advance(chrono::Duration::days(8));
    let recent_text = "the bench supply for the second rig";
    prompt(&server, "session-new", recent_text);

    server.run(app::prune_activation_log(&state));

    let entries = all_entries(&server);
    assert!(
        entries_of(&entries, &session_key("session-old"), BENCH).is_empty()
            && entries_of(&entries, &session_key("session-old"), WIRING).is_empty(),
        "the entries eight days old are deleted, got {entries:?}"
    );
    let gone = server.run(scope_activation_get(&state, old.id));
    assert!(
        matches!(gone, Err(OperationError::NotFound(_))),
        "a deleted entry cannot be read, got {gone:?}"
    );
    let recent = only_entry(&entries, &session_key("session-new"), BENCH);
    let kept = server
        .run(scope_activation_get(&state, recent.id))
        .expect("the recent entry is kept");
    assert_eq!(
        kept.message.as_deref(),
        Some(recent_text),
        "the message a kept entry names is kept"
    );
    assert_eq!(
        server.stats().count_rows(Table::ActivationMessages),
        1,
        "the old message is deleted with the entries that named it"
    );
}
