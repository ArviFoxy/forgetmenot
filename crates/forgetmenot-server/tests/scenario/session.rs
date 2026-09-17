//! One Claude Code session, and one subagent inside it.
//!
//! Each action sends the events Claude Code sends for it, in Claude Code's
//! order, and then does what Claude Code does with the answer: a denied
//! `PreToolUse` is followed by no `PostToolUse`, and every answer's text is put
//! into the session's context under the rule that saves a long answer to a file
//! and shows the model a preview of it.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::common;
use crate::scenario::answer::{Answer, seen_in};
use crate::scenario::mcp::Mcp;
use crate::scenario::payloads::{self, Agent, Common};
use crate::scenario::world::World;

/// The context a session starts with: the system prompt and the harness's own
/// opening turns, before the scenario does anything.
pub const STARTING_TOKENS: u64 = 10_000;

/// What one event costs the context beyond the text it carries: the model's own
/// turn around it. A model of context growth, not a measurement, so a scenario
/// that depends on an exact size sets one with [`Session::at_tokens`].
pub const TOKENS_PER_EVENT: u64 = 100;

/// Characters of injected text per token.
pub const CHARACTERS_PER_TOKEN: u64 = 4;

/// The summary line a compaction leaves in the transcript, which is the name a
/// compacted session carries when nobody renamed it.
pub const COMPACTION_SUMMARY: &str = "the conversation so far";

/// One session, or one subagent inside one: a subagent takes the same actions,
/// and its events carry its agent id.
pub struct Session<'w> {
    world: &'w World,
    machine: String,
    session_id: String,
    /// The subagent this is, or `None` for a session's own context.
    agent: Option<Agent>,
    transcript: PathBuf,
    state: RefCell<State>,
}

/// A subagent takes exactly the actions a session does, so it is the same type.
pub type Subagent<'w> = Session<'w>;

/// What the session has done so far.
struct State {
    cwd: Option<String>,
    tokens: u64,
    /// The size the transcript's last assistant line reports, so a line is
    /// appended only when the size has moved.
    reported_tokens: Option<u64>,
    prompt_id: String,
    prompts: usize,
    agents: usize,
    tools: usize,
    /// The name the user gave the session with `/rename`.
    custom_title: Option<String>,
    /// The name a compaction left behind.
    summary: Option<String>,
    first_prompt: Option<String>,
    task: Option<String>,
    /// Everything Claude Code injected into this context, in order, as the
    /// model saw it.
    injected: Vec<String>,
    /// The answers Claude Code saved to a file whole.
    persisted: Vec<PathBuf>,
}

/// The two answers a compaction produces.
pub struct Compaction<'w> {
    /// The session start of the conversation the compaction rebuilt, which is
    /// where everything due arrives again.
    pub start: Answer<'w>,
    /// The compaction itself, which Claude Code takes no context from.
    pub post: Answer<'w>,
}

/// One tool call, from the `PreToolUse` that may stop it to the `PostToolUse`
/// that reports what it returned.
pub struct ToolCall<'s, 'w> {
    session: &'s Session<'w>,
    name: String,
    input: Value,
    tool_use_id: String,
    answer: Answer<'w>,
}

/// The input of a `Bash` call.
pub fn bash(command: &str) -> Value {
    json!({ "command": command })
}

/// The input of a `Read` call.
pub fn read(path: &str) -> Value {
    json!({ "file_path": path })
}

impl<'w> Session<'w> {
    pub(crate) fn start(world: &'w World, machine: &str, session_id: &str) -> Self {
        let transcript = world
            .transcripts()
            .join(machine)
            .join(format!("{session_id}.jsonl"));
        create(&transcript);
        Self {
            world,
            machine: machine.to_string(),
            session_id: session_id.to_string(),
            agent: None,
            transcript,
            state: RefCell::new(State::new()),
        }
    }

    // -- facts ------------------------------------------------------------

    pub fn machine(&self) -> &str {
        &self.machine
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The session key the MCP tools take: `machine/session-id`, and
    /// `machine/session-id/agent-id` inside a subagent.
    pub fn key(&self) -> String {
        match &self.agent {
            Some(agent) => format!("{}/{}/{}", self.machine, self.session_id, agent.agent_id),
            None => format!("{}/{}", self.machine, self.session_id),
        }
    }

    /// The directory the session reports, once it has started somewhere.
    pub fn cwd(&self) -> Option<String> {
        self.state.borrow().cwd.clone()
    }

    /// The transcript Claude Code writes for this session, which is the file
    /// the client reads.
    pub fn transcript(&self) -> &Path {
        &self.transcript
    }

    /// The context size the next event will report.
    pub fn tokens(&self) -> u64 {
        self.state.borrow().tokens
    }

    /// Set the context size, for the scenarios that are about a threshold and
    /// cannot leave the size to the model of growth.
    pub fn at_tokens(&self, tokens: u64) -> &Self {
        assert!(
            tokens >= 2,
            "a transcript reports the size over three counters, so it needs at least 2 tokens"
        );
        self.state.borrow_mut().tokens = tokens;
        self
    }

    pub(crate) fn world(&self) -> &'w World {
        self.world
    }

    // -- actions ----------------------------------------------------------

    /// Start the session in a directory: `SessionStart` with source `startup`.
    pub fn start_in(&self, cwd: &str) -> Answer<'w> {
        self.state.borrow_mut().cwd = Some(cwd.to_string());
        let event = payloads::session_start(&self.common(), "startup");
        self.send(event)
    }

    /// Resume a session that was closed: `SessionStart` with source `resume`.
    pub fn resume(&self) -> Answer<'w> {
        let event = payloads::session_start(&self.common(), "resume");
        self.send(event)
    }

    /// The user submits a prompt.
    pub fn prompt(&self, text: &str) -> Answer<'w> {
        {
            let mut state = self.state.borrow_mut();
            state.prompts += 1;
            state.prompt_id = format!("prompt-{:04}", state.prompts);
            if state.first_prompt.is_none() {
                state.first_prompt = Some(cut(text));
            }
        }
        self.append_line(&json!({
            "type": "user",
            "message": { "role": "user", "content": text },
        }));
        let event = payloads::user_prompt_submit(&self.common(), text);
        self.send(event)
    }

    /// The model asks to run a tool: `PreToolUse`, whose answer may stop it.
    pub fn tool(&self, name: &str, input: Value) -> ToolCall<'_, 'w> {
        let tool_use_id = {
            let mut state = self.state.borrow_mut();
            state.tools += 1;
            format!("toolu_{:024}", state.tools)
        };
        let event = payloads::pre_tool_use(&self.common(), name, &input, &tool_use_id);
        let answer = self.send(event);
        ToolCall {
            session: self,
            name: name.to_string(),
            input,
            tool_use_id,
            answer,
        }
    }

    /// The assistant finishes its turn: `Stop`, with what it said last.
    pub fn says(&self, text: &str) -> Answer<'w> {
        self.append_assistant(text);
        let event = payloads::stop(&self.common(), text);
        self.send(event)
    }

    /// The session's shell moves: `CwdChanged`, carrying both directories.
    pub fn cd(&self, directory: &str) -> Answer<'w> {
        let previous = self.state.borrow().cwd.clone();
        let event = payloads::cwd_changed(&self.common(), previous.as_deref(), directory);
        self.state.borrow_mut().cwd = Some(directory.to_string());
        self.send(event)
    }

    /// The user compacts the conversation: the `SessionStart` of the rebuilt
    /// conversation, then the `PostCompact` reporting what was removed.
    pub fn compact(&self) -> Compaction<'w> {
        self.compaction("manual")
    }

    /// The same, when Claude Code compacted by itself.
    pub fn auto_compact(&self) -> Compaction<'w> {
        self.compaction("auto")
    }

    /// The user names the session with `/rename`.
    ///
    /// No hook event carries a name: the client reads it from the transcript's
    /// `custom-title` line, so the line is what this writes, and the name is
    /// reported with the next event.
    pub fn rename(&self, title: &str) {
        self.append_line(&json!({
            "type": "custom-title",
            "customTitle": title,
            "sessionId": self.session_id,
        }));
        self.state.borrow_mut().custom_title = Some(title.to_string());
    }

    /// The model spawns a subagent: `SubagentStart` on this session, naming the
    /// child, whose context the answer is delivered into.
    ///
    /// The task reaches the server beside the event, read from the metadata file
    /// Claude Code writes for the subagent, which this writes too.
    pub fn subagent(&self, agent_type: &str, task: &str) -> Subagent<'w> {
        let agent_id = {
            let mut state = self.state.borrow_mut();
            state.agents += 1;
            format!("agent-{}", state.agents)
        };
        common::write_subagent_meta(&self.transcript, &agent_id, task);
        let agent = Agent {
            agent_id: agent_id.clone(),
            agent_type: agent_type.to_string(),
        };
        let child = Session {
            world: self.world,
            machine: self.machine.clone(),
            session_id: self.session_id.clone(),
            agent: Some(agent.clone()),
            transcript: subagent_transcript(&self.transcript, &agent_id),
            state: RefCell::new(State {
                cwd: self.state.borrow().cwd.clone(),
                task: Some(cut(task)),
                first_prompt: self.state.borrow().first_prompt.clone(),
                custom_title: self.state.borrow().custom_title.clone(),
                summary: self.state.borrow().summary.clone(),
                ..State::new()
            }),
        };
        create(&child.transcript);

        // The event is the parent's, so it reports the parent's size and the
        // parent's transcript; its answer belongs to the child, which is the
        // context it was planned for.
        let mut common = self.common();
        common.agent = Some(agent);
        let event = payloads::subagent_start(&common);
        let answer = self.post(&event, Some(&cut(task)));
        child.absorb(&answer);
        child
    }

    /// The MCP tools, called the way Claude Code calls them.
    pub fn mcp(&self) -> Mcp<'_, 'w> {
        Mcp::new(self)
    }

    // -- what the model saw ------------------------------------------------

    /// Everything Claude Code injected into this context so far, in order, as
    /// the model saw it: an answer Claude Code saved to a file contributes the
    /// harness line and the preview and nothing more.
    pub fn context(&self) -> String {
        self.state.borrow().injected.join("\n\n")
    }

    /// The files Claude Code saved a whole answer to, in order.
    pub fn persisted_files(&self) -> Vec<PathBuf> {
        self.state.borrow().persisted.clone()
    }

    /// Whether the memory's body is in the text the model can read, which is
    /// not the same as the server having sent it: an answer saved to a file
    /// leaves the model a preview.
    pub fn model_saw_full(&self, memory_id: &str) -> bool {
        self.saw(memory_id).full
    }

    /// Whether the model was given the memory's id and description.
    pub fn model_saw_index(&self, memory_id: &str) -> bool {
        self.saw(memory_id).index
    }

    /// The scopes this context works in, as the server reports them.
    pub fn active_scopes(&self) -> BTreeSet<String> {
        self.world.context(&self.key())["active_scopes"]
            .as_array()
            .expect("a context reports its active scopes as an array")
            .iter()
            .map(|scope| scope.as_str().expect("a scope id is a string").to_string())
            .collect()
    }

    /// How many memories this context is recorded as holding.
    pub fn delivered_count(&self) -> usize {
        self.world.context(&self.key())["delivered_count"]
            .as_u64()
            .expect("a context reports how much it holds") as usize
    }

    // -- sending -----------------------------------------------------------

    fn compaction(&self, trigger: &str) -> Compaction<'w> {
        self.append_line(&json!({
            "type": "summary",
            "summary": COMPACTION_SUMMARY,
            "leafUuid": format!("{}-compact", self.session_id),
        }));
        self.state.borrow_mut().summary = Some(COMPACTION_SUMMARY.to_string());
        let start = self.send(payloads::session_start(&self.common(), "compact"));
        let post = self.send(payloads::post_compact(&self.common(), trigger));
        Compaction { start, post }
    }

    /// The fields every payload of this session carries.
    fn common(&self) -> Common {
        let state = self.state.borrow();
        Common {
            session_id: self.session_id.clone(),
            transcript_path: self
                .transcript
                .to_str()
                .expect("a utf-8 transcript path")
                .to_string(),
            cwd: state.cwd.clone(),
            permission_mode: "default".to_string(),
            agent: self.agent.clone(),
            prompt_id: state.prompt_id.clone(),
        }
    }

    /// Send one event and apply its answer to this context.
    fn send(&self, event: Value) -> Answer<'w> {
        let task = self.state.borrow().task.clone();
        let answer = self.post(&event, task.as_deref());
        self.absorb(&answer);
        Answer::new(self.world, event, answer)
    }

    /// Send one event and report the answer, without applying it anywhere.
    fn post(&self, event: &Value, task: Option<&str>) -> Value {
        self.report_tokens();
        let state = self.state.borrow();
        let title = state.custom_title.clone().or_else(|| state.summary.clone());
        let first_prompt = state.first_prompt.clone();
        let context_tokens = Some(state.tokens);
        drop(state);
        self.world
            .transport()
            .send(&crate::scenario::claude::HookPost {
                base_url: &self.world.url(),
                machine: &self.machine,
                context_tokens,
                session_title: title.as_deref(),
                first_prompt: first_prompt.as_deref(),
                task,
                event,
            })
    }

    /// Put an answer into this context the way Claude Code does, and grow the
    /// context by what it cost.
    fn absorb(&self, answer: &Value) {
        let mut shown = Vec::new();
        for text in injected_strings(answer) {
            let saved_to = self.next_persisted_path();
            if payloads::is_saved_to_a_file(&text) {
                write(&saved_to, &text);
                self.state.borrow_mut().persisted.push(saved_to.clone());
            }
            shown.push(payloads::shown_to_model(&text, &saved_to));
        }
        let cost: u64 = shown
            .iter()
            .map(|text| text.chars().count() as u64 / CHARACTERS_PER_TOKEN)
            .sum();
        let mut state = self.state.borrow_mut();
        state.injected.extend(shown);
        state.tokens += TOKENS_PER_EVENT + cost;
    }

    fn next_persisted_path(&self) -> PathBuf {
        let number = self.state.borrow().persisted.len() + 1;
        self.transcript
            .parent()
            .expect("the transcript is in a directory")
            .join("tool-results")
            .join(format!(
                "hook-{}-{number}-additionalContext.txt",
                self.key().replace('/', "-")
            ))
    }

    fn saw(&self, memory_id: &str) -> crate::scenario::answer::Seen {
        let context = self.context();
        let lines: Vec<&str> = context.lines().map(str::trim).collect();
        let catalog = self.world.catalog();
        let memory = catalog.memory(&forgetmenot_server::store::MemoryId::new(memory_id));
        seen_in(
            &lines,
            memory_id,
            memory
                .map(|entry| entry.document.description())
                .unwrap_or(""),
            memory
                .map(|entry| entry.document.body.as_str())
                .unwrap_or(""),
        )
    }

    // -- the transcript ----------------------------------------------------

    /// Make the transcript's last assistant line report the size the next event
    /// will, which is what the client reads.
    fn report_tokens(&self) {
        let tokens = self.state.borrow().tokens;
        if self.state.borrow().reported_tokens == Some(tokens) {
            return;
        }
        self.append_assistant("");
    }

    /// Append an assistant line carrying what the model said and the size of the
    /// context after it, split over the three counters that make it up.
    fn append_assistant(&self, content: &str) {
        let tokens = self.state.borrow().tokens;
        self.append_line(&json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "usage": {
                    "input_tokens": tokens - 2,
                    "cache_read_input_tokens": 1,
                    "cache_creation_input_tokens": 1,
                    "output_tokens": 7,
                },
                "content": content,
            },
        }));
        self.state.borrow_mut().reported_tokens = Some(tokens);
    }

    fn append_line(&self, line: &Value) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.transcript)
            .unwrap_or_else(|error| {
                panic!("opening {} failed: {error}", self.transcript.display())
            });
        writeln!(
            file,
            "{}",
            serde_json::to_string(line).expect("a transcript line serialises")
        )
        .expect("the transcript is writable");
    }
}

impl State {
    fn new() -> Self {
        Self {
            cwd: None,
            tokens: STARTING_TOKENS,
            reported_tokens: None,
            prompt_id: "prompt-0000".to_string(),
            prompts: 0,
            agents: 0,
            tools: 0,
            custom_title: None,
            summary: None,
            first_prompt: None,
            task: None,
            injected: Vec::new(),
            persisted: Vec::new(),
        }
    }
}

impl<'s, 'w> ToolCall<'s, 'w> {
    /// Check the answer to the `PreToolUse` and carry on.
    pub fn assert(self, check: impl FnOnce(&Answer<'w>)) -> Self {
        let Self {
            session,
            name,
            input,
            tool_use_id,
            answer,
        } = self;
        Self {
            session,
            name,
            input,
            tool_use_id,
            answer: answer.assert(check),
        }
    }

    /// Whether the call was stopped.
    pub fn held(&self) -> bool {
        self.answer.held()
    }

    /// Whether the call goes ahead.
    pub fn allowed(&self) -> bool {
        self.answer.allowed()
    }

    /// The answer to the `PreToolUse`.
    pub(crate) fn into_answer(self) -> Answer<'w> {
        self.answer
    }

    /// The model issues the same call again after reading what stopped it: a
    /// new `PreToolUse` with the same input.
    ///
    /// The call carries a new `tool_use_id`, because it is a new call from the
    /// model; the server does not read it.
    pub fn reissue(&self) -> ToolCall<'s, 'w> {
        assert!(
            self.answer.held(),
            "only a call that was stopped is issued again; this one went ahead"
        );
        self.session.tool(&self.name, self.input.clone())
    }

    /// The call ran and returned `output`: `PostToolUse`.
    ///
    /// A call that was stopped never runs, and Claude Code sends no
    /// `PostToolUse` for it.
    pub fn result(self, output: Value) -> Answer<'w> {
        assert!(
            !self.answer.held(),
            "the call was stopped, so Claude Code sends no PostToolUse for it"
        );
        let event = payloads::post_tool_use(
            &self.session.common(),
            &self.name,
            &self.input,
            &output,
            &self.tool_use_id,
        );
        self.session.send(event)
    }

    /// The call ran and returned nothing of note.
    pub fn run(self) -> Answer<'w> {
        self.result(json!({ "stdout": "" }))
    }
}

/// The two strings Claude Code puts in front of the model for one answer: why a
/// call was stopped, and the context the answer carries. Each goes through the
/// save-to-a-file rule on its own, which is how Claude Code handles them.
fn injected_strings(answer: &Value) -> Vec<String> {
    let Some(output) = answer.get("hookSpecificOutput") else {
        return Vec::new();
    };
    let mut strings = Vec::new();
    for key in ["permissionDecisionReason", "additionalContext"] {
        match output.get(key).and_then(Value::as_str) {
            Some(text) if !text.is_empty() => strings.push(text.to_string()),
            _ => {}
        }
    }
    strings
}

/// Where Claude Code writes a subagent's own transcript: beside its metadata
/// file, under the directory named after the session.
fn subagent_transcript(session_transcript: &Path, agent_id: &str) -> PathBuf {
    session_transcript
        .parent()
        .expect("the transcript is in a directory")
        .join(
            session_transcript
                .file_stem()
                .expect("the transcript is named after its session"),
        )
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"))
}

/// How much of a first prompt or a task travels with an event. Source:
/// `transcript_name::PROMPT_CHARACTERS` and `subagent_meta::TASK_CHARACTERS`,
/// which the client cuts both to.
const REPORTED_CHARACTERS: usize = 200;

fn cut(text: &str) -> String {
    match text.char_indices().nth(REPORTED_CHARACTERS) {
        Some((end, _)) => text[..end].to_string(),
        None => text.to_string(),
    }
}

/// Make sure the transcript is there to append to, without touching one that
/// already is: a session picked up again by its id, after a restart or in a
/// second `Claude`, goes on writing the transcript it was writing before.
fn create(path: &Path) {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .unwrap_or_else(|error| panic!("creating {} failed: {error}", directory.display()));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|error| panic!("creating {} failed: {error}", path.display()));
}

fn write(path: &Path, text: &str) {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .unwrap_or_else(|error| panic!("creating {} failed: {error}", directory.display()));
    }
    std::fs::write(path, text)
        .unwrap_or_else(|error| panic!("writing {} failed: {error}", path.display()));
}
