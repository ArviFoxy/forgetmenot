//! What one hook event means for a context, whatever harness sent it.
//!
//! Each harness has a module that reads its own events into an [`EventPlan`]
//! and writes an [`Answer`] the way that harness reads one: [`super::claude_code`]
//! and [`super::antigravity`]. Everything between the two, the state machine of
//! [`super::handle`], knows the plan and the answer and no harness.

use forgetmenot_types::hook::Harness;

use crate::context::ContextKey;
use crate::render::ANSWER_MARKER;
use crate::store::scope::TriggerField;
use crate::store::settings::Settings;

/// One event, reduced to what the state machine needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPlan {
    /// The context this event acts on. For `SubagentStart` that is the child,
    /// not the session the event arrived on.
    pub key: ContextKey,
    /// The harness the event comes from, which is what writes the answer.
    pub harness: Harness,
    /// The event's name as its harness spells it.
    pub event_name: &'static str,
    /// The texts to match triggers against, in the order they are matched.
    pub texts: Vec<(TriggerField, String)>,
    /// Whether the answer to this event can put text in front of the model.
    /// Where it cannot, the event still turns scopes on, and what the context
    /// is owed is worked out, so that a call can be held for it, but nothing is
    /// recorded as delivered: the next event that can deliver carries it.
    pub delivers: bool,
    /// Whether a pinned arrival may stop the call, which only `PreToolUse`
    /// can do. Whether it does is the store's to say: see
    /// [`Settings::interrupts`].
    pub may_deny: bool,
    /// The tool about to run, for `PreToolUse` alone, which is what the store's
    /// exemptions are matched against.
    pub tool_name: Option<String>,
    /// Whether the answer also tells the model which session key to pass to
    /// the MCP tools.
    pub session_start: bool,
    /// The directory this event reports: for Claude Code the event's own `cwd`,
    /// except for `CwdChanged`, where it is the directory the event puts in
    /// force; for Antigravity the conversation's workspace directories, one per
    /// line. A context that has not remembered a session directory yet takes
    /// this as its own.
    pub cwd: Option<String>,
    /// Whether this event matches the two directory fields. True for the
    /// events that carry a directory worth matching: the session start, every
    /// tool call, and a change of the shell's directory.
    pub matches_directories: bool,
    /// The task a subagent was given, as the client read it from the subagent's
    /// metadata file; absent outside a subagent. At `SubagentStart` it is also
    /// matched against the user-message triggers, because it is what the
    /// subagent was told to do; this field is what names the context.
    pub task: Option<String>,
    /// The first thing the user said in the session, where the event itself
    /// carries it and the client need not read it from a transcript.
    pub first_prompt: Option<String>,
    /// The kind of subagent that started, for `SubagentStart` alone. It is what
    /// names the context when nothing said what the task was.
    pub agent_type: Option<String>,
    /// The context that spawned the one this event acts on, for `SubagentStart`
    /// alone: the spawning subagent when the request named one, else the
    /// session the event arrived on. It is what the new context inherits its
    /// scopes and its session directory from. Every other event acts on a
    /// context that already exists, so it names no spawner.
    pub parent: Option<ContextKey>,
    /// Whether the context size this event reports is the parent's rather than
    /// this plan's own context's, which is true of `SubagentStart` alone.
    ///
    /// The client reads the size from the transcript of the session the event
    /// arrived on, and a `SubagentStart` arrives on the parent while it acts on
    /// the child. The child's transcript begins empty, so its own events report
    /// a size that starts near zero and has nothing to do with the number here.
    /// Everything counted in tokens is therefore left unset by such an event and
    /// counts from the first event of the child that carries the child's own
    /// size; the statistics still record the size as it arrived.
    pub tokens_are_the_parents: bool,
}

/// What the server decided to say to one event, before a harness's module
/// writes it in that harness's own form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The text to put in front of the model, or `None` when nothing is due or
    /// the event cannot carry any.
    pub text: Option<String>,
    /// Whether the tool call the event reports is stopped.
    pub hold: bool,
}

/// Whether `text` is this server's own answer handed back through a tool.
///
/// An answer past the store's threshold is written to a file by Claude Code,
/// which shows the model a preview and the path; the notice at the top of such
/// an answer tells the model to read that file, so the whole answer comes back
/// as the result of a `Read` — every scope id of the available list and the
/// text of every pinned memory delivered. Matched against the tool-result
/// triggers, that one result activates every scope the answer names, which is
/// the store's own traffic and not the session's work.
///
/// The mark is [`ANSWER_MARKER`], the line every answer opens with. It is
/// looked for on any line, because the notice comes before it and a `Read`
/// prints the file's lines numbered: the digits, the whitespace and the `\u{2192}`
/// or tab Claude Code puts in front of a line are stripped before comparing.
pub fn carries_own_answer(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim_start_matches(|character: char| {
            character.is_ascii_digit() || character.is_whitespace() || character == '\u{2192}'
        })
        .starts_with(ANSWER_MARKER)
    })
}

/// Whether the store lists `tool_name` among the tools whose name, input and
/// result are kept out of the texts the triggers are matched against. The
/// names are the harness's own, so a store used from two harnesses lists a
/// tool under each name it goes by.
pub fn listed_exempt(settings: &Settings, tool_name: &str) -> bool {
    settings
        .trigger_exempt_tools
        .iter()
        .any(|exempt| exempt == tool_name)
}

pub fn push_text(plan: &mut EventPlan, field: TriggerField, text: Option<&str>) {
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        plan.texts.push((field, text.to_string()));
    }
}

/// Every string in a JSON value, in document order, joined by newlines and
/// truncated to `cap` bytes on a character boundary.
///
/// Object keys are left out: a trigger matches what a tool was called with or
/// what it returned, not the names of the fields carrying it.
pub fn string_leaves(value: &serde_json::Value, cap: usize) -> String {
    let mut text = String::new();
    collect_strings(value, cap, &mut text);
    text
}

fn collect_strings(value: &serde_json::Value, cap: usize, text: &mut String) {
    if text.len() >= cap {
        return;
    }
    match value {
        serde_json::Value::String(leaf) => {
            if !text.is_empty() {
                text.push('\n');
            }
            let room = cap.saturating_sub(text.len());
            if leaf.len() <= room {
                text.push_str(leaf);
            } else {
                let mut end = room;
                while end > 0 && !leaf.is_char_boundary(end) {
                    end -= 1;
                }
                text.push_str(&leaf[..end]);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_strings(item, cap, text);
            }
        }
        serde_json::Value::Object(fields) => {
            for field in fields.values() {
                collect_strings(field, cap, text);
            }
        }
        _ => {}
    }
}
