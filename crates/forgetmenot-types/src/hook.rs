//! What the `forgetmenot-hook` client sends the server for one hook event.
//!
//! A coding agent's harness runs the client as a child process for every hook
//! event, with the event as JSON on its stdin, and reads the client's stdout.
//! Each harness has its own events and its own answers, and each has a module
//! of its own: [`crate::claude_code`] and [`crate::antigravity`]. What the
//! client POSTs to the server is the same for both, and lives here.

use serde::{Deserialize, Serialize};

/// The harness a hook event comes from: the program that runs the model, calls
/// its tools and runs the hooks.
///
/// It decides how the event in a [`HookRequest`] is read and what the answer
/// looks like. The two are peers: neither one's events are expressed in the
/// other's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    /// Anthropic's Claude Code. The default, because a client older than this
    /// field is a Claude Code client.
    #[default]
    ClaudeCode,
    /// Google's Antigravity command-line agent (`agy`).
    Antigravity,
}

impl Harness {
    /// The name the client's `--harness` flag takes and the wire carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::ClaudeCode => "claude-code",
            Harness::Antigravity => "antigravity",
        }
    }

    /// The harness `name` names, or `None` for a name that is neither.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude-code" => Some(Harness::ClaudeCode),
            "antigravity" => Some(Harness::Antigravity),
            _ => None,
        }
    }
}

/// Body of `POST /hook`, sent by the `forgetmenot-hook` client for every hook
/// event a harness delivers to it.
///
/// `hook` is the event JSON as the harness wrote it, so the server sees fields
/// this crate does not model yet, and `harness` says whose event it is.
/// `context_tokens` is the context size read from the session's transcript,
/// `None` when the transcript is missing or carries no usage record yet.
/// `session_title` and `first_prompt` are read from the same transcript and say
/// what the session is, where no hook event carries it. `task` and
/// `parent_agent_id` are read from the subagent metadata file beside a Claude
/// Code transcript and say what a subagent was asked to do and which agent asked
/// it. `last_assistant_message` is read from an Antigravity transcript, whose
/// `Stop` event does not carry it.
///
/// Everything but `machine` and `hook` defaults to absent, because a client
/// older than the field it does not know still POSTs a body the server must
/// accept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookRequest {
    /// Name of the machine the session runs on, from the client's `--machine`
    /// flag. No harness reports it.
    pub machine: String,
    /// The harness the event comes from, from the client's `--harness` flag.
    #[serde(default)]
    pub harness: Harness,
    /// Context size in tokens at the last model message of the session.
    pub context_tokens: Option<u64>,
    /// The name the user gave the session with Claude Code's `/rename`, from
    /// the last `custom-title` line of the transcript. `None` when the session
    /// was never named, and for a harness that keeps no name in its transcript.
    #[serde(default)]
    pub session_title: Option<String>,
    /// The first thing the user said in the session, cut to 200 characters.
    /// `None` when the transcript carries none yet, and for a harness whose
    /// events carry the prompt themselves.
    #[serde(default)]
    pub first_prompt: Option<String>,
    /// The task the parent gave the subagent this event comes from, cut to 200
    /// characters, as the client read it from the subagent's metadata file.
    /// `None` for an event outside a subagent, and for one whose metadata file
    /// is missing or unreadable.
    #[serde(default)]
    pub task: Option<String>,
    /// The agent that spawned the subagent this event comes from, as the client
    /// read it from the subagent's metadata file. `None` when the session
    /// spawned it, which is what the file says by carrying no such agent, and
    /// for an event that comes from no subagent at all.
    #[serde(default)]
    pub parent_agent_id: Option<String>,
    /// The text of the model's last message, as the client read it from the
    /// transcript at an Antigravity `Stop`. `None` at every other event: Claude
    /// Code's `Stop` carries the text itself.
    #[serde(default)]
    pub last_assistant_message: Option<String>,
    /// The hook event JSON, verbatim.
    pub hook: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Detects a request from a client older than the `harness` field being
    // refused or read as some other harness: every Claude Code machine runs
    // such a client until it is updated, and its events must keep working.
    #[test]
    fn a_request_without_a_harness_is_a_claude_code_request() {
        let request: HookRequest = serde_json::from_value(json!({
            "machine": "alpha",
            "context_tokens": null,
            "hook": { "hook_event_name": "Stop", "session_id": "session-1" }
        }))
        .expect("a request from an older client must parse");

        assert_eq!(request.harness, Harness::ClaudeCode);
        assert_eq!(request.last_assistant_message, None);
    }

    // Detects the flag's spelling and the wire's spelling drifting apart: the
    // client writes `as_str` into the body and the server reads it with serde.
    #[test]
    fn a_harness_is_written_on_the_wire_as_the_name_its_flag_takes() {
        for harness in [Harness::ClaudeCode, Harness::Antigravity] {
            assert_eq!(
                serde_json::to_value(harness).expect("a harness serialises"),
                json!(harness.as_str())
            );
            assert_eq!(Harness::parse(harness.as_str()), Some(harness));
        }
        assert_eq!(Harness::parse("some-other-harness"), None);
    }
}
