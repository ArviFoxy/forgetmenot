//! The Claude Code running on one machine, and the two ways its hook events
//! reach the server.

use std::cell::Cell;

use serde_json::{Value, json};

use crate::common;
use crate::scenario::session::Session;
use crate::scenario::world::World;

/// Claude Code on one machine. Every session it starts belongs to that machine,
/// which is what the `--machine` flag of the hook client says.
pub struct Claude<'w> {
    world: &'w World,
    machine: String,
    /// Numbers the sessions this machine has started, so two sessions of one
    /// machine never share an id.
    started: Cell<usize>,
}

impl<'w> Claude<'w> {
    pub(crate) fn new(world: &'w World, machine: &str) -> Self {
        Self {
            world,
            machine: machine.to_string(),
            started: Cell::new(0),
        }
    }

    pub fn machine(&self) -> &str {
        &self.machine
    }

    /// A session nobody has seen before.
    pub fn session(&self) -> Session<'w> {
        let number = self.started.get() + 1;
        self.started.set(number);
        self.session_named(&format!("session-{number}"))
    }

    /// A session with the id given, for the scenarios about a session id that
    /// is not this machine's to choose: the same id on two machines, or a
    /// session the store already holds memories for.
    pub fn session_named(&self, id: &str) -> Session<'w> {
        Session::start(self.world, &self.machine, id)
    }
}

// ---------------------------------------------------------------------------
// Transports
// ---------------------------------------------------------------------------

/// One hook event on its way to the server, with everything the client sends
/// beside it.
pub struct HookPost<'a> {
    pub base_url: &'a str,
    pub machine: &'a str,
    /// The context size the transcript's last assistant line reports.
    pub context_tokens: Option<u64>,
    /// The name the session carries, from the transcript's title lines.
    pub session_title: Option<&'a str>,
    /// The user's first words in the session.
    pub first_prompt: Option<&'a str>,
    /// What the subagent this event comes from was asked to do.
    pub task: Option<&'a str>,
    /// The event as Claude Code writes it, `transcript_path` included.
    pub event: &'a Value,
}

/// How a hook event reaches the server.
pub trait Transport: Send + Sync {
    /// Send one event and report the answer JSON Claude Code would read.
    fn send(&self, post: &HookPost<'_>) -> Value;
}

/// POST the body the client builds straight to `/hook`.
///
/// The transcript is written all the same, so a scenario reads the same way
/// under either transport; this one just does not read it back.
pub struct Http;

/// Run the compiled `forgetmenot-hook` binary with the event on its stdin, as
/// Claude Code does, and parse what it writes to stdout.
///
/// Everything beside the event is left out of the call: the client reads the
/// context size, the title, the first prompt and the task from the transcript
/// and the metadata file itself, which is the point of running it.
pub struct Client;

/// The transport a world posts straight to `/hook` with.
pub static HTTP: Http = Http;

/// The transport a world runs the real client with.
pub static CLIENT: Client = Client;

impl Transport for Http {
    fn send(&self, post: &HookPost<'_>) -> Value {
        let body = json!({
            "machine": post.machine,
            "context_tokens": post.context_tokens,
            "session_title": post.session_title,
            "first_prompt": post.first_prompt,
            "task": post.task,
            "hook": post.event,
        });
        let agent = common::test_agent();
        let mut response = agent
            .post(format!("{}/hook", post.base_url))
            .header("content-type", "application/json")
            .send(serde_json::to_string(&body).expect("the body serialises"))
            .expect("the request reaches the server");
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .expect("the answer is text");
        assert_eq!(
            status, 200,
            "the server must answer every event it is sent, got {text}"
        );
        serde_json::from_str(&text).unwrap_or_else(|error| {
            panic!("the answer is not JSON: {error}; the server said {text:?}")
        })
    }
}

impl Transport for Client {
    fn send(&self, post: &HookPost<'_>) -> Value {
        let payload = format!(
            "{}\n",
            serde_json::to_string(post.event).expect("the event serialises")
        );
        let run = common::run_hook_client(post.base_url, post.machine, payload.as_bytes());
        assert_eq!(
            run.code,
            Some(0),
            "the client must exit cleanly; it said {:?} on stderr",
            run.stderr_text()
        );
        run.single_json_object()
            .unwrap_or_else(|problem| panic!("the client wrote {problem}"))
    }
}
