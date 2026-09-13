//! Tests for the hook client: the transcript tail read, and the binary's
//! behaviour against a real socket.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use forgetmenot_hook::transcript_tail::last_assistant_context_tokens;
use forgetmenot_types::hook::HookRequest;

/// A transcript written by the test, together with what the writer knows the
/// answer to be. The expected value is arithmetic done by the generator, not a
/// value read back out of the file.
struct GeneratedTranscript {
    expected_context_tokens: u64,
    assistant_line_count: u64,
}

/// Write a transcript of at least `minimum_bytes`, shaped like the awkward case:
/// the last assistant line is followed by several lines of other types and then
/// by a line the writer did not finish.
///
/// Every value is derived from the line index, so the file is the same on every
/// run.
fn write_transcript(path: &Path, minimum_bytes: u64) -> GeneratedTranscript {
    let padding = "x".repeat(400);
    let mut file = BufWriter::new(File::create(path).expect("the transcript must be creatable"));
    let mut written = 0u64;
    let mut assistant_line_count = 0u64;
    let mut last_assistant_sum = 0u64;

    while written < minimum_bytes {
        let index = assistant_line_count;
        let user_line = format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"turn {index} {padding}\"}}}}\n"
        );
        let input_tokens = 1_000 + index * 3;
        let cache_read = 20_000 + index * 7;
        let cache_creation = index * 11;
        let assistant_line = format!(
            "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input_tokens\":{input_tokens},\"cache_read_input_tokens\":{cache_read},\"cache_creation_input_tokens\":{cache_creation},\"output_tokens\":17}},\"content\":\"reply {index} {padding}\"}}}}\n"
        );
        file.write_all(user_line.as_bytes()).expect("write");
        file.write_all(assistant_line.as_bytes()).expect("write");
        written += (user_line.len() + assistant_line.len()) as u64;
        assistant_line_count += 1;
        last_assistant_sum = input_tokens + cache_read + cache_creation;
    }

    // What makes the tail read non-trivial: the answer is not on the last line.
    for index in 0..4 {
        let trailing = format!(
            "{{\"type\":\"system\",\"subtype\":\"post_tool\",\"index\":{index},\"text\":\"{padding}\"}}\n"
        );
        file.write_all(trailing.as_bytes()).expect("write");
    }
    file.write_all(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"cont")
        .expect("write");
    file.flush().expect("flush");

    GeneratedTranscript {
        expected_context_tokens: last_assistant_sum,
        assistant_line_count,
    }
}

/// Independent implementation of the same question, reading every line. Used
/// as a second source for the expected sum and as a check on the generator.
fn context_tokens_by_reading_everything(path: &Path) -> (Option<u64>, u64) {
    let text = std::fs::read_to_string(path).expect("the transcript must be readable");
    let mut answer = None;
    let mut assistant_line_count = 0;
    for line in text.lines() {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if parsed.get("type").and_then(|value| value.as_str()) != Some("assistant") {
            continue;
        }
        let Some(usage) = parsed
            .get("message")
            .and_then(|message| message.get("usage"))
        else {
            continue;
        };
        assistant_line_count += 1;
        let counter = |name: &str| usage.get(name).and_then(|v| v.as_u64()).unwrap_or(0);
        answer = Some(
            counter("input_tokens")
                + counter("cache_read_input_tokens")
                + counter("cache_creation_input_tokens"),
        );
    }
    (answer, assistant_line_count)
}

// Detects a generator that does not write what it claims to write: without
// this, the size and the expected sum used by the tail test are unchecked, and
// a silent bug in the generator would make the tail test pass on anything.
#[test]
fn transcript_generator_writes_the_lines_and_the_sum_it_reports() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("session-1.jsonl");
    let generated = write_transcript(&path, 64 * 1024);

    let (sum, assistant_line_count) = context_tokens_by_reading_everything(&path);
    assert_eq!(
        sum,
        Some(generated.expected_context_tokens),
        "the sum the generator reports must be the sum of the last assistant line it wrote"
    );
    assert_eq!(
        assistant_line_count, generated.assistant_line_count,
        "the generator must write as many assistant lines as it reports"
    );
}

// Detects a tail read that picks an assistant line other than the last one, and
// a tail read that walks the whole file: the hook runs on every Claude Code
// event, so the read is on the harness's critical path. Tolerance: on this
// machine, in a debug build, the windowed read of this file takes under 1 ms
// and a read of the whole file takes 437 ms, so 100 ms passes every windowed
// implementation and fails every whole-file one.
#[test]
fn transcript_tail_sums_the_last_assistant_usage_within_the_time_budget() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("session-1.jsonl");
    let generated = write_transcript(&path, 30 * 1024 * 1024);
    let (independent_sum, _) = context_tokens_by_reading_everything(&path);

    let started = Instant::now();
    let tokens = last_assistant_context_tokens(&path);
    let elapsed = started.elapsed();

    assert_eq!(
        tokens,
        Some(generated.expected_context_tokens),
        "the tail read must report the sum of the last assistant line's usage"
    );
    assert_eq!(
        tokens, independent_sum,
        "the tail read must agree with a read of the whole file"
    );
    assert!(
        elapsed < Duration::from_millis(100),
        "the tail read of a 30 MB transcript took {elapsed:?}, over the 100 ms budget"
    );
}

// Detects a broken window-doubling scan: when the last assistant line is
// further back than the first window, a scan that gives up, loops forever or
// mis-slices the next window reports the wrong context size for a session whose
// last turn produced large tool results.
#[test]
fn transcript_tail_finds_an_assistant_line_beyond_the_first_window() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("session-1.jsonl");
    let padding = "y".repeat(4_000);
    let mut file = BufWriter::new(File::create(&path).expect("creatable"));
    file.write_all(b"{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":3,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":7}}}\n")
        .expect("write");
    // More than twice the first window, so at least one doubling is needed.
    for index in 0..180 {
        file.write_all(
            format!("{{\"type\":\"system\",\"index\":{index},\"text\":\"{padding}\"}}\n")
                .as_bytes(),
        )
        .expect("write");
    }
    file.flush().expect("flush");
    let written = std::fs::metadata(&path).expect("metadata").len();
    assert!(
        written > 512 * 1024,
        "the file must be larger than two initial windows, got {written} bytes"
    );

    assert_eq!(
        last_assistant_context_tokens(&path),
        Some(15),
        "the scan must widen its window until it finds the assistant line, 3 + 5 + 7"
    );
}

// Detects an absent cache counter being treated as "no answer" or as a parse
// failure instead of zero: the first assistant message of a session has no
// cache read, and its context size is not unknown.
#[test]
fn transcript_tail_counts_an_absent_cache_counter_as_zero() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("session-1.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"user\",\"message\":{\"content\":\"hello\"}}\n\
         {\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":7,\"cache_creation_input_tokens\":3,\"output_tokens\":2}}}\n",
    )
    .expect("the transcript must be writable");

    assert_eq!(
        last_assistant_context_tokens(&path),
        Some(10),
        "counters Claude Code did not write count as zero, so 7 + 0 + 3"
    );
}

// Detects a missing transcript being reported as a context size of zero, which
// the server would read as "the context shrank" and re-deliver on.
#[test]
fn transcript_tail_reports_nothing_for_a_missing_file() {
    let directory = tempfile::tempdir().expect("a temp directory");

    assert_eq!(
        last_assistant_context_tokens(directory.path().join("not-written-yet.jsonl")),
        None,
        "a transcript that does not exist has no context size"
    );
}

// Detects the same confusion for a transcript that exists but has no assistant
// message yet, which is the state at SessionStart, and detects a scan that
// loops instead of stopping once the whole file has been covered.
#[test]
fn transcript_tail_reports_nothing_when_no_assistant_line_exists() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("session-1.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"user\",\"message\":{\"content\":\"hello\"}}\n\
         {\"type\":\"system\",\"subtype\":\"init\"}\n\
         not json at all\n",
    )
    .expect("the transcript must be writable");

    assert_eq!(
        last_assistant_context_tokens(&path),
        None,
        "a transcript with no assistant usage record has no context size"
    );
}

/// What the test's socket saw.
struct RecordedRequest {
    request_line: String,
    body: Vec<u8>,
}

/// Serve exactly one HTTP request on `listener`, answer it with `reply_body`,
/// and report what was received.
///
/// This is the other end of the wire contract, not a stand-in for the server's
/// behaviour: it speaks HTTP and records bytes, and decides nothing.
fn serve_one_request(listener: TcpListener, reply_body: &'static str) -> RecordedRequest {
    let (stream, _) = listener.accept().expect("the client must connect");
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .expect("a request line must arrive");
    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).expect("headers must arrive");
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().expect("a numeric content-length");
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).expect("the body must arrive");

    let stream: &mut TcpStream = reader.get_mut();
    let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply_body}",
        reply_body.len()
    );
    stream
        .write_all(reply.as_bytes())
        .expect("the reply must send");
    stream.flush().expect("the reply must flush");

    RecordedRequest {
        request_line: request_line.trim_end().to_string(),
        body,
    }
}

/// Run the compiled client with `stdin_payload` on stdin.
fn run_client(server: &str, machine: &str, stdin_payload: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_forgetmenot-hook"))
        .args(["--server", server, "--machine", machine])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the client binary must start");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(stdin_payload)
        .expect("the payload must be writable to the client");
    child.wait_with_output().expect("the client must finish")
}

// Detects every way the client can break the wire contract: a wrong path or
// method, a machine name or context size that does not reach the server, a
// hook payload the server cannot read back as a HookRequest, an altered event,
// and a reply that does not reach stdout byte for byte (Claude Code reads
// stdout as the hook's whole answer).
#[test]
fn client_posts_the_event_with_machine_and_context_tokens_and_copies_the_reply() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    std::fs::write(
        &transcript,
        "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":11,\"cache_read_input_tokens\":29,\"cache_creation_input_tokens\":2}}}\n",
    )
    .expect("the transcript must be writable");
    let event = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "session-1",
        "transcript_path": transcript.to_str().expect("a utf-8 temp path"),
        "cwd": directory.path().to_str().expect("a utf-8 temp path"),
        "permission_mode": "default",
        "tool_name": "Bash",
        "tool_use_id": "toolu_1",
        "tool_input": { "command": "ls -l", "description": "list files" }
    });
    let stdin_payload = format!("{}\n", serde_json::to_string(&event).expect("serialises"));

    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    let reply_body = "{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\"}}";
    let server_thread = std::thread::spawn(move || serve_one_request(listener, reply_body));

    let output = run_client(
        &format!("http://127.0.0.1:{port}"),
        "alpha",
        stdin_payload.as_bytes(),
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "a served request must exit 0; stderr was {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        reply_body,
        "the server's reply must reach stdout unchanged"
    );

    let recorded = server_thread.join().expect("the server thread must finish");
    assert!(
        recorded.request_line.starts_with("POST /hook "),
        "the client must POST to /hook, not {:?}",
        recorded.request_line
    );
    let request: HookRequest =
        serde_json::from_slice(&recorded.body).expect("the body must be a HookRequest");
    assert_eq!(
        request.machine, "alpha",
        "the machine name from the command line must reach the server"
    );
    assert_eq!(
        request.context_tokens,
        Some(42),
        "context_tokens must be the transcript's last assistant usage sum, 11 + 29 + 2"
    );
    assert_eq!(
        request.hook, event,
        "the hook event must reach the server as it arrived"
    );
}

// Detects a client that writes something to stdout when it cannot reach the
// server: Claude Code would inject that text, or a broken hook response, into
// the session. Also detects a silent failure, which would leave the user with
// no way to tell that memory delivery stopped.
#[test]
fn client_fails_without_touching_stdout_when_the_server_is_not_listening() {
    // Binding and dropping is how the test gets a port that refuses connections.
    let closed_port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.local_addr().expect("a bound address").port()
    };
    let payload = b"{\"hook_event_name\":\"Stop\",\"session_id\":\"session-1\"}";

    let output = run_client(&format!("http://127.0.0.1:{closed_port}"), "alpha", payload);

    assert_eq!(
        output.status.code(),
        Some(1),
        "an unreachable server must exit 1"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing may reach stdout on failure, got {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !output.stderr.is_empty(),
        "the failure must be reported on stderr"
    );
}

// Detects a misconfigured hook command being indistinguishable from a server
// that is down, and detects usage text escaping to stdout and into the session.
#[test]
fn client_exits_two_on_arguments_it_cannot_parse() {
    for arguments in [vec![], vec!["--machine", "alpha"], vec!["--server"]] {
        let child = Command::new(env!("CARGO_BIN_EXE_forgetmenot-hook"))
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the client binary must start");
        let output = child.wait_with_output().expect("the client must finish");

        assert_eq!(
            output.status.code(),
            Some(2),
            "{arguments:?} must exit 2, the code for unusable arguments"
        );
        assert!(
            output.stdout.is_empty(),
            "{arguments:?}: nothing may reach stdout"
        );
    }
}

/// The tail window the client scans for a title at events other than
/// `SessionStart`, mirrored here because these tests are about which side of it
/// a line falls on. Changing the client's window is a change to this constant,
/// and the assertions below say so in their messages.
const CLIENT_TAIL_WINDOW_BYTES: u64 = 256 * 1024;

/// A user line shaped like Claude Code's: `type` is not the first key, so a
/// reader that looks only at the start of a line does not find it.
///
/// Expectation source: a transcript written by Claude Code 2.1.257, read on
/// this machine on 2026-09-13, whose user lines begin
/// `{"parentUuid":…,"isSidechain":…,"promptId":…,"type":"user",…}`.
fn user_line(content: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "parentUuid": serde_json::Value::Null,
        "isSidechain": false,
        "promptId": "prompt-1",
        "type": "user",
        "message": { "role": "user", "content": content },
    })
}

/// A user line Claude Code marks as its own, `isMeta`, rather than the user's.
///
/// Expectation source: the same transcripts, in 17 of which the first user line
/// is an `isMeta` line.
fn meta_user_line(content: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "parentUuid": serde_json::Value::Null,
        "isSidechain": false,
        "type": "user",
        "message": { "role": "user", "content": content },
        "isMeta": true,
    })
}

/// The line `/rename` appends, as Claude Code writes it.
///
/// Expectation source: the same transcript, whose title lines are exactly
/// `{"type":"custom-title","customTitle":"ROC audio #2","sessionId":"…"}`.
fn title_line(title: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "custom-title",
        "customTitle": title,
        "sessionId": "session-1",
    })
}

/// The line Claude Code appends when it has named a session itself.
///
/// Expectation source: a transcript written by Claude Code on this machine on
/// 2026-09-13, whose generated-title lines are
/// `{"type":"ai-title","aiTitle":"Memory system design via MCP","sessionId":"…"}`.
fn ai_title_line(title: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "ai-title",
        "aiTitle": title,
        "sessionId": "session-1",
    })
}

/// The line a compaction appends, which is the last thing that says what a
/// session is about when nothing has named it.
fn summary_line(summary: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "summary",
        "summary": summary,
        "leafUuid": "6f1d2c3e-0000-4000-8000-000000000001",
    })
}

/// Write `lines` as a JSONL transcript, one line each, newline-terminated.
fn write_transcript_lines(path: &Path, lines: &[serde_json::Value]) {
    let mut text = String::new();
    for line in lines {
        text.push_str(&serde_json::to_string(line).expect("a transcript line must serialise"));
        text.push('\n');
    }
    std::fs::write(path, text).expect("the transcript must be writable");
}

/// A hook event that is not `SessionStart`, so the client scans only the tail.
fn stop_event(transcript: &Path) -> serde_json::Value {
    serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": "session-1",
        "transcript_path": transcript.to_str().expect("a utf-8 temp path"),
    })
}

/// The one event per session at which the client scans the whole transcript.
fn session_start_event(transcript: &Path) -> serde_json::Value {
    serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "session-1",
        "transcript_path": transcript.to_str().expect("a utf-8 temp path"),
        "source": "startup",
    })
}

/// Run the client for one event against a one-shot stub and give back the
/// request the stub received.
fn post_one_event(event: &serde_json::Value) -> HookRequest {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("a bound address").port();
    let server_thread = std::thread::spawn(move || serve_one_request(listener, "{}"));

    let payload = format!(
        "{}\n",
        serde_json::to_string(event).expect("the event serialises")
    );
    let output = run_client(
        &format!("http://127.0.0.1:{port}"),
        "alpha",
        payload.as_bytes(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "the client must exit 0; stderr was {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let recorded = server_thread.join().expect("the server thread must finish");
    serde_json::from_slice(&recorded.body).expect("the body must be a HookRequest")
}

// Detects a title read that keeps the first custom-title line instead of the
// last: /rename appends a line and leaves the earlier ones in the file, so a
// renamed session would keep travelling under the name it was given first.
// Also detects a first prompt that never leaves the client, and a user line
// found by its first bytes rather than by a search inside the line.
#[test]
fn client_sends_the_last_custom_title_line_and_the_first_prompt() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(
        &transcript,
        &[
            user_line(serde_json::json!("the very first prompt")),
            title_line("the name it was given first"),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant"}}),
            user_line(serde_json::json!("a later prompt")),
            title_line("the name in force"),
        ],
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.session_title.as_deref(),
        Some("the name in force"),
        "the title must come from the last custom-title line of the transcript"
    );
    assert_eq!(
        request.first_prompt.as_deref(),
        Some("the very first prompt"),
        "the first prompt must come from the first user line, not a later one"
    );
}

// Detects a first-prompt read that only understands a string content: a prompt
// submitted with an attachment is written as content blocks, and a reader that
// gives up on it, or that takes the text of every block including an image's,
// reports the wrong thing for exactly the sessions that pasted something.
#[test]
fn client_joins_the_text_blocks_of_a_first_prompt_written_as_content_blocks() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(
        &transcript,
        &[user_line(serde_json::json!([
            {"type": "text", "text": "the first line of the prompt"},
            {"type": "image", "source": {"type": "base64", "data": "not text"}},
            {"type": "text", "text": "the second line of the prompt"},
        ]))],
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.first_prompt.as_deref(),
        Some("the first line of the prompt\nthe second line of the prompt"),
        "the prompt must be the text blocks joined with a newline, and nothing from the other blocks"
    );
}

// Detects a client that invents a title for a session nobody named: Claude Code
// writes no title line of its own, so anything sent for such a session is made
// up. Also detects a title read that reports the previous session's title, or
// fails, when the file has no custom-title line at all.
#[test]
fn client_sends_no_title_for_a_session_that_was_never_named() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(
        &transcript,
        &[
            user_line(serde_json::json!("the very first prompt")),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant"}}),
        ],
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.session_title, None,
        "a session with no custom-title line has no name to send"
    );
    assert_eq!(
        request.first_prompt.as_deref(),
        Some("the very first prompt"),
        "an unnamed session must still be identified by its first prompt"
    );
}

// Detects a client that takes the last name line of whatever kind: Claude Code
// writes its own title after the user has typed one, so a session the user
// named with /rename would travel under the generated name instead.
// Expectation source: Claude Code's own session picker, which names a session
// by its custom title, then its AI title, then a compaction summary, then its
// first prompt (observed on this machine on 2026-09-13).
#[test]
fn client_sends_the_users_title_rather_than_the_generated_one_written_after_it() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(
        &transcript,
        &[
            user_line(serde_json::json!("the very first prompt")),
            title_line("the name the user typed"),
            ai_title_line("the name Claude Code wrote afterwards"),
            summary_line("what a compaction made of it"),
        ],
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.session_title.as_deref(),
        Some("the name the user typed"),
        "the name the user typed must win over the ones written for them"
    );
}

// Detects a client that knows only the line /rename writes: Claude Code names
// most sessions itself, and every one of them would be listed by its first
// prompt while the harness shows it under a name of its own.
#[test]
fn client_sends_the_generated_title_of_a_session_the_user_never_named() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(
        &transcript,
        &[
            user_line(serde_json::json!("the very first prompt")),
            ai_title_line("the name it was given first"),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant"}}),
            ai_title_line("the generated name in force"),
        ],
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.session_title.as_deref(),
        Some("the generated name in force"),
        "a session Claude Code named must travel under the last generated name"
    );
}

// Detects a session with no title line of either kind being left nameless
// although its transcript says what it is about: a compacted session that was
// never named is exactly the long-running one worth telling apart in the list.
#[test]
fn client_sends_the_compaction_summary_of_a_session_with_no_title_line() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(
        &transcript,
        &[
            user_line(serde_json::json!("the very first prompt")),
            summary_line("Rebuilding the thermocouple rig"),
        ],
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.session_title.as_deref(),
        Some("Rebuilding the thermocouple rig"),
        "a session named by nothing else must be named by its compaction summary"
    );
}

/// A transcript written with custom-title lines at known places. The offsets
/// are counted as the file is built, not read back out of it.
struct TitledTranscript {
    /// Byte offset of the title line written near the start of the file.
    early_title_offset: Option<u64>,
    /// Byte offset of the title line written at the end of the file.
    late_title_offset: Option<u64>,
    /// Length of the whole file in bytes.
    file_length: u64,
}

/// Append `line` and a newline to `text` and report the offset it starts at.
fn push_line(text: &mut String, line: &serde_json::Value) -> u64 {
    let offset = text.len() as u64;
    text.push_str(&serde_json::to_string(line).expect("a transcript line must serialise"));
    text.push('\n');
    offset
}

/// Write a transcript of at least `minimum_bytes`: a first user line, then
/// `early_title` if given, then padding, then `late_title` if given.
fn write_titled_transcript(
    path: &Path,
    first_prompt: &str,
    early_title: Option<&str>,
    late_title: Option<&str>,
    minimum_bytes: u64,
) -> TitledTranscript {
    let padding = "z".repeat(900);
    let mut text = String::new();
    push_line(&mut text, &user_line(serde_json::json!(first_prompt)));
    let early_title_offset = early_title.map(|title| push_line(&mut text, &title_line(title)));
    let mut index = 0u64;
    while (text.len() as u64) < minimum_bytes {
        push_line(
            &mut text,
            &serde_json::json!({"type": "assistant", "index": index, "text": padding}),
        );
        index += 1;
    }
    let late_title_offset = late_title.map(|title| push_line(&mut text, &title_line(title)));
    std::fs::write(path, &text).expect("the transcript must be writable");

    TitledTranscript {
        early_title_offset,
        late_title_offset,
        file_length: text.len() as u64,
    }
}

/// Independent read of the same question: the byte offset and text of every
/// custom-title line, found by walking the file line by line.
fn title_lines_by_reading_everything(path: &Path) -> Vec<(u64, String)> {
    let text = std::fs::read_to_string(path).expect("the transcript must be readable");
    let mut found = Vec::new();
    let mut offset = 0u64;
    for line in text.split_inclusive('\n') {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(without_newline)
            && parsed.get("type").and_then(|value| value.as_str()) == Some("custom-title")
        {
            let title = parsed
                .get("customTitle")
                .and_then(|value| value.as_str())
                .expect("a custom-title line carries a customTitle");
            found.push((offset, title.to_string()));
        }
        offset += line.len() as u64;
    }
    found
}

// Detects a generator that does not put the title lines where it says it does:
// the window tests below decide what they decide from those offsets, so a
// silent bug here would make them pass whatever the client did.
#[test]
fn transcript_generator_places_the_title_lines_where_it_reports() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("session-1.jsonl");
    let generated =
        write_titled_transcript(&path, "the prompt", Some("early"), Some("late"), 512 * 1024);

    assert_eq!(
        title_lines_by_reading_everything(&path),
        vec![
            (
                generated.early_title_offset.expect("an early title"),
                "early".to_string()
            ),
            (
                generated.late_title_offset.expect("a late title"),
                "late".to_string()
            ),
        ],
        "a read of every line must find exactly the title lines the generator reports, where it reports them"
    );
    assert_eq!(
        std::fs::metadata(&path).expect("metadata").len(),
        generated.file_length,
        "the generator must write as many bytes as it reports"
    );
}

// Detects a client that scans the whole transcript at every event: that is the
// cost the owner accepted once per session, not on every tool call. The title
// line here is outside the tail window, so a client that reports it read more
// than the window.
#[test]
fn client_does_not_report_a_title_line_before_the_tail_window_at_a_later_event() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    let generated = write_titled_transcript(
        &transcript,
        "the very first prompt",
        Some("named before the window"),
        None,
        512 * 1024,
    );
    let early_title_offset = generated.early_title_offset.expect("an early title");
    assert!(
        early_title_offset + CLIENT_TAIL_WINDOW_BYTES < generated.file_length,
        "the fixture must put the title line outside the {CLIENT_TAIL_WINDOW_BYTES} byte tail window: it starts at {early_title_offset} of {} bytes",
        generated.file_length
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.session_title, None,
        "an event other than SessionStart must read no further back than the tail window"
    );
}

// Detects a tail scan that loses the title line it does cover: the window
// starts in the middle of a line, and a scan that mis-slices that fragment, or
// stops at the first line instead of the last, reports no name for a session
// that has one. This is the ordinary case, a long session that was renamed.
#[test]
fn client_reports_a_title_line_inside_the_tail_window_at_a_later_event() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    let generated = write_titled_transcript(
        &transcript,
        "the very first prompt",
        None,
        Some("named inside the window"),
        512 * 1024,
    );
    let late_title_offset = generated.late_title_offset.expect("a late title");
    assert!(
        generated.file_length - late_title_offset < CLIENT_TAIL_WINDOW_BYTES,
        "the fixture must put the title line inside the {CLIENT_TAIL_WINDOW_BYTES} byte tail window: it starts at {late_title_offset} of {} bytes",
        generated.file_length
    );
    assert!(
        generated.file_length > CLIENT_TAIL_WINDOW_BYTES,
        "the fixture must be larger than the tail window, or the window covers the file and the test says nothing; it is {} bytes",
        generated.file_length
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.session_title.as_deref(),
        Some("named inside the window"),
        "a title line inside the tail window must reach the server"
    );
}

// Detects a SessionStart that reads no further than the other events do: a
// session named at its first prompt and resumed a week later would arrive
// nameless, and nothing later in the session would fix it. Same fixture as the
// tail test, opposite expectation, which is the whole difference SessionStart
// makes.
#[test]
fn client_reports_a_title_line_before_the_tail_window_at_session_start() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    let generated = write_titled_transcript(
        &transcript,
        "the very first prompt",
        Some("named before the window"),
        None,
        512 * 1024,
    );
    let early_title_offset = generated.early_title_offset.expect("an early title");
    assert!(
        early_title_offset + CLIENT_TAIL_WINDOW_BYTES < generated.file_length,
        "the fixture must put the title line outside the {CLIENT_TAIL_WINDOW_BYTES} byte tail window: it starts at {early_title_offset} of {} bytes",
        generated.file_length
    );

    let request = post_one_event(&session_start_event(&transcript));

    assert_eq!(
        request.session_title.as_deref(),
        Some("named before the window"),
        "SessionStart must scan the whole transcript, however far back the title line is"
    );
    assert_eq!(
        request.first_prompt.as_deref(),
        Some("the very first prompt"),
        "the first prompt must be read from the head of the file whatever its size"
    );
}

/// A JSON transcript line of exactly `length` bytes, newline included.
fn filler_line(length: usize) -> String {
    let overhead = "{\"type\":\"system\",\"text\":\"\"}\n".len();
    assert!(
        length >= overhead,
        "a filler line needs at least {overhead} bytes, asked for {length}"
    );
    let padding = "x".repeat(length - overhead);
    format!("{{\"type\":\"system\",\"text\":\"{padding}\"}}\n")
}

// Detects a whole-file scan that treats each read of the file as a fresh start:
// a title line that spans two reads would be lost, and a session whose title
// happens to sit at that offset would have no name at all. Input: the title
// line starts ten bytes before a power-of-two offset, for every power of two
// from 4 KiB to 1 MiB, so it straddles whatever read size the scan uses.
#[test]
fn whole_file_scan_finds_a_title_line_that_spans_two_reads() {
    let directory = tempfile::tempdir().expect("a temp directory");
    for boundary in [4096usize, 16 * 1024, 64 * 1024, 256 * 1024, 1024 * 1024] {
        let path = directory.path().join(format!("session-{boundary}.jsonl"));
        let head = format!(
            "{}\n",
            serde_json::to_string(&user_line(serde_json::json!("the very first prompt")))
                .expect("serialises")
        );
        let title_starts_at = boundary - 10;
        let mut text = head.clone();
        text.push_str(&filler_line(title_starts_at - head.len()));
        assert_eq!(
            text.len(),
            title_starts_at,
            "the fixture must start the title line ten bytes before {boundary}"
        );
        text.push_str(
            &serde_json::to_string(&title_line("the name in force")).expect("serialises"),
        );
        text.push('\n');
        text.push_str(&filler_line(500));
        std::fs::write(&path, &text).expect("the transcript must be writable");

        assert_eq!(
            forgetmenot_hook::transcript_name::session_title(
                &path,
                forgetmenot_hook::transcript_name::TitleScan::WholeFile
            )
            .as_deref(),
            Some("the name in force"),
            "a title line starting at byte {title_starts_at} must be found"
        );
    }
}

// Detects a prompt sent whole, which would put a pasted file into every hook
// request, and detects a cut made at byte 200 rather than character 200, which
// splits a multi-byte character: that is a panic in the client, on Claude
// Code's critical path, for any prompt whose 200th character is not ASCII.
// The expectation is arithmetic: the first 150 characters are two bytes each,
// so byte 200 falls inside the 101st of them.
#[test]
fn a_long_first_prompt_is_cut_to_two_hundred_characters_not_two_hundred_bytes() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("session-1.jsonl");
    let prompt = format!("{}{}", "é".repeat(150), "a".repeat(150));
    write_transcript_lines(&path, &[user_line(serde_json::json!(prompt))]);
    let expected = format!("{}{}", "é".repeat(150), "a".repeat(50));
    assert_eq!(
        expected.chars().count(),
        200,
        "the expected prompt is the first 200 characters of the written one"
    );

    let read = forgetmenot_hook::transcript_name::first_prompt(&path);

    assert_eq!(
        read.as_deref(),
        Some(expected.as_str()),
        "the prompt must be cut after 200 characters, whatever they weigh in bytes"
    );
}

// Detects a read that fails, or panics, on a transcript that is not there: at
// SessionStart Claude Code may not have written the file yet, and a client that
// dies there takes the hook down with it.
#[test]
fn a_transcript_that_does_not_exist_reports_nothing_rather_than_failing() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let missing = directory.path().join("not-written-yet.jsonl");

    assert_eq!(
        forgetmenot_hook::transcript_name::session_title(
            &missing,
            forgetmenot_hook::transcript_name::TitleScan::WholeFile
        ),
        None,
        "a transcript that does not exist has no title"
    );
    assert_eq!(
        forgetmenot_hook::transcript_name::session_title(
            &missing,
            forgetmenot_hook::transcript_name::TitleScan::TailWindow
        ),
        None,
        "the tail scan of a transcript that does not exist has no title either"
    );
    assert_eq!(
        forgetmenot_hook::transcript_name::first_prompt(&missing),
        None,
        "a transcript that does not exist has no first prompt"
    );
}

// Detects the two new fields being made mandatory on the wire: a client that
// predates them POSTs a body without the keys, and the server must still read
// it rather than reject every hook event that machine sends.
#[test]
fn a_body_without_the_transcript_fields_still_reads_as_a_request() {
    let body = br#"{"machine":"alpha","context_tokens":42,"hook":{"hook_event_name":"Stop"}}"#;

    let request: HookRequest =
        serde_json::from_slice(body).expect("a body from an older client must still parse");

    assert_eq!(
        request.session_title, None,
        "a body that names no title must read as no title"
    );
    assert_eq!(
        request.first_prompt, None,
        "a body that names no first prompt must read as no first prompt"
    );
    assert_eq!(
        request.task, None,
        "a body that names no task must read as no task"
    );
}

// Detects a first prompt taken from the harness's own turns: Claude Code writes
// the caveat it prepends to a slash command, the command itself, the command's
// output and a skill's instructions as user lines, so a client that takes the
// first user line reports "Caveat: The messages below…" as what the session is
// about, for most sessions.
//
// The fixture is the real sequence, read on this machine on 2026-09-13: an
// isMeta <local-command-caveat> block, an isMeta skill block that carries no
// tag, <command-name> and <local-command-stdout> lines that carry no isMeta, a
// user line whose content is only a tool result, and then the user's words. It
// turns red if either half of the skip rule is dropped: the skill block is
// skipped only by isMeta, the command lines only by their opening tag.
#[test]
fn client_skips_the_harness_turns_that_precede_the_first_prompt() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(
        &transcript,
        &[
            meta_user_line(serde_json::json!(
                "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>"
            )),
            meta_user_line(serde_json::json!(
                "Base directory for this skill: /home/arvi/.claude/skills/recover-memory"
            )),
            user_line(serde_json::json!("<command-name>/model</command-name>")),
            user_line(serde_json::json!(
                "<local-command-stdout>Set model to claude-opus-5</local-command-stdout>"
            )),
            user_line(serde_json::json!([
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "a tool's answer"},
            ])),
            user_line(serde_json::json!(
                "please recover the memory of the last session"
            )),
        ],
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.first_prompt.as_deref(),
        Some("please recover the memory of the last session"),
        "the prompt must be the first user line that is the user's own words"
    );
}

/// The subagent the tests below are about, named as Claude Code names one.
const AGENT_ID: &str = "agent-7f3a";

/// What the parent asked that subagent to do, as its metadata file records it.
const TASK: &str = "Survey the rocketry crate and list its public functions";

/// Where Claude Code puts the metadata file of `agent_id` for a session whose
/// transcript is `<directory>/<session>.jsonl`, and where the subagent's own
/// transcript sits beside it.
///
/// Expectation source: the files Claude Code 2.1.270 wrote on this machine on
/// 2026-09-13, `<dir>/<session_id>/subagents/agent-<agent_id>.meta.json` beside
/// `<dir>/<session_id>/subagents/agent-<agent_id>.jsonl`.
fn subagents_directory(directory: &Path, session: &str) -> std::path::PathBuf {
    directory.join(session).join("subagents")
}

/// Write the metadata file Claude Code writes for one subagent.
///
/// Expectation source: the same files, whose content is
/// `{"agentType":"general-purpose","description":"Name check: say done",
/// "toolUseId":"…","spawnDepth":1,"requestShape":"foreground",
/// "requestNonInteractive":true,"model":"haiku"}`. Every field is written, not
/// only the one under test, so a client that sends the agent type or the model
/// as the task is caught.
fn write_subagent_meta(directory: &Path, agent_id: &str, description: &str) {
    std::fs::create_dir_all(directory).expect("the subagents directory must be creatable");
    let meta = serde_json::json!({
        "agentType": "general-purpose",
        "description": description,
        "toolUseId": "toolu_01DDDDDDDDDDDDDDDDDDDDDD",
        "spawnDepth": 1,
        "requestShape": "foreground",
        "requestNonInteractive": true,
        "model": "haiku",
    });
    std::fs::write(
        directory.join(format!("agent-{agent_id}.meta.json")),
        serde_json::to_string(&meta).expect("the metadata serialises"),
    )
    .expect("the metadata file must be writable");
}

/// An event from inside a subagent, which is the only kind that carries an
/// `agent_id`. `transcript` is whichever transcript the event reports.
fn event_in_subagent(transcript: &Path, agent_id: &str) -> serde_json::Value {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "session-1",
        "transcript_path": transcript.to_str().expect("a utf-8 temp path"),
        "agent_id": agent_id,
        "tool_name": "Grep",
        "tool_use_id": "toolu_1",
        "tool_input": { "pattern": "pub fn" },
    })
}

// Detects a task that never leaves the client: no hook event says what a
// subagent was asked to do, so a server that is not sent the task has nothing
// to name a subagent by. Also detects a client that sends another field of the
// metadata file, such as the agent type, as the task.
#[test]
fn client_sends_the_task_from_the_metadata_file_of_the_subagent_the_event_comes_from() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(&transcript, &[user_line(serde_json::json!("the prompt"))]);
    write_subagent_meta(
        &subagents_directory(directory.path(), "session-1"),
        AGENT_ID,
        TASK,
    );

    let request = post_one_event(&event_in_subagent(&transcript, AGENT_ID));

    assert_eq!(
        request.task.as_deref(),
        Some(TASK),
        "the task must be the description of the metadata file of the event's own subagent"
    );
}

// Detects a client that reads a metadata file for an event that is not a
// subagent's: a session would be named for whatever subagent it last spawned,
// and every event of every session would pay for the read.
#[test]
fn client_sends_no_task_for_an_event_outside_a_subagent() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(&transcript, &[user_line(serde_json::json!("the prompt"))]);
    write_subagent_meta(
        &subagents_directory(directory.path(), "session-1"),
        AGENT_ID,
        TASK,
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.task, None,
        "an event with no agent id is not a subagent's, whatever metadata files exist beside it"
    );
}

// Detects a client that fails, or invents a task, when the metadata file is not
// there: Claude Code may not have written it yet, or may stop writing it, and
// either would otherwise take the hook down on every event of every subagent.
#[test]
fn client_sends_no_task_and_still_succeeds_when_the_subagent_has_no_metadata_file() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(&transcript, &[user_line(serde_json::json!("the prompt"))]);

    // post_one_event fails the test on any exit code but 0.
    let request = post_one_event(&event_in_subagent(&transcript, AGENT_ID));

    assert_eq!(
        request.task, None,
        "a subagent with no metadata file has no task to send"
    );
}

// Detects a client that only understands one of the two transcripts an event
// inside a subagent may report: which one Claude Code sends is not something
// this client is told, so a derivation that works from the session's transcript
// alone loses every task the day the subagent's own is sent instead.
#[test]
fn client_finds_the_metadata_file_beside_a_subagents_own_transcript() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let subagents = subagents_directory(directory.path(), "session-1");
    write_subagent_meta(&subagents, AGENT_ID, TASK);
    let own_transcript = subagents.join(format!("agent-{AGENT_ID}.jsonl"));
    write_transcript_lines(
        &own_transcript,
        &[user_line(serde_json::json!("the whole task prompt"))],
    );

    let request = post_one_event(&event_in_subagent(&own_transcript, AGENT_ID));

    assert_eq!(
        request.task.as_deref(),
        Some(TASK),
        "the metadata file beside the subagent's own transcript must be the one read"
    );
}

// Detects a task sent whole, which would put a task with a pasted file in it
// into every event of a subagent, and detects a cut made at byte 200 rather
// than character 200: that splits a multi-byte character, which is a panic in
// the client, on Claude Code's critical path. The expectation is arithmetic:
// the first 150 characters are two bytes each, so byte 200 falls inside the
// 101st of them.
#[test]
fn a_long_task_is_cut_to_two_hundred_characters_not_two_hundred_bytes() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    write_transcript_lines(&transcript, &[user_line(serde_json::json!("the prompt"))]);
    let description = format!("{}{}", "é".repeat(150), "a".repeat(150));
    write_subagent_meta(
        &subagents_directory(directory.path(), "session-1"),
        AGENT_ID,
        &description,
    );
    let expected = format!("{}{}", "é".repeat(150), "a".repeat(50));
    assert_eq!(
        expected.chars().count(),
        200,
        "the expected task is the first 200 characters of the written one"
    );

    let request = post_one_event(&event_in_subagent(&transcript, AGENT_ID));

    assert_eq!(
        request.task.as_deref(),
        Some(expected.as_str()),
        "the task must be cut after 200 characters, whatever they weigh in bytes"
    );
}

// Detects a first-prompt read that gives up on a line longer than one read of
// the file: a session that opened with a pasted file has a first user line of
// hundreds of kilobytes, and it is exactly the sessions that pasted something
// that are worth identifying. Also detects a read that returns the line's tail
// or its middle rather than its opening.
#[test]
fn client_sends_the_opening_of_a_first_prompt_larger_than_one_read() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let transcript = directory.path().join("session-1.jsonl");
    let opening = "abcdefghij".repeat(30);
    let prompt = format!("{opening}{}", "p".repeat(300 * 1024));
    write_transcript_lines(&transcript, &[user_line(serde_json::json!(prompt))]);

    // An independent read of what was written, so the size this test claims is
    // the size on disk and not the generator's word for it.
    let written = std::fs::read_to_string(&transcript).expect("the transcript must be readable");
    let first_line_bytes = written.lines().next().expect("a first line").len();
    assert!(
        first_line_bytes >= 300 * 1024,
        "the fixture's first user line must be at least 300 KiB, it is {first_line_bytes} bytes"
    );

    let request = post_one_event(&stop_event(&transcript));

    assert_eq!(
        request.first_prompt.as_deref(),
        Some(&opening[..200]),
        "the prompt must be the first 200 characters of the line, however long the line is"
    );
}
