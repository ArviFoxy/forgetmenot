//! The contract of `POST /hook`: the status codes, the shape of the answer
//! Claude Code reads, and that the store's settings file is what the endpoint
//! behaves by.
//!
//! What the answer says is decided by the state machine, the event mapping and
//! the renderer, each of which is tested at its own interface beside the code
//! in `src/context`, `src/hook` and `src/render`; the sequences a session goes
//! through are tested in `tests/scenarios_*.rs`. What is left here is what only
//! this endpoint can be asked: the JSON keys Claude Code validates, the codes a
//! client sees, and one event answered by two servers reading two settings
//! files.

mod common;

use serde_json::{Value, json};

use common::{
    TestServer, additional_context, example_store_files, example_store_with_settings, hook_fixture,
    permission_decision, post_hook,
};

/// The reason a stopped tool call is given. Source: the plan's hook contract,
/// where this text is fixed so that the model can tell an automatic keyword
/// match apart from a human refusing the call.
const DENY_REASON: &str = "This tool call was not executed. Its input matched a memory trigger and a critical memory is new or changed for this context. This is an automatic keyword match, not a review, approval or denial of the call. Read the memory in the additional context. Issue the call again if it is still what you intend.";

/// A line in the body of the example store's `widget-naming` and nowhere else,
/// so that "the memory is in the answer" can be told from anything else the
/// store delivers.
const WIDGET_NAMING_BODY: &str = "Downstream drawings cite part numbers by value";

/// A line in the body of the example store's global critical memory.
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";

/// The context size reported with an event, which no test here depends on.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// The payload Claude Code reads, or `Value::Null` when the answer carries
/// none. Read by its wire name, because the camelCase spelling is the whole of
/// what Claude Code looks for.
fn payload(answer: &Value) -> &Value {
    &answer["hookSpecificOutput"]
}

/// A `PreToolUse` whose tool and input match no trigger of the example store,
/// so that the answer depends on the delivery record alone.
fn neutral_pre_tool_use() -> Value {
    let mut event = hook_fixture("pre_tool_use_read");
    let object = event.as_object_mut().expect("a hook payload is an object");
    object.insert("tool_name".to_string(), json!("Read"));
    object.insert(
        "tool_input".to_string(),
        json!({ "file_path": "/home/dev/notes/README.md" }),
    );
    event
}

/// Whether any line of `text`, trimmed, is exactly `wanted`. Used for the list
/// of scope ids, which is one id per line; it does not depend on the wording of
/// the section label above it.
fn has_line(text: &str, wanted: &str) -> bool {
    text.lines().any(|line| line.trim() == wanted)
}

/// Detects an answer Claude Code drops or refuses at a session start: it reads
/// `hookSpecificOutput.additionalContext` under the event's own name and
/// nothing else, so a misspelled or missing key injects nothing silently. It
/// also detects a start that omits the session key the MCP tools take, which
/// would leave the session with no way to ask for a memory, and one that names
/// scopes the session is not in: what is not active is not delivered and not
/// listed, and the tools are where a session asks what else exists.
///
/// The global critical memory stands for what the answer carries: which
/// memories are owed and how each is written are decided by the state machine
/// and the renderer and are tested there.
#[test]
fn a_session_start_is_answered_with_the_context_shape_the_session_key_and_nothing_inactive() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    assert_eq!(status, 200, "a session start must be answered");
    assert_eq!(
        payload(&answer)["hookEventName"],
        json!("SessionStart"),
        "the answer must name the event it answers, got {answer}"
    );
    let text = payload(&answer)["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("the text must be injected under additionalContext: {answer}"));
    assert!(
        text.contains(BENCH_POWER_BODY),
        "the memories the session's scopes owe it must be in the answer, got {text:?}"
    );
    assert!(
        text.contains("session_key: alpha/session-1"),
        "the session key the MCP tools take must be printed, got {text:?}"
    );
    for scope in ["widgets", "rocketry", "workshop"] {
        assert!(
            !has_line(text, scope),
            "the scope {scope} is not active here and must not be named, got {text:?}"
        );
    }
}

/// Detects a stopped call answered in a shape Claude Code does not act on: the
/// decision and its reason live beside the text under `hookSpecificOutput`, in
/// exactly these camelCase keys, and a key Claude Code does not know is dropped
/// without a word, so the call would run with the model never told why it was
/// meant to stop. The reason is the fixed text, because the model has to tell
/// an automatic keyword match apart from a human refusing the call.
#[test]
fn a_held_tool_call_is_answered_with_the_deny_shape_its_fixed_reason_and_the_memory() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_bash"));

    assert_eq!(status, 200, "a tool call must be answered");
    assert_eq!(
        payload(&answer)["hookEventName"],
        json!("PreToolUse"),
        "the answer must name the event it answers, got {answer}"
    );
    assert_eq!(
        payload(&answer)["permissionDecision"],
        json!("deny"),
        "a new critical memory must stop the call, got {answer}"
    );
    assert_eq!(
        payload(&answer)["permissionDecisionReason"],
        json!(DENY_REASON),
        "the reason must be the fixed text, got {answer}"
    );
    assert!(
        payload(&answer)["additionalContext"]
            .as_str()
            .is_some_and(|text| text.contains(WIDGET_NAMING_BODY)),
        "the stopped call must carry the memory to read, got {answer}"
    );
}

/// Detects an event with nothing owed answered with a payload: Claude Code
/// validates the object it reads and refuses `hookSpecificOutput` for some
/// event names, so an answer that names an event for the sake of saying nothing
/// would report the hook as failing. Every top-level key is optional, which
/// makes `{}` the answer that asks for nothing.
#[test]
fn an_event_with_nothing_owed_is_answered_with_the_empty_object() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &neutral_pre_tool_use());

    assert_eq!(status, 200, "the event must be answered");
    assert_eq!(
        answer,
        json!({}),
        "nothing is owed, so the answer must ask for nothing, got {answer}"
    );
}

/// Detects an answer to a compaction that carries a `hookSpecificOutput`:
/// Claude Code validates the object it reads and takes no payload for
/// `PostCompact`, so the hook would be reported as failing after every
/// compaction, whether or not anything was owed.
///
/// Expectation source: Claude Code 2.1.x refused
/// `{"hookSpecificOutput": {"hookEventName": "PostCompact"}}` with
/// `hookSpecificOutput.hookEventName: expected one of "PreToolUse" | …`, and
/// leaves every top-level key optional, so `{}` is valid for every event
/// (observed in a session on 2026-09-13).
#[test]
fn a_compaction_is_answered_with_the_empty_object_and_nothing_else() {
    let server = TestServer::start(example_store_files(), |_| {});
    // A session start first, so that the compaction arrives at a context the
    // server has already delivered to rather than one it is seeing for the
    // first time.
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("post_compact"));

    assert_eq!(status, 200, "a compaction must be acknowledged");
    assert_eq!(
        answer,
        json!({}),
        "a compaction must be answered with the empty object, got {answer}"
    );
}

/// Detects a server that rejects an event name a newer Claude Code sends: the
/// hook would fail on every one of those events instead of being ignored.
#[test]
fn an_event_name_the_server_does_not_know_is_answered_with_an_empty_object() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("unknown_event"));

    assert_eq!(status, 200, "an unknown event must be acknowledged");
    assert_eq!(answer, json!({}), "the answer must ask for nothing");
}

/// Detects a body that is not a request being treated as one, which would make
/// a client bug look like an empty event.
#[test]
fn a_body_that_is_not_a_hook_request_is_rejected() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (malformed, _) = server.post_raw("/hook", "{not json");
    let (incomplete, _) = server.post_raw("/hook", "{\"machine\": \"alpha\"}");

    assert_eq!(malformed, 400, "a body that is not JSON must be rejected");
    assert_eq!(
        incomplete, 400,
        "a body without the hook event must be rejected"
    );
}

/// Detects an endpoint that answers by its own defaults rather than by the
/// store it is serving: the settings file is part of the store and a session
/// must behave the way the store it reads asks for.
///
/// One setting stands for the file: the same call and the same memory, held on
/// the store the other tests here run on and let through on a store whose
/// `config.yml` turns the interrupt off, where the memory still arrives. What
/// each setting means is a rule of the settings and of the state machine, and
/// is tested there.
#[test]
fn a_setting_the_stores_config_file_carries_is_what_the_endpoint_answers_by() {
    let server = TestServer::start(
        example_store_with_settings("interrupt_on_critical: false\n"),
        |_| {},
    );

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_bash"));

    assert_eq!(status, 200, "a tool call must be answered");
    assert_eq!(
        permission_decision(&answer),
        None,
        "this store turns the interrupt off, so the call must not be stopped, got {answer}"
    );
    assert!(
        additional_context(&answer).is_some_and(|text| text.contains(WIDGET_NAMING_BODY)),
        "the critical memory must still be delivered in full, got {answer}"
    );
}

/// Detects a critical section that does not cover computing and recording a
/// delivery together: parallel tool calls would each be stopped, or several
/// would carry the same memory. Nothing below the endpoint can be asked this:
/// the guarantee is about two requests being served at once.
#[test]
fn parallel_tool_calls_on_one_context_are_stopped_exactly_once() {
    let server = TestServer::start(example_store_files(), |_| {});
    let url = server.url();
    let call = hook_fixture("pre_tool_use_bash");

    let answers: Vec<Value> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..32)
            .map(|_| {
                let url = url.clone();
                let call = call.clone();
                scope.spawn(move || {
                    let (status, answer) = post_hook(&url, "alpha", SOME_TOKENS, &call);
                    assert_eq!(status, 200, "every parallel call must be answered");
                    answer
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("the request thread finishes"))
            .collect()
    });

    let stopped = answers
        .iter()
        .filter(|answer| permission_decision(answer) == Some("deny"))
        .count();
    assert_eq!(
        stopped, 1,
        "exactly one of 32 parallel calls may be stopped, {stopped} were"
    );
    let carried = answers
        .iter()
        .filter(|answer| {
            permission_decision(answer).is_none()
                && additional_context(answer).is_some_and(|text| text.contains(WIDGET_NAMING_BODY))
        })
        .count();
    assert_eq!(
        carried, 0,
        "a call that was not stopped must not carry the memory, {carried} did"
    );
}
