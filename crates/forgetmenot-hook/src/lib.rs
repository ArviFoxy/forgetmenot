//! The forgetmenot hook client.
//!
//! A harness, Claude Code or Antigravity, runs this binary for every hook
//! event, with the event JSON on stdin, and acts on whatever it writes to
//! stdout. The client adds what the server cannot know by itself: the machine
//! name, which harness the event comes from, and what only the session's
//! transcript says. For both harnesses that is the size of the session's
//! context. For Claude Code it is also what the session is called and its first
//! prompt, and, inside a subagent, the task the subagent was given and the
//! agent that spawned it, which only its metadata file says. For Antigravity it
//! is the model's last message at the end of a turn, which the event does not
//! carry. It POSTs the result to the server and copies the server's answer to
//! stdout unchanged. It holds no state, so any number of them may run at once.
//!
//! A failure is reported the way the harness can take it, and always in one
//! line on stderr. Claude Code shows the model whatever lands on stdout and
//! treats a failed hook as a hook that said nothing, so there a failure writes
//! nothing to stdout and exits 1. Antigravity requires an answer to a tool call
//! and does not say what it does without one, so there a failure writes the
//! answer that lets the event run its course and exits 0: a memory server that
//! is down must not stop the agent's tools.

pub mod antigravity_transcript;
pub mod subagent_meta;
pub mod transcript_name;
pub mod transcript_tail;

use std::io::{Read, Write};
use std::time::Duration;

use forgetmenot_types::antigravity::AntigravityReply;
use forgetmenot_types::hook::Harness;

/// Usage line printed when the arguments do not parse.
pub const USAGE: &str =
    "usage: forgetmenot-hook --server URL --machine NAME [--harness claude-code|antigravity]";

/// Exit code for arguments that do not parse, distinct from the exit code for
/// a failed request so a misconfigured hook can be told apart from a server
/// that is down.
pub const EXIT_USAGE: i32 = 2;

/// A hook event must not hold the harness up, so the client gives up early: a
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
    /// The harness that runs this client. Claude Code when the flag is absent,
    /// which is how every hook configured before the flag existed runs it.
    pub harness: Harness,
}

/// Parse `--server URL --machine NAME [--harness NAME]` in any order. The error
/// names the problem for the stderr line; it is not meant to be matched on.
pub fn parse_arguments(arguments: &[String]) -> Result<Arguments, String> {
    let mut server = None;
    let mut machine = None;
    let mut harness = None;
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        let target = match argument.as_str() {
            "--server" => &mut server,
            "--machine" => &mut machine,
            "--harness" => &mut harness,
            other => return Err(format!("unknown argument {other}")),
        };
        match remaining.next() {
            Some(value) => *target = Some(value.clone()),
            None => return Err(format!("{argument} needs a value")),
        }
    }
    let harness = match harness {
        Some(name) => Harness::parse(&name)
            .ok_or_else(|| format!("--harness {name} is neither claude-code nor antigravity"))?,
        None => Harness::default(),
    };
    match (server, machine) {
        (Some(server), Some(machine)) => Ok(Arguments {
            server,
            machine,
            harness,
        }),
        (None, _) => Err("--server is required".to_string()),
        (_, None) => Err("--machine is required".to_string()),
    }
}

/// What the client read for one event from the files the harness keeps beside
/// the session, to send with the event. Whatever a harness has no such file
/// for stays absent.
#[derive(Default)]
struct Reads {
    context_tokens: Option<u64>,
    session_title: Option<String>,
    first_prompt: Option<String>,
    subagent: subagent_meta::SubagentMeta,
    last_assistant_message: Option<String>,
}

/// What Claude Code's files say about the session `event` comes from.
fn claude_code_reads(event: &serde_json::Value) -> Reads {
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
    // Only an event from inside a subagent has a task and an agent that spawned
    // it, and the event names the subagent whose metadata file carries both.
    let agent_id = event
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .filter(|agent_id| !agent_id.is_empty());
    let subagent = match (transcript_path, agent_id) {
        (Some(path), Some(agent_id)) => subagent_meta::read(path, agent_id),
        _ => subagent_meta::SubagentMeta::default(),
    };
    Reads {
        context_tokens,
        session_title,
        first_prompt,
        subagent,
        last_assistant_message: None,
    }
}

/// What Antigravity's transcript says about the conversation `event` comes
/// from. The conversation's first prompt is in its events, and Antigravity
/// keeps no name for a conversation, so neither is read here.
fn antigravity_reads(event: &serde_json::Value) -> Reads {
    let transcript_path = event
        .get("transcriptPath")
        .and_then(serde_json::Value::as_str);
    // The model's last message is what the turn ended on, so it is read where
    // the turn ends and nowhere else.
    let last_assistant_message = match antigravity_event_name(event) {
        Some("Stop") => transcript_path.and_then(antigravity_transcript::last_model_message),
        _ => None,
    };
    Reads {
        context_tokens: transcript_path.and_then(antigravity_transcript::last_model_context_tokens),
        last_assistant_message,
        ..Reads::default()
    }
}

/// The name of an Antigravity event, which says what its answer has to be.
fn antigravity_event_name(event: &serde_json::Value) -> Option<&str> {
    event
        .get("hookEventName")
        .and_then(serde_json::Value::as_str)
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

    // Known once the event has been read; a failure after that is answered for
    // this event, and a failure before it for an event of no name.
    let mut event_name = None;
    match exchange(&parsed, stdin, &mut event_name) {
        Ok(answer) => match stdout.write_all(&answer).and_then(|()| stdout.flush()) {
            Ok(()) => 0,
            Err(error) => {
                let _ = writeln!(stderr, "forgetmenot-hook: writing stdout failed: {error}");
                1
            }
        },
        Err(problem) => {
            let _ = writeln!(stderr, "forgetmenot-hook: {problem}");
            match parsed.harness {
                Harness::ClaudeCode => 1,
                Harness::Antigravity => {
                    let reply = AntigravityReply::neutral(event_name.as_deref().unwrap_or(""));
                    let written = serde_json::to_writer(&mut *stdout, &reply)
                        .map_err(std::io::Error::from)
                        .and_then(|()| stdout.flush());
                    match written {
                        Ok(()) => 0,
                        Err(_) => 1,
                    }
                }
            }
        }
    }
}

/// Read one event from `stdin`, send it to the server and give the server's
/// answer, or say in one line what failed. `event_name` is set as soon as the
/// event has been read, for the harness whose failures are answered by event.
fn exchange(
    parsed: &Arguments,
    stdin: &mut impl Read,
    event_name: &mut Option<String>,
) -> Result<Vec<u8>, String> {
    let mut hook_json = Vec::new();
    stdin
        .read_to_end(&mut hook_json)
        .map_err(|error| format!("reading stdin failed: {error}"))?;
    let hook_json = trim_ascii_whitespace(&hook_json);

    // The event is parsed only to find the transcript and the event's name; the
    // bytes are forwarded as they arrived so the server sees what the harness
    // wrote, including fields no version of this client knows about.
    let event: serde_json::Value = serde_json::from_slice(hook_json)
        .map_err(|error| format!("stdin is not valid JSON: {error}"))?;
    let reads = match parsed.harness {
        Harness::ClaudeCode => claude_code_reads(&event),
        Harness::Antigravity => {
            *event_name = antigravity_event_name(&event).map(str::to_string);
            antigravity_reads(&event)
        }
    };

    let body = request_body(&parsed.machine, parsed.harness, &reads, hook_json);
    let url = format!("{}/hook", parsed.server.trim_end_matches('/'));

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(TOTAL_TIMEOUT))
        // Statuses are handled below so the stderr line can name the status.
        .http_status_as_error(false)
        .build()
        .into();

    let mut response = agent
        .post(&url)
        .header("content-type", "application/json")
        .send(&body)
        .map_err(|error| format!("POST {url} failed: {error}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("POST {url} answered HTTP {status}"));
    }
    response
        .body_mut()
        .read_to_vec()
        .map_err(|error| format!("reading the answer failed: {error}"))
}

/// Build the `POST /hook` body, splicing the hook event in as received.
///
/// The result is a `forgetmenot_types::hook::HookRequest` on the wire; it is
/// assembled by hand rather than serialised from that type because re-encoding
/// the event through `serde_json::Value` would reorder its keys and drop the
/// distinction between the bytes the harness sent and this client's idea of
/// them.
fn request_body(machine: &str, harness: Harness, reads: &Reads, hook_json: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(hook_json.len() + 512);
    body.extend_from_slice(b"{\"machine\":");
    serde_json::to_writer(&mut body, machine).expect("a string always serialises");
    body.extend_from_slice(b",\"harness\":");
    serde_json::to_writer(&mut body, harness.as_str()).expect("a string always serialises");
    body.extend_from_slice(b",\"context_tokens\":");
    match reads.context_tokens {
        Some(tokens) => body.extend_from_slice(tokens.to_string().as_bytes()),
        None => body.extend_from_slice(b"null"),
    }
    body.extend_from_slice(b",\"session_title\":");
    write_optional_string(&mut body, reads.session_title.as_deref());
    body.extend_from_slice(b",\"first_prompt\":");
    write_optional_string(&mut body, reads.first_prompt.as_deref());
    body.extend_from_slice(b",\"task\":");
    write_optional_string(&mut body, reads.subagent.task.as_deref());
    body.extend_from_slice(b",\"parent_agent_id\":");
    write_optional_string(&mut body, reads.subagent.parent_agent_id.as_deref());
    body.extend_from_slice(b",\"last_assistant_message\":");
    write_optional_string(&mut body, reads.last_assistant_message.as_deref());
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

/// A harness ends the payload with a newline; JSON does not care, but the
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
