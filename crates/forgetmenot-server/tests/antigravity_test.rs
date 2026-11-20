//! Antigravity sessions against a real server: what only shows over several
//! events, and the client run the way Antigravity runs it.
//!
//! How one event is read and how one answer is written are tested beside the
//! code, in `src/hook/antigravity.rs` and in the types crate. What is left here
//! is what the events do to each other: Antigravity takes text at a session
//! start and before a model call and nowhere else, so a memory that a tool call
//! or the end of a turn makes due has to wait for the next model call without
//! being counted as delivered in the meantime.
//!
//! The payloads carry the fields Antigravity 1.5.12 sent to a hook on
//! 2026-11-20, with invented identifiers and paths.

mod common;

use serde_json::{Value, json};

use common::{TestServer, example_store_files, post_antigravity_hook, run_antigravity_hook_client};

const CONVERSATION: &str = "0b6f7c7e-1d2a-4f39-9c55-2a8e4d1f6b10";

/// A line in the body of the example store's global pinned memory.
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";

/// A line in the body of the example store's `widget-naming`, a pinned memory
/// of the `widgets` scope, which a tool input naming widgets turns on.
const WIDGET_NAMING_BODY: &str = "Downstream drawings cite part numbers by value";

/// The description of the example store's `rocket-stages`, a memory of the
/// `rocketry` scope that is not pinned, so it is delivered as this line.
const ROCKET_STAGES_DESCRIPTION: &str = "Stage one lights on the pad";

/// The reason a stopped tool call is given, fixed so that the model can tell an
/// automatic keyword match apart from a human refusing the call.
const DENY_REASON: &str = "This tool call was not executed. Its input matched a memory trigger and a pinned memory is new or changed for this context. This is an automatic keyword match, not a review, approval or denial of the call. The memory arrives in a system message before your next step. Read it, then issue the call again if it is still what you intend.";

/// The context size reported with an event, which no test here depends on.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// An event of the one conversation these tests run, with `extra` beside the
/// fields every event carries.
fn event(name: &str, prompt: &str, extra: Value) -> Value {
    let mut payload = json!({
        "hookEventName": name,
        "conversationId": CONVERSATION,
        "cwd": "/home/dev/.frontend/attachments",
        "workspacePaths": ["/home/dev/.frontend/attachments", "/home/dev/notes"],
        "transcriptPath": "/home/dev/.profile/brain/transcript_full.jsonl",
        "artifactDirectoryPath": "/home/dev/.profile/brain",
        "modelName": "gemini-example",
        "lastUserInput": prompt
    });
    let fields = payload.as_object_mut().expect("an object");
    for (key, value) in extra.as_object().expect("an object") {
        fields.insert(key.clone(), value.clone());
    }
    payload
}

fn session_start(prompt: &str) -> Value {
    event("SessionStart", prompt, json!({}))
}

/// The model call numbered `number` of the turn `prompt` began.
fn model_call(prompt: &str, number: u64) -> Value {
    event(
        "PreInvocation",
        prompt,
        json!({ "invocationNum": number, "initialNumSteps": 4 }),
    )
}

fn run_command(prompt: &str, command: &str) -> Value {
    event(
        "PreToolUse",
        prompt,
        json!({
            "stepIdx": 4,
            "toolCall": {
                "id": "call_1",
                "name": "run_command",
                "args": { "CommandLine": command, "WaitMsBeforeAsync": 5000 }
            }
        }),
    )
}

/// The text an answer puts in front of the model, or `None` when it puts none.
/// Read by its wire names, because the nesting is the whole of what Antigravity
/// looks for.
fn system_message(answer: &Value) -> Option<&str> {
    answer["injectSteps"][0]["systemMessage"]["systemMessage"].as_str()
}

/// Send one event of the conversation and return the answer, which must be a
/// 200: Antigravity acts on the body alone.
fn send(server: &TestServer, last_assistant_message: Option<&str>, event: &Value) -> Value {
    let (status, answer) = post_antigravity_hook(
        &server.url(),
        "alpha",
        SOME_TOKENS,
        last_assistant_message,
        event,
    );
    assert_eq!(status, 200, "the event must be answered, got {answer}");
    answer
}

/// Detects a session start whose memories Antigravity never shows the model:
/// it reads `injectSteps` with a system message nested under the same key
/// twice, and ignores every key of Claude Code's answer. Also detects a start
/// without the session key, which would leave the session unable to write a
/// memory, and a first prompt that turns no scope on.
#[test]
fn a_session_and_its_first_prompt_are_answered_in_system_messages() {
    let server = TestServer::start(example_store_files(), |_| {});
    let prompt = "renumber the widget drawings";

    let start = send(&server, None, &session_start(prompt));
    let text = system_message(&start)
        .unwrap_or_else(|| panic!("the text must be a system message step: {start}"));
    assert!(text.contains(BENCH_POWER_BODY), "got {text:?}");
    assert!(
        text.contains(&format!("session_key: alpha/{CONVERSATION}")),
        "the session key the MCP tools take must be printed, got {text:?}"
    );
    assert!(
        start.get("hookSpecificOutput").is_none(),
        "Claude Code's key says nothing to Antigravity, got {start}"
    );
    assert!(
        !text.contains(WIDGET_NAMING_BODY),
        "the prompt is read at the model call, not at the session start"
    );

    let first_call = send(&server, None, &model_call(prompt, 0));
    let text = system_message(&first_call)
        .unwrap_or_else(|| panic!("the prompt's memory must be delivered: {first_call}"));
    assert!(text.contains(WIDGET_NAMING_BODY), "got {text:?}");

    let second_call = send(&server, None, &model_call(prompt, 1));
    assert_eq!(second_call, json!({}), "nothing is owed twice");
}

/// Detects a memory counted as delivered at an event that cannot deliver it:
/// Antigravity reads a decision and a reason from the answer to a tool call and
/// no text, so a memory "delivered" there is never shown, and is never sent
/// again either. Also detects a held call that is not refused, a refusal that
/// repeats once the memory has been shown, and a tool call left without the
/// decision Antigravity requires.
#[test]
fn a_call_held_for_a_pinned_memory_is_refused_and_the_memory_comes_with_the_next_model_call() {
    let server = TestServer::start(example_store_files(), |_| {});
    let prompt = "run the tests";
    send(&server, None, &session_start(prompt));
    send(&server, None, &model_call(prompt, 0));
    let call = run_command(prompt, "cargo test -p widgets");

    let held = send(&server, None, &call);
    assert_eq!(
        held,
        json!({ "decision": "deny", "reason": DENY_REASON }),
        "a new pinned memory must stop the call, with the fixed reason and no text"
    );

    let next_call = send(&server, None, &model_call(prompt, 1));
    let text = system_message(&next_call)
        .unwrap_or_else(|| panic!("the memory must arrive before the model goes on: {next_call}"));
    assert!(text.contains(WIDGET_NAMING_BODY), "got {text:?}");

    let reissued = send(&server, None, &call);
    assert_eq!(
        reissued,
        json!({ "decision": "allow" }),
        "the call runs once the memory has been shown"
    );
    assert_eq!(send(&server, None, &model_call(prompt, 2)), json!({}));
}

/// Detects the model's last words turning nothing on, the scope they turn on
/// being lost because the end of a turn carries no text, and a turn that cannot
/// end: answering `Stop` with `continue` makes the agent go on.
#[test]
fn what_a_turn_ends_on_is_delivered_when_the_next_turn_begins() {
    let server = TestServer::start(example_store_files(), |_| {});
    send(&server, None, &session_start("hello"));
    send(&server, None, &model_call("hello", 0));

    let stop = send(
        &server,
        Some("The next step is the rocket's second stage."),
        &event(
            "Stop",
            "hello",
            json!({ "executionNum": 0, "terminationReason": "NO_TOOL_CALL", "fullyIdle": true }),
        ),
    );
    assert!(
        stop["decision"]
            .as_str()
            .is_some_and(|decision| decision != "continue"),
        "the turn must be let end, got {stop}"
    );
    assert!(system_message(&stop).is_none(), "got {stop}");

    let next_turn = send(&server, None, &model_call("go on", 0));
    let text = system_message(&next_turn)
        .unwrap_or_else(|| panic!("what the last turn made due must arrive: {next_turn}"));
    assert!(text.contains(ROCKET_STAGES_DESCRIPTION), "got {text:?}");
}

/// Detects an event this server does nothing at answered with an error or with
/// keys: Antigravity runs the hook for whatever its configuration lists, and
/// the empty object is the answer that changes nothing.
#[test]
fn an_event_nothing_is_done_at_is_answered_with_the_empty_object() {
    let server = TestServer::start(example_store_files(), |_| {});

    for name in ["PostToolUse", "PostInvocation", "SomeEventAddedLater"] {
        let answer = send(&server, None, &event(name, "hello", json!({})));
        assert_eq!(answer, json!({}), "{name}");
    }
}

/// Detects the client and the server disagreeing about the harness on the
/// wire: an event the client forwards without naming Antigravity is read as
/// Claude Code's, refused, and the session gets no memory at all. Runs the
/// compiled client, which is the only place the flag becomes a request.
#[test]
fn the_client_run_for_antigravity_delivers_a_session_start() {
    let server = TestServer::start(example_store_files(), |_| {});
    let payload = serde_json::to_vec(&session_start("hello")).expect("the payload serialises");

    let run = run_antigravity_hook_client(&server.url(), "alpha", &payload);

    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr_text());
    let answer = run.single_json_object().expect("one JSON object on stdout");
    let text = system_message(&answer)
        .unwrap_or_else(|| panic!("the memories must reach stdout: {answer}"));
    assert!(text.contains(BENCH_POWER_BODY), "got {text:?}");
}

/// Detects a memory server that is down stopping the agent's tools: Antigravity
/// requires a decision for every tool call, so a client that prints nothing, or
/// fails, leaves the call without one. The client must print the decision that
/// lets the call run, say what failed on stderr and nowhere else, and succeed.
#[test]
fn the_client_lets_a_tool_call_run_when_the_server_cannot_be_reached() {
    // A port nothing listens on: bound to find a free one, then released.
    let unused = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let address = format!("http://{}", unused.local_addr().expect("an address"));
    drop(unused);
    let payload =
        serde_json::to_vec(&run_command("hello", "cargo test")).expect("the payload serialises");

    let run = run_antigravity_hook_client(&address, "alpha", &payload);

    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr_text());
    assert_eq!(
        run.single_json_object().expect("one JSON object on stdout"),
        json!({ "decision": "allow" })
    );
    assert!(
        run.stderr_text().contains("forgetmenot-hook:"),
        "the failure must be reported on stderr, got {:?}",
        run.stderr_text()
    );
}
