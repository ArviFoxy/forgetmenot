//! The forgetmenot hook client.
//!
//! Claude Code runs this binary for every hook event, with the event JSON on
//! stdin, and injects whatever it writes to stdout. The client adds what the
//! server cannot know by itself: the machine name, what only the session's
//! transcript says, the size of the session's context, what the session is
//! called and its first prompt, and, inside a subagent, the task the subagent
//! was given, which only its metadata file says. It POSTs the result to the
//! server and
//! copies the server's answer to stdout unchanged. It holds no state, so any
//! number of them may run at once.
//!
//! Claude Code shows the model whatever lands on stdout, so a failure must
//! stay off it: every failure path writes one line to stderr, nothing to
//! stdout, and exits 1.

pub mod subagent_meta;
pub mod transcript_name;
pub mod transcript_tail;

use std::io::{Read, Write};
use std::time::Duration;

/// Usage line printed when the arguments do not parse.
pub const USAGE: &str = "usage: forgetmenot-hook --server URL --machine NAME";

/// Exit code for arguments that do not parse, distinct from the exit code for
/// a failed request so a misconfigured hook can be told apart from a server
/// that is down.
pub const EXIT_USAGE: i32 = 2;

/// A hook event must not hold Claude Code up, so the client gives up early: a
/// server on the LAN either answers or is not there.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

/// The command line the client needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arguments {
    /// Base URL of the forgetmenot server, without the `/hook` path.
    pub server: String,
    /// Name this machine is known by in the store.
    pub machine: String,
}

/// Parse `--server URL --machine NAME` in either order. The error names the
/// problem for the stderr line; it is not meant to be matched on.
pub fn parse_arguments(arguments: &[String]) -> Result<Arguments, String> {
    let mut server = None;
    let mut machine = None;
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        let target = match argument.as_str() {
            "--server" => &mut server,
            "--machine" => &mut machine,
            other => return Err(format!("unknown argument {other}")),
        };
        match remaining.next() {
            Some(value) => *target = Some(value.clone()),
            None => return Err(format!("{argument} needs a value")),
        }
    }
    match (server, machine) {
        (Some(server), Some(machine)) => Ok(Arguments { server, machine }),
        (None, _) => Err("--server is required".to_string()),
        (_, None) => Err("--machine is required".to_string()),
    }
}

/// Run one hook event end to end and return the process exit code.
///
/// `arguments` excludes the program name.
pub fn run(
    arguments: &[String],
    stdin: &mut impl Read,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> i32 {
    let parsed = match parse_arguments(arguments) {
        Ok(parsed) => parsed,
        Err(problem) => {
            let _ = writeln!(stderr, "forgetmenot-hook: {problem}");
            let _ = writeln!(stderr, "{USAGE}");
            return EXIT_USAGE;
        }
    };

    let mut hook_json = Vec::new();
    if let Err(error) = stdin.read_to_end(&mut hook_json) {
        return fail(stderr, format_args!("reading stdin failed: {error}"));
    }
    let hook_json = trim_ascii_whitespace(&hook_json);

    // The event is parsed only to find the transcript path; the bytes are
    // forwarded as they arrived so the server sees what Claude Code wrote,
    // including fields no version of this client knows about.
    let event: serde_json::Value = match serde_json::from_slice(hook_json) {
        Ok(event) => event,
        Err(error) => {
            return fail(stderr, format_args!("stdin is not valid JSON: {error}"));
        }
    };
    let transcript_path = event
        .get("transcript_path")
        .and_then(serde_json::Value::as_str);
    let context_tokens = transcript_path.and_then(transcript_tail::last_assistant_context_tokens);
    // A session is named once and the line saying so is re-emitted as the
    // transcript grows, so the whole file is read only at the one event per
    // session that can afford it; every other event reads the tail, and a
    // rename mid-session is reported at the next event after it.
    let title_scan = if event
        .get("hook_event_name")
        .and_then(serde_json::Value::as_str)
        == Some("SessionStart")
    {
        transcript_name::TitleScan::WholeFile
    } else {
        transcript_name::TitleScan::TailWindow
    };
    let session_title =
        transcript_path.and_then(|path| transcript_name::session_title(path, title_scan));
    let first_prompt = transcript_path.and_then(transcript_name::first_prompt);
    // Only an event from inside a subagent has a task, and the event names the
    // subagent whose metadata file carries it.
    let agent_id = event
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .filter(|agent_id| !agent_id.is_empty());
    let task = match (transcript_path, agent_id) {
        (Some(path), Some(agent_id)) => subagent_meta::task(path, agent_id),
        _ => None,
    };

    let body = request_body(
        &parsed.machine,
        context_tokens,
        session_title.as_deref(),
        first_prompt.as_deref(),
        task.as_deref(),
        hook_json,
    );
    let url = format!("{}/hook", parsed.server.trim_end_matches('/'));

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(TOTAL_TIMEOUT))
        // Statuses are handled below so the stderr line can name the status.
        .http_status_as_error(false)
        .build()
        .into();

    let mut response = match agent
        .post(&url)
        .header("content-type", "application/json")
        .send(&body)
    {
        Ok(response) => response,
        Err(error) => return fail(stderr, format_args!("POST {url} failed: {error}")),
    };
    let status = response.status().as_u16();
    if status != 200 {
        return fail(stderr, format_args!("POST {url} answered HTTP {status}"));
    }
    let answer = match response.body_mut().read_to_vec() {
        Ok(answer) => answer,
        Err(error) => {
            return fail(stderr, format_args!("reading the answer failed: {error}"));
        }
    };

    if let Err(error) = stdout.write_all(&answer).and_then(|()| stdout.flush()) {
        return fail(stderr, format_args!("writing stdout failed: {error}"));
    }
    0
}

/// Report a failure on stderr in one line and give the exit code for it.
fn fail(stderr: &mut impl Write, what_failed: std::fmt::Arguments) -> i32 {
    let _ = writeln!(stderr, "forgetmenot-hook: {what_failed}");
    1
}

/// Build the `POST /hook` body, splicing the hook event in as received.
///
/// The result is a `forgetmenot_types::hook::HookRequest` on the wire; it is
/// assembled by hand rather than serialised from that type because re-encoding
/// the event through `serde_json::Value` would reorder its keys and drop the
/// distinction between the bytes Claude Code sent and this client's idea of
/// them.
fn request_body(
    machine: &str,
    context_tokens: Option<u64>,
    session_title: Option<&str>,
    first_prompt: Option<&str>,
    task: Option<&str>,
    hook_json: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(hook_json.len() + 512);
    body.extend_from_slice(b"{\"machine\":");
    serde_json::to_writer(&mut body, machine).expect("a string always serialises");
    body.extend_from_slice(b",\"context_tokens\":");
    match context_tokens {
        Some(tokens) => body.extend_from_slice(tokens.to_string().as_bytes()),
        None => body.extend_from_slice(b"null"),
    }
    body.extend_from_slice(b",\"session_title\":");
    write_optional_string(&mut body, session_title);
    body.extend_from_slice(b",\"first_prompt\":");
    write_optional_string(&mut body, first_prompt);
    body.extend_from_slice(b",\"task\":");
    write_optional_string(&mut body, task);
    body.extend_from_slice(b",\"hook\":");
    body.extend_from_slice(hook_json);
    body.extend_from_slice(b"}");
    body
}

/// Write a JSON string, or `null` when there is nothing to write. The value
/// comes from a transcript the client did not write, so the quoting and
/// escaping are left to `serde_json` rather than done here.
fn write_optional_string(body: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(text) => serde_json::to_writer(body, text).expect("a string always serialises"),
        None => body.extend_from_slice(b"null"),
    }
}

/// Claude Code ends the payload with a newline; JSON does not care, but the
/// spliced body is neater without it.
fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |last| last + 1);
    &bytes[start..end]
}
