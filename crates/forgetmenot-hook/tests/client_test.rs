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
