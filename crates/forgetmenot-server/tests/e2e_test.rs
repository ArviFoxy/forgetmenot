//! The whole chain: the compiled client binary, a real transcript on disk, a
//! real server on loopback, and the bytes that land on stdout.
//!
//! Everything here is asserted on what Claude Code can see: the process's exit
//! code, its stdout, its stderr. The tests exist for the failures that live
//! between the parts and are invisible inside any one of them — a wire
//! mismatch, a second document or a log line on stdout, a context size that
//! never leaves the client, and a failure that writes to stdout and so injects
//! junk into a session.
//!
//! Expectation sources: the plan's hook contract (stdout is one JSON object;
//! the answer names the event; a failure writes nothing to stdout and exits 1),
//! the plan's state machine (what a session start and a stale delivery owe),
//! and the example store committed in this repository.

mod common;

use std::path::Path;

use common::{
    ClientRun, TestServer, additional_context, example_store_files, hook_fixture_names,
    hook_fixture_payload, run_hook_client, write_transcript,
};
use serde_json::json;
use tempfile::TempDir;

/// A line of the example store's global critical memory and of nothing else, so
/// that "delivered in full" can be told from "named in an index line".
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";

/// The context size the transcripts written here report, chosen away from zero
/// so that a client sending a constant is not mistaken for a working one.
const BASE_TOKENS: u64 = 40_000;

/// Transcripts these tests write are small: the size of the read is the
/// client's own test, and the 40 MB case is in the limits test.
const SMALL_TRANSCRIPT: u64 = 8 * 1024;

/// A transcript reporting `context_tokens` in a temporary directory.
fn transcript(directory: &TempDir, name: &str, context_tokens: u64) -> std::path::PathBuf {
    let path = directory.path().join(name);
    write_transcript(&path, context_tokens, SMALL_TRANSCRIPT);
    path
}

/// Run one recorded payload through the client against `server`, with a
/// transcript reporting `context_tokens`.
fn run_fixture(
    server: &TestServer,
    fixture: &str,
    transcript_path: &Path,
    machine: &str,
) -> ClientRun {
    let payload = hook_fixture_payload(fixture, transcript_path);
    run_hook_client(&server.url(), machine, &payload)
}

/// Detects every way the chain can break on a payload Claude Code really sends:
/// a client or server that cannot read the payload, a non-zero exit (Claude Code
/// reports the hook as failed), anything on stdout that is not exactly one JSON
/// object (Claude Code parses stdout as the whole answer), and noise on stderr,
/// which the user sees as a broken hook.
///
/// Every payload in the fixture directory is run, so a payload added there is
/// covered without a change to this test.
#[test]
fn every_recorded_payload_through_the_client_exits_zero_with_one_json_object_on_stdout() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = transcript(&directory, "session-1.jsonl", BASE_TOKENS);

    for fixture in hook_fixture_names() {
        // A server per payload, so that what one payload delivered cannot
        // change what the next one is owed.
        let server = TestServer::start(example_store_files(), |_| {});
        let run = run_fixture(&server, &fixture, &path, "alpha");

        assert_eq!(
            run.code,
            Some(0),
            "{fixture}: the client must exit 0; stderr was {}",
            run.stderr_text()
        );
        assert!(
            run.stderr.is_empty(),
            "{fixture}: a served event must report nothing on stderr, got {:?}",
            run.stderr_text()
        );
        let answer = run.single_json_object().unwrap_or_else(|problem| {
            panic!("{fixture}: {problem}, stdout was {:?}", run.stdout_text())
        });
        assert!(
            answer.get("hookSpecificOutput").is_some()
                || answer.as_object().is_some_and(|fields| fields.is_empty()),
            "{fixture}: the answer must be a hook response or the empty object, got {answer}"
        );
    }
}

/// Detects a chain that delivers nothing at a session start, and an answer whose
/// event name does not match the event: Claude Code drops an answer whose
/// `hookEventName` is not the one it asked about, so the memory would never
/// arrive even though every part looked healthy.
#[test]
fn a_session_start_through_the_client_injects_the_global_critical_memory() {
    let server = TestServer::start(example_store_files(), |_| {});
    let directory = TempDir::new().expect("a temporary directory");
    let path = transcript(&directory, "session-1.jsonl", BASE_TOKENS);

    let run = run_fixture(&server, "session_start", &path, "alpha");

    let answer = run
        .single_json_object()
        .unwrap_or_else(|problem| panic!("{problem}, stdout was {:?}", run.stdout_text()));
    assert_eq!(
        answer["hookSpecificOutput"]["hookEventName"],
        json!("SessionStart"),
        "the answer must name the event it answers, got {answer}"
    );
    let injected = additional_context(&answer).unwrap_or_default();
    assert!(
        injected.contains(BENCH_POWER_BODY),
        "the global critical memory must arrive in full through the client, got {injected:?}"
    );
}

/// Detects a client that turns an event name it does not know into an error, a
/// usage line, or an empty stdout: Claude Code would report a failing hook on
/// every event a newer version adds.
#[test]
fn an_event_name_nothing_knows_yet_puts_the_empty_object_on_stdout() {
    let server = TestServer::start(example_store_files(), |_| {});
    let directory = TempDir::new().expect("a temporary directory");
    let path = transcript(&directory, "session-1.jsonl", BASE_TOKENS);

    let run = run_fixture(&server, "unknown_event", &path, "alpha");

    assert_eq!(run.code, Some(0), "an unknown event must not fail the hook");
    let answer = run
        .single_json_object()
        .unwrap_or_else(|problem| panic!("{problem}, stdout was {:?}", run.stdout_text()));
    assert_eq!(
        answer,
        json!({}),
        "an unknown event must be acknowledged with an answer that asks for nothing, got {answer}"
    );
}

/// Detects a context size that does not reach the server: a client that never
/// reads the transcript, sends `null`, sends a constant, or reads a counter
/// other than the context size.
///
/// The chain is observed through the one rule that depends on the number: two
/// identical tool calls whose transcripts differ by exactly the stale threshold
/// must make the second one deliver the critical memories again. With no
/// context size at either end, nothing is owed at the second call and the
/// answer carries nothing.
#[test]
fn the_context_size_read_from_the_transcript_reaches_the_stale_rule() {
    let reminder_tokens = 1_000;
    let server = TestServer::start(
        common::example_store_with_settings(&format!("reminder_tokens: {reminder_tokens}\n")),
        |_| {},
    );
    let directory = TempDir::new().expect("a temporary directory");
    let before = transcript(&directory, "before.jsonl", BASE_TOKENS);
    let after = transcript(&directory, "after.jsonl", BASE_TOKENS + reminder_tokens);

    let first = run_fixture(&server, "pre_tool_use_bash", &before, "alpha");
    let first_answer = first
        .single_json_object()
        .unwrap_or_else(|problem| panic!("{problem}, stdout was {:?}", first.stdout_text()));
    assert!(
        additional_context(&first_answer)
            .unwrap_or_default()
            .contains(BENCH_POWER_BODY),
        "the first call must be given the critical memory, got {first_answer}"
    );

    let second = run_fixture(&server, "pre_tool_use_bash", &after, "alpha");

    let second_answer = second
        .single_json_object()
        .unwrap_or_else(|problem| panic!("{problem}, stdout was {:?}", second.stdout_text()));
    assert!(
        additional_context(&second_answer)
            .unwrap_or_default()
            .contains(BENCH_POWER_BODY),
        "a context grown by the stale threshold must be given the memory again, \
         which it can only be if the size the client read reached the server; got {second_answer}"
    );
}

/// Detects a chain that does not fail closed when the server it was talking to
/// goes away: anything on stdout is injected into the session, and a silent exit
/// leaves nobody aware that memory delivery has stopped.
///
/// The hook crate tests the same exit against a port that never listened; this
/// one is the case the user meets, a server that was answering and stops.
#[test]
fn the_client_stays_off_stdout_when_the_server_it_was_using_has_stopped() {
    let directory = TempDir::new().expect("a temporary directory");
    let path = transcript(&directory, "session-1.jsonl", BASE_TOKENS);
    let payload = hook_fixture_payload("pre_tool_use_bash", &path);
    let url = {
        let server = TestServer::start(example_store_files(), |_| {});
        let url = server.url();
        let run = run_hook_client(&url, "alpha", &payload);
        assert_eq!(
            run.code,
            Some(0),
            "the chain must work before the server is stopped; stderr was {}",
            run.stderr_text()
        );
        url
    };

    let run = run_hook_client(&url, "alpha", &payload);

    assert_eq!(
        run.code,
        Some(1),
        "a server that has stopped must make the client exit 1"
    );
    assert!(
        run.stdout.is_empty(),
        "nothing may reach stdout when the server is gone, got {:?}",
        run.stdout_text()
    );
    assert!(
        !run.stderr.is_empty(),
        "the failure must be reported on stderr so the user can see it"
    );
}
