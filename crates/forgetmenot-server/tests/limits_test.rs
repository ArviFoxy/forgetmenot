//! The project's runtime limits, which are requirements and not targets.
//!
//! Source of both numbers: the plan's build order, which records them in the
//! README as well. A hook runs on every Claude Code event and a slow one is felt
//! as a slow harness, so the limits are asserted here rather than watched.
//!
//! Every test reports the measured figure in its failure message, and prints it
//! under `cargo test -- --nocapture`, so a run on another machine says how much
//! room is left rather than only whether it passed.

mod common;

use std::time::{Duration, Instant};

use common::{
    TestServer, example_store_files, hook_fixture_payload, run_hook_client_binary,
    write_subagent_meta, write_transcript, write_transcript_with_custom_title,
};
use serde_json::json;

/// The limit on the 99th percentile of `/hook`, over loopback, measured by the
/// caller and so including the HTTP round trip the real client pays.
const HOOK_P99_LIMIT: Duration = Duration::from_millis(50);

/// How many events the percentile is taken over. Source: the plan.
const EVENTS: usize = 200;

/// The limit on one whole run of the client, from spawning the process to its
/// exit, with a transcript of the size a long session reaches. It is the limit
/// at every event but `SessionStart`, which reads more of the transcript and
/// has [`SESSION_START_CLIENT_LIMIT`] of its own.
const CLIENT_LIMIT: Duration = Duration::from_millis(30);

/// The limit on one whole run of the client at `SessionStart`, the one event
/// that reads the whole transcript instead of its tail, so that a session named
/// long ago is still named after its transcript has grown past the tail window.
/// It happens once per session, which is what buys it the larger budget.
const SESSION_START_CLIENT_LIMIT: Duration = Duration::from_millis(200);

/// The session the recorded payloads belong to, which is the session whose
/// transcript the timed runs read.
const SESSION_ID: &str = "session-1";

/// The name the session start test gives its session. Distinctive, so that a
/// name read back cannot have come from anywhere else in the transcript, the
/// event or the store.
const SESSION_TITLE: &str = "Rebuild the vacuum former thermocouple rig";

/// The subagent the timed tool call is made inside, as the recorded payload
/// names it, and what its metadata file says it was asked to do.
const SUBAGENT_ID: &str = "agent-7f3a";
const SUBAGENT_TASK: &str = "Survey the rocketry crate and list its public functions";

/// The transcript size the client limit is stated for. Source: the plan.
const TRANSCRIPT_BYTES: u64 = 40 * 1024 * 1024;

/// The context size the transcript reports; any value the stale rule can use.
const CONTEXT_TOKENS: u64 = 40_000;

/// A tool call that matches no trigger of the example store, so that every
/// timed event does the same work: the whole pass over the catalog, the context's
/// critical section and the statistics record, with nothing to deliver.
fn steady_state_event(index: usize) -> serde_json::Value {
    json!({
        "hook_event_name": "PreToolUse",
        "session_id": "session-1",
        "transcript_path": "/nonexistent/transcript.jsonl",
        "cwd": "/home/dev/notes",
        "permission_mode": "default",
        "tool_name": "Read",
        "tool_use_id": format!("toolu_{index:08}"),
        "tool_input": { "file_path": "/home/dev/notes/README.md" }
    })
}

/// The value at the 99th percentile of `samples`, which must be sorted: the
/// smallest sample that at least 99% of the samples are not above.
fn percentile_99(samples: &[Duration]) -> Duration {
    let rank = (samples.len() as f64 * 0.99).ceil() as usize;
    samples[rank.max(1) - 1]
}

/// Detects a hook endpoint that is too slow to sit in front of every Claude Code
/// event: work done per event that belongs in the catalog snapshot, a lock held
/// across the store, or a statistics write that blocks the answer. Any of those
/// shows up as a tail far past the limit rather than as a wrong answer.
///
/// Tolerance: the limit is the stated one, 50 ms, and the measured p99 on the
/// development machine in the dev profile is about two orders of magnitude below
/// it, so every implementation that keeps the store off the hot path passes and a
/// per-event git or sqlite round trip does not.
#[test]
fn the_hook_endpoint_answers_200_events_within_the_p99_limit() {
    let server = TestServer::start(example_store_files(), |_| {});
    // The warm-up event is also the one that has something to deliver: it opens
    // the connection path, builds the catalog snapshot and fills the context's
    // delivery record, which is the state a session spends its life in.
    let (status, _) = server.hook(
        "alpha",
        Some(CONTEXT_TOKENS),
        &json!({
            "hook_event_name": "SessionStart",
            "session_id": "session-1",
            "cwd": "/home/dev/notes",
            "source": "startup"
        }),
    );
    assert_eq!(status, 200, "the warm-up event must be answered");

    let mut samples = Vec::with_capacity(EVENTS);
    for index in 0..EVENTS {
        let event = steady_state_event(index);
        let started = Instant::now();
        let (status, answer) = server.hook("alpha", Some(CONTEXT_TOKENS), &event);
        samples.push(started.elapsed());
        assert_eq!(status, 200, "event {index} must be answered, got {answer}");
    }
    samples.sort();

    let p99 = percentile_99(&samples);
    println!(
        "hook over {EVENTS} events: p50 {:?}, p99 {p99:?}, max {:?}",
        samples[samples.len() / 2],
        samples[samples.len() - 1]
    );
    assert!(
        p99 < HOOK_P99_LIMIT,
        "the p99 of {EVENTS} hook events was {p99:?}, over the {HOOK_P99_LIMIT:?} limit"
    );
}

/// Detects a client that reads the whole transcript instead of its tail, and any
/// other cost that grows with the session: the hook is on Claude Code's critical
/// path, so a client that takes a second on a long session stalls every tool
/// call.
///
/// The transcript is the shape a long session has, 40 MB with its last assistant
/// message at the end. The pathological shape, an assistant line only at the very
/// start, is the hook crate's own window-doubling test.
///
/// Tolerance: the stated limit is 30 ms for one whole run, process spawn
/// included. Measured on the development machine with the dev-profile binary
/// against a real server: 4.4 to 4.8 ms with the subagent metadata read
/// included (2026-09-13), an order of magnitude under the limit, so the limit is
/// asserted against the binary the test suite builds rather than against a
/// release build only. `FORGETMENOT_RELEASE_BIN` runs the same check against
/// another build of the client when one is wanted.
#[test]
fn one_client_run_with_a_40_megabyte_transcript_stays_under_the_limit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let directory = tempfile::tempdir().expect("a temporary directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript(&transcript, CONTEXT_TOKENS, TRANSCRIPT_BYTES);
    let written = std::fs::metadata(&transcript)
        .expect("the transcript was written")
        .len();
    assert!(
        written >= TRANSCRIPT_BYTES,
        "the transcript must be at least {TRANSCRIPT_BYTES} bytes, got {written}"
    );
    // A tool call, not a session start: this limit is the one for the events
    // that read the tail of the transcript, and a session start reads all of it.
    // The call is one made inside a subagent, which is the most a non-start
    // event does: the transcript tail, the first prompt and the subagent's
    // metadata file.
    write_subagent_meta(&transcript, SUBAGENT_ID, SUBAGENT_TASK);
    let payload = hook_fixture_payload("pre_tool_use_in_subagent", &transcript);
    let binary = match std::env::var_os("FORGETMENOT_RELEASE_BIN") {
        Some(path) => std::path::PathBuf::from(path),
        None => common::hook_client_binary(),
    };

    let started = Instant::now();
    let run = run_hook_client_binary(&binary, &server.url(), "alpha", &payload);
    let elapsed = started.elapsed();

    assert_eq!(
        run.code,
        Some(0),
        "the timed run must have succeeded; stderr was {}",
        run.stderr_text()
    );
    run.single_json_object()
        .unwrap_or_else(|problem| panic!("the timed run must answer with one object: {problem}"));
    println!(
        "client end to end with a {written} byte transcript, {}: {elapsed:?}",
        binary.display()
    );
    assert!(
        elapsed < CLIENT_LIMIT,
        "one client run with a {written} byte transcript took {elapsed:?}, \
         over the {CLIENT_LIMIT:?} limit"
    );
}

/// Detects a `SessionStart` that has become slow enough to be felt as a hanging
/// harness: the whole transcript is read at that event, so a byte-at-a-time
/// scan, a second pass over the file, or the file being read into memory whole
/// all show up here. It also detects the scan being narrowed back to the tail,
/// through the name the run must have found.
///
/// The transcript is 40 MB with its `custom-title` line near the start, which is
/// where a session named at its beginning has it after a long session has grown
/// past it. Only a scan of the whole file reaches it.
///
/// Tolerance: the stated limit is 200 ms for one whole run, process spawn
/// included. Measured on the development machine with the dev-profile binary
/// against a real server: 88 to 91 ms, so a conforming implementation has room
/// to spare while a scan that costs a comparison per byte of 40 MB, measured at
/// 113 ms for the scan alone, does not fit beside the rest of the run.
/// `FORGETMENOT_RELEASE_BIN` runs the same check against another build.
#[test]
fn one_session_start_client_run_with_a_40_megabyte_transcript_stays_under_the_limit() {
    let server = TestServer::start(example_store_files(), |_| {});
    let directory = tempfile::tempdir().expect("a temporary directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_with_custom_title(
        &transcript,
        SESSION_ID,
        SESSION_TITLE,
        CONTEXT_TOKENS,
        TRANSCRIPT_BYTES,
    );
    let written = std::fs::metadata(&transcript)
        .expect("the transcript was written")
        .len();
    assert!(
        written >= TRANSCRIPT_BYTES,
        "the transcript must be at least {TRANSCRIPT_BYTES} bytes, got {written}"
    );
    let payload = hook_fixture_payload("session_start", &transcript);
    let binary = match std::env::var_os("FORGETMENOT_RELEASE_BIN") {
        Some(path) => std::path::PathBuf::from(path),
        None => common::hook_client_binary(),
    };

    let started = Instant::now();
    let run = run_hook_client_binary(&binary, &server.url(), "alpha", &payload);
    let elapsed = started.elapsed();

    assert_eq!(
        run.code,
        Some(0),
        "the timed run must have succeeded; stderr was {}",
        run.stderr_text()
    );
    run.single_json_object()
        .unwrap_or_else(|problem| panic!("the timed run must answer with one object: {problem}"));
    println!(
        "client end to end at SessionStart with a {written} byte transcript, {}: {elapsed:?}",
        binary.display()
    );

    // What the run did, before how long it took: a run that skipped the scan is
    // fast and is not a measurement of anything.
    let (status, contexts) = server.api("GET", "/api/contexts", None);
    assert_eq!(status, 200, "the contexts must be readable, got {contexts}");
    let row = contexts
        .as_array()
        .expect("the contexts are a list")
        .iter()
        .find(|row| row["key"] == json!("alpha/session-1"))
        .unwrap_or_else(|| panic!("the session the run was for must be listed, got {contexts}"));
    assert_eq!(
        row["name"],
        json!(SESSION_TITLE),
        "the timed run must have found the title near the start of the {written} byte \
         transcript, got {row}"
    );

    assert!(
        elapsed < SESSION_START_CLIENT_LIMIT,
        "one SessionStart client run with a {written} byte transcript took {elapsed:?}, \
         over the {SESSION_START_CLIENT_LIMIT:?} limit"
    );
}
