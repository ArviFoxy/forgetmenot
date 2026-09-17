//! The real hook client in front of the server, on the transcripts Claude Code
//! writes.
//!
//! Everything here is a property of the chain rather than of either end: the
//! client reads the transcript and the subagent's metadata file, builds the
//! request body, and copies the answer to stdout, and none of that is visible to
//! a scenario that posts to `/hook` itself. What the answer says is asserted
//! where the answer is decided; what is asserted here is that the client found
//! what is only in the files, and handed over what the server wrote, whole.

mod common;
mod scenario;

use std::time::{Duration, Instant};

use scenario::{Store, StoreBuilder, World};
use serde_json::json;

/// The machine these scenarios run on.
const ALPHA: &str = "alpha";

/// A directory that matches no trigger of the example store.
const NEUTRAL_DIRECTORY: &str = "/home/dev/notes";

/// The limit on one whole run of the client at `SessionStart`, process spawn
/// included. Source: the README's runtime limits, which `limits_test` asserts
/// the same number against.
const SESSION_START_CLIENT_LIMIT: Duration = Duration::from_millis(200);

/// The transcript size the limit is stated for. Source: the README.
const TRANSCRIPT_BYTES: u64 = 40 * 1024 * 1024;

/// The context size the big transcript reports; any value the stale rule can
/// use.
const CONTEXT_TOKENS: u64 = 40_000;

/// The name the user gave the session in the big transcript, written near the
/// start of the file where only a scan of the whole transcript reaches it.
/// Distinctive, so that a name read back cannot have come from anywhere else in
/// the file, the event or the store.
const TRANSCRIPT_TITLE: &str = "Rebuild the vacuum former thermocouple rig";

/// The name the user gives with `/rename` in the small-transcript scenario.
const RENAMED: &str = "Bracket rework";

/// What a subagent is asked to do, which is in its metadata file and in no hook
/// event.
const SUBAGENT_TASK: &str = "Survey the rocketry crate";

/// The memories that make the start answer long. Eight of them, each of
/// [`BODY_LINES`] lines, which puts the answer over [`LONG_ANSWER_CHARACTERS`]
/// and well past the threshold at which Claude Code saves an answer to a file.
const LONG_MEMORIES: [&str; 8] = [
    "bench-log",
    "meter-care",
    "rail-checks",
    "solder-rules",
    "crimp-rules",
    "labelling",
    "fixture-care",
    "spares-order",
];
const BODY_LINES: usize = 36;

/// The size the long answer has to reach for this scenario to be about a long
/// answer at all: far past the 10 000 characters at which Claude Code stops
/// showing an answer and saves it to a file, and past any single pipe buffer.
const LONG_ANSWER_CHARACTERS: usize = 34_000;

/// One long critical memory's body: a heading and [`BODY_LINES`] lines that name
/// the memory, so that a line of one body cannot be mistaken for a line of
/// another and a truncated answer cannot pass by accident.
fn long_body(name: &str) -> String {
    let mut text = format!("# The {name} rules\n\n");
    for index in 0..BODY_LINES {
        text.push_str(&format!(
            "{name} rule {index}: the bench log records the measurement, \
             the instrument it was taken with and the settling time.\n"
        ));
    }
    text
}

/// The example store with [`LONG_MEMORIES`] added, every one of them critical
/// and global, so all of them are due at any session's first event.
fn long_store() -> StoreBuilder {
    let mut store = Store::example();
    for name in LONG_MEMORIES {
        store = store.memory(name, |memory| {
            memory
                .critical()
                .description("Bench rules recorded for the workshop")
                .body(&long_body(name));
        });
    }
    store
}

/// Detects a `SessionStart` that has become slow enough to be felt as a hanging
/// harness, and a whole-transcript scan narrowed back to the tail, over the
/// chain Claude Code actually runs: the client spawned as a process with the
/// event on stdin, the server answering over loopback, and the answer parsed
/// back.
///
/// `limits_test` times the same event against a recorded payload; what is added
/// here is the simulator's own transcript and session, so the measurement covers
/// the shape a session of this suite produces rather than a fixture alone.
///
/// The name is the check that the run did the work: it is on the second line of
/// a 40 MB file, past every tail window, so a run that skipped the scan is fast
/// and reports nothing. Tolerance: the stated limit is 200 ms and the measured
/// figure on the development machine is under half of it, so a conforming
/// implementation has room while a comparison per byte of 40 MB does not.
#[test]
fn a_session_start_with_a_forty_megabyte_transcript_stays_under_the_limit_and_reports_the_title() {
    let world = World::new().through_client().build();
    let session = world.claude(ALPHA).session();
    common::write_transcript_with_custom_title(
        session.transcript(),
        session.session_id(),
        TRANSCRIPT_TITLE,
        CONTEXT_TOKENS,
        TRANSCRIPT_BYTES,
    );
    let written = std::fs::metadata(session.transcript())
        .expect("the transcript was written")
        .len();
    assert!(
        written >= TRANSCRIPT_BYTES,
        "the transcript must be at least {TRANSCRIPT_BYTES} bytes, got {written}"
    );
    session.at_tokens(CONTEXT_TOKENS);

    let started = Instant::now();
    session.start_in(NEUTRAL_DIRECTORY);
    let elapsed = started.elapsed();

    println!("session start through the client with a {written} byte transcript: {elapsed:?}");
    assert_eq!(
        world.context(&session.key())["name"],
        json!(TRANSCRIPT_TITLE),
        "the run must have found the title near the start of the {written} byte transcript"
    );
    assert!(
        elapsed < SESSION_START_CLIENT_LIMIT,
        "one session start through the client with a {written} byte transcript took {elapsed:?}, \
         over the {SESSION_START_CLIENT_LIMIT:?} limit"
    );
}

/// Detects a rename that never leaves the transcript: the contexts page would go
/// on listing the session under its first prompt for the rest of its life, and
/// the user would have no way to name a session at all.
///
/// No hook event carries a name. `/rename` writes a line into the transcript and
/// nothing else happens, so the name reaches the server only because the client
/// reads the transcript at the next event, whatever that event is. The rename is
/// made after the session start, so the name cannot have come from the start's
/// own read.
#[test]
fn a_rename_reaches_the_contexts_page_at_the_next_event() {
    let world = World::new().through_client().build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL_DIRECTORY);
    assert_eq!(
        world.context(&session.key())["title"],
        json!(null),
        "the session has not been named yet"
    );

    session.rename(RENAMED);
    session.prompt("carry on where we left off");

    assert_eq!(
        world.context(&session.key())["title"],
        json!(RENAMED),
        "the name the user gave reaches the page at the first event after the rename"
    );
}

/// Detects a subagent whose task never reaches the server: the contexts page
/// would list every subagent of a session under the same name, and the scopes a
/// subagent works in would be decided without the words it was spawned for.
///
/// A `SubagentStart` carries the agent's id and not its task. The task is in the
/// metadata file Claude Code writes beside the session's transcript, which only
/// the client reads, so a client that does not find that file leaves the task
/// empty and the page falls back to the kind of subagent. The name is asked for
/// as well because it is derived from the task and is what the page shows.
#[test]
fn a_subagents_task_comes_from_its_meta_file() {
    let world = World::new().through_client().build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL_DIRECTORY);
    let child = session.subagent("general-purpose", SUBAGENT_TASK);

    let row = world.context(&child.key());
    assert_eq!(
        row["task"],
        json!(SUBAGENT_TASK),
        "the task must be the text of the metadata file the client read"
    );
    assert_eq!(
        row["name"],
        json!(SUBAGENT_TASK),
        "the page names a subagent by what it was asked to do, got {row}"
    );
}

/// Detects an answer cut short between the server and the model: a read of one
/// pipe buffer, a line-oriented copy, or a write that is not waited for. An
/// answer this long is the one case where that is possible at all, and it is the
/// case that matters, because a cut answer loses the critical memories at the
/// end of it, silently.
///
/// The store is built so that the start answer is over 34 000 characters, made
/// of eight critical memories whose lines name themselves. Each body arriving
/// whole is the check: the expectation comes from the store the test wrote, not
/// from the answer, so an answer cut anywhere past the first memory fails. The
/// answer is also past the 10 000 characters at which Claude Code saves an
/// answer to a file and shows the model a preview, so it must open with the
/// notice that tells the model to read the file, and the file Claude Code saves
/// must hold the whole of what the client printed rather than the preview.
#[test]
fn a_long_answer_is_copied_by_the_client_byte_for_byte() {
    let world = World::new().store(long_store()).through_client().build();
    let session = world.claude(ALPHA).session();

    let answer = session.start_in(NEUTRAL_DIRECTORY).assert(|answer| {
        for name in LONG_MEMORIES {
            assert!(
                answer.delivers_full(name),
                "{name} must arrive whole through the client"
            );
        }
        assert!(
            answer.chars() >= LONG_ANSWER_CHARACTERS,
            "the answer must be the long one this scenario is about, got {} characters",
            answer.chars()
        );
        assert!(
            answer.starts_with_notice(),
            "an answer Claude Code will save to a file opens with the notice to read it"
        );
    });

    let saved = session.persisted_files();
    let saved = saved
        .first()
        .expect("Claude Code saves an answer this long to a file");
    let text = std::fs::read_to_string(saved).expect("the saved answer is readable");
    assert_eq!(
        text.encode_utf16().count(),
        answer.chars(),
        "the file holds the whole answer the client printed, not the preview the model was shown"
    );
}
