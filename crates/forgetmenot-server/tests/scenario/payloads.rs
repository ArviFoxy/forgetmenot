//! The JSON Claude Code writes to a hook's stdin, and the model of what Claude
//! Code does with the answer it reads back.
//!
//! Both live here and nowhere else in the scenario library, so that a change in
//! Claude Code is a change to one file. The payload shapes are held to the
//! recordings under `tests/fixtures/hooks/` by the tests at the bottom, and the
//! answer model is held to the constants those tests name.

use std::path::Path;

use serde_json::{Map, Value, json};

/// The Claude Code whose payloads, answer limit and preview rule are modelled
/// here. Every constant and every key below was read from this version.
pub const CLAUDE_CODE_VERSION: &str = "2.1.270";

/// Characters of one hook answer string above which Claude Code writes the
/// whole string to a file and shows the model a preview of it instead.
///
/// Counted in UTF-16 code units, which is what a JavaScript string length is;
/// the server counts the same way in `render::render`.
pub const ANSWER_CHARACTERS: usize = 10_000;

/// Characters of the preview Claude Code shows in place of an answer it saved
/// to a file.
pub const PREVIEW_CHARACTERS: usize = 2_000;

/// The preview is cut back to the last newline inside it, but only when that
/// newline lies past this many characters: a preview whose only newline is near
/// the start would otherwise be cut to almost nothing.
pub const PREVIEW_NEWLINE_FLOOR: usize = 1_000;

/// The fields every hook payload carries, and the two that only an event from
/// inside a subagent does.
#[derive(Clone, Debug)]
pub struct Common {
    pub session_id: String,
    pub transcript_path: String,
    /// Absent only for a session that has never reported a directory.
    pub cwd: Option<String>,
    pub permission_mode: String,
    /// The subagent the event comes from, or the subagent a `SubagentStart`
    /// announces; absent in a session's own events.
    pub agent: Option<Agent>,
    /// Claude Code numbers the events of one user turn with a shared id.
    pub prompt_id: String,
}

/// The subagent an event belongs to.
#[derive(Clone, Debug)]
pub struct Agent {
    pub agent_id: String,
    pub agent_type: String,
}

impl Common {
    /// The keys every event carries, without the ones that depend on the event.
    fn base(&self) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("session_id".to_string(), json!(self.session_id));
        payload.insert("transcript_path".to_string(), json!(self.transcript_path));
        payload.insert("cwd".to_string(), json!(self.cwd));
        payload.insert("permission_mode".to_string(), json!(self.permission_mode));
        if let Some(agent) = &self.agent {
            payload.insert("agent_id".to_string(), json!(agent.agent_id));
            payload.insert("agent_type".to_string(), json!(agent.agent_type));
        }
        payload
    }

    /// The base keys and the id shared by the events of one user turn, which
    /// the events of a turn carry and a session-level event does not.
    fn in_turn(&self) -> Map<String, Value> {
        let mut payload = self.base();
        payload.insert("prompt_id".to_string(), json!(self.prompt_id));
        payload
    }
}

/// The model Claude Code reports at a session start.
pub const MODEL: &str = "claude-opus-5";

/// `SessionStart`. `source` is `startup`, `resume`, `clear`, `compact` or
/// `fork`.
pub fn session_start(common: &Common, source: &str) -> Value {
    let mut payload = common.base();
    payload.insert("hook_event_name".to_string(), json!("SessionStart"));
    payload.insert("source".to_string(), json!(source));
    payload.insert("model".to_string(), json!(MODEL));
    Value::Object(payload)
}

/// `UserPromptSubmit`, sent before the model sees the prompt.
pub fn user_prompt_submit(common: &Common, prompt: &str) -> Value {
    let mut payload = common.in_turn();
    payload.insert("hook_event_name".to_string(), json!("UserPromptSubmit"));
    payload.insert("prompt".to_string(), json!(prompt));
    Value::Object(payload)
}

/// `PreToolUse`, the only event whose answer may stop the call.
pub fn pre_tool_use(
    common: &Common,
    tool_name: &str,
    tool_input: &Value,
    tool_use_id: &str,
) -> Value {
    let mut payload = common.in_turn();
    payload.insert("hook_event_name".to_string(), json!("PreToolUse"));
    payload.insert("tool_name".to_string(), json!(tool_name));
    payload.insert("tool_input".to_string(), tool_input.clone());
    payload.insert("tool_use_id".to_string(), json!(tool_use_id));
    Value::Object(payload)
}

/// `PostToolUse`, sent once the call has run.
///
/// The recorded payload names the result `tool_output`; the server also accepts
/// `tool_response`, which an older Claude Code sent.
pub fn post_tool_use(
    common: &Common,
    tool_name: &str,
    tool_input: &Value,
    tool_output: &Value,
    tool_use_id: &str,
) -> Value {
    let mut payload = common.in_turn();
    payload.insert("hook_event_name".to_string(), json!("PostToolUse"));
    payload.insert("tool_name".to_string(), json!(tool_name));
    payload.insert("tool_input".to_string(), tool_input.clone());
    payload.insert("tool_use_id".to_string(), json!(tool_use_id));
    payload.insert("tool_output".to_string(), tool_output.clone());
    Value::Object(payload)
}

/// `Stop`, sent when the assistant's turn ends.
pub fn stop(common: &Common, last_assistant_message: &str) -> Value {
    let mut payload = common.in_turn();
    payload.insert("hook_event_name".to_string(), json!("Stop"));
    payload.insert(
        "last_assistant_message".to_string(),
        json!(last_assistant_message),
    );
    Value::Object(payload)
}

/// `CwdChanged`, sent when the session's shell moves.
pub fn cwd_changed(common: &Common, previous_cwd: Option<&str>, cwd: &str) -> Value {
    let mut payload = common.base();
    // The event reports the directory it puts in force in `cwd` as well, so the
    // common `cwd` this payload already carries is overwritten with it.
    payload.insert("cwd".to_string(), json!(cwd));
    payload.insert("previous_cwd".to_string(), json!(previous_cwd));
    payload.insert("hook_event_name".to_string(), json!("CwdChanged"));
    Value::Object(payload)
}

/// The tokens a compaction reports having removed. A fixed number: nothing the
/// server does depends on it, and the recorded payload carries one.
pub const TOKENS_REMOVED: u64 = 150_000;

/// `PostCompact`. `trigger` is `manual` or `auto`.
pub fn post_compact(common: &Common, trigger: &str) -> Value {
    let mut payload = common.base();
    payload.insert("hook_event_name".to_string(), json!("PostCompact"));
    payload.insert("trigger".to_string(), json!(trigger));
    payload.insert("tokens_removed".to_string(), json!(TOKENS_REMOVED));
    Value::Object(payload)
}

/// `SubagentStart`, which arrives on the parent session and names the child.
///
/// `common.agent` is the child: the event carries the new subagent's id and
/// type in the same two keys an event from inside a subagent carries its own.
/// It says nothing about the task, which is why the client sends that beside
/// the event.
pub fn subagent_start(common: &Common) -> Value {
    let mut payload = common.base();
    payload.insert("hook_event_name".to_string(), json!("SubagentStart"));
    Value::Object(payload)
}

// ---------------------------------------------------------------------------
// What Claude Code does with the answer
// ---------------------------------------------------------------------------

/// What the model is shown for one string a hook answered with.
///
/// Up to [`ANSWER_CHARACTERS`] the string itself. Past it, Claude Code writes
/// the whole string to `saved_to` and shows the harness line naming that file
/// followed by [`preview`] of the string, which is what this returns.
pub fn shown_to_model(text: &str, saved_to: &Path) -> String {
    if text.encode_utf16().count() <= ANSWER_CHARACTERS {
        return text.to_string();
    }
    format!(
        "Output too large ({}). Full output saved to: {}\n\nPreview (first 2KB):\n{}",
        size_label(text),
        saved_to.display(),
        preview(text)
    )
}

/// Whether Claude Code saves this answer to a file instead of showing it.
pub fn is_saved_to_a_file(text: &str) -> bool {
    text.encode_utf16().count() > ANSWER_CHARACTERS
}

/// The part of a saved answer the model is shown: its first
/// [`PREVIEW_CHARACTERS`] characters, cut back to the last newline among them
/// when that newline lies past [`PREVIEW_NEWLINE_FLOOR`].
pub fn preview(text: &str) -> String {
    let head: Vec<char> = text.chars().take(PREVIEW_CHARACTERS).collect();
    let kept = match head.iter().rposition(|character| *character == '\n') {
        Some(newline) if newline > PREVIEW_NEWLINE_FLOOR => &head[..newline],
        _ => &head[..],
    };
    kept.iter().collect()
}

/// The size the harness line reports, in the form it writes it.
fn size_label(text: &str) -> String {
    format!("{:.1}KB", text.len() as f64 / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common;
    use std::collections::BTreeSet;

    /// Keys Claude Code carries on some recordings of an event and not on
    /// others, so a payload that lacks one is not a payload of the wrong shape.
    ///
    /// `effort` is written only when the session runs with an effort level set:
    /// two of the three recorded `PreToolUse` payloads carry it and one does
    /// not.
    const VARYING_KEYS: [&str; 1] = ["effort"];

    /// The one recorded payload the simulator has no event for: it is an event
    /// name the server does not know, recorded so that the server's answer to
    /// one can be tested, and Claude Code 2.1.270 sends no such event.
    const NOT_SENT_BY_THE_SIMULATOR: [&str; 1] = ["unknown_event"];

    fn common_for(fixture: &Value) -> Common {
        Common {
            session_id: "session-1".to_string(),
            transcript_path: "/nonexistent/transcript.jsonl".to_string(),
            cwd: Some("/home/dev/widgets".to_string()),
            permission_mode: "default".to_string(),
            agent: fixture.get("agent_id").map(|_| Agent {
                agent_id: "agent-7f3a".to_string(),
                agent_type: "general-purpose".to_string(),
            }),
            prompt_id: "6f1d2c3e-0000-4000-8000-000000000001".to_string(),
        }
    }

    /// The payload the simulator sends for the event one recording is of.
    fn payload_like(fixture: &Value) -> Option<Value> {
        let common = common_for(fixture);
        let event = fixture.get("hook_event_name")?.as_str()?;
        Some(match event {
            "SessionStart" => session_start(&common, "startup"),
            "UserPromptSubmit" => user_prompt_submit(&common, "look at the rocketry notes"),
            "PreToolUse" => pre_tool_use(&common, "Bash", &json!({ "command": "ls" }), "toolu_1"),
            "PostToolUse" => post_tool_use(
                &common,
                "Bash",
                &json!({ "command": "ls" }),
                &json!("a\n"),
                "toolu_1",
            ),
            "Stop" => stop(&common, "the build is green"),
            "CwdChanged" => cwd_changed(&common, Some("/home/dev/widgets"), "/home/dev/rocketry"),
            "PostCompact" => post_compact(&common, "auto"),
            "SubagentStart" => subagent_start(&common),
            _ => return None,
        })
    }

    fn keys(payload: &Value) -> BTreeSet<String> {
        payload
            .as_object()
            .expect("a hook payload is a JSON object")
            .keys()
            .filter(|key| !VARYING_KEYS.contains(&key.as_str()))
            .cloned()
            .collect()
    }

    /// Detects the simulator drifting from the payloads Claude Code writes: a
    /// key it stops sending is a field the server never sees in a scenario
    /// though it sees it in life, and a key it invents is a scenario asserting
    /// something about an event that does not exist.
    ///
    /// Expectation source: the payloads under `tests/fixtures/hooks/`, recorded
    /// from Claude Code 2.1.270.
    #[test]
    fn the_simulator_sends_the_keys_the_recorded_payloads_carry() {
        let mut checked = 0;
        for name in common::hook_fixture_names() {
            if NOT_SENT_BY_THE_SIMULATOR.contains(&name.as_str()) {
                continue;
            }
            let fixture = common::hook_fixture(&name);
            let sent = payload_like(&fixture)
                .unwrap_or_else(|| panic!("{name}: the simulator sends no event of this name"));
            assert_eq!(
                keys(&sent),
                keys(&fixture),
                "{name}: the simulator's payload must carry the keys the recording does"
            );
            checked += 1;
        }
        assert!(
            checked >= 8,
            "only {checked} recordings were checked, so most events are covered by nothing"
        );
    }

    /// Detects the recorded payloads and the simulator agreeing on the keys but
    /// not on the event: a payload built for the wrong event name would pass
    /// the key check for any two events that happen to carry the same fields.
    #[test]
    fn each_payload_names_the_event_the_recording_names() {
        for name in common::hook_fixture_names() {
            if NOT_SENT_BY_THE_SIMULATOR.contains(&name.as_str()) {
                continue;
            }
            let fixture = common::hook_fixture(&name);
            let sent = payload_like(&fixture).expect("the simulator sends this event");
            assert_eq!(
                sent.get("hook_event_name"),
                fixture.get("hook_event_name"),
                "{name}: the payload must be of the event the recording is of"
            );
        }
    }

    /// Detects the answer model drifting from the numbers it was measured at:
    /// every `model_saw_full` answer depends on them, and a threshold that is
    /// wrong makes those assertions report the opposite of the truth.
    ///
    /// Expectation source: Claude Code 2.1.270, where the limit is compared
    /// against the JavaScript string length, and the server's own
    /// `DEFAULT_ANSWER_FILE_THRESHOLD`, which is read from the same place.
    #[test]
    fn the_answer_model_holds_the_measured_constants() {
        assert_eq!(CLAUDE_CODE_VERSION, "2.1.270");
        assert_eq!(ANSWER_CHARACTERS, 10_000);
        assert_eq!(PREVIEW_CHARACTERS, 2_000);
        assert_eq!(PREVIEW_NEWLINE_FLOOR, 1_000);
        assert_eq!(
            forgetmenot_server::store::settings::DEFAULT_ANSWER_FILE_THRESHOLD as usize,
            ANSWER_CHARACTERS,
            "the store's default notice threshold is the limit this models, so the notice \
             is sent exactly for the answers Claude Code saves to a file"
        );
    }

    /// Detects an answer under the limit being previewed, which would hide from
    /// `model_saw_full` a body the model was shown whole.
    #[test]
    fn an_answer_at_the_limit_is_shown_whole() {
        let text = "x".repeat(ANSWER_CHARACTERS);
        assert!(!is_saved_to_a_file(&text));
        assert_eq!(shown_to_model(&text, Path::new("/saved")), text);
    }

    /// Detects a limit counted in bytes: a non-ASCII answer Claude Code shows
    /// whole would then be modelled as saved to a file, and every
    /// `model_saw_full` over it would report nothing seen.
    #[test]
    fn the_limit_counts_utf16_units_and_not_bytes() {
        // Three bytes each, one UTF-16 unit each.
        let text = "\u{4e2d}".repeat(ANSWER_CHARACTERS);
        assert_eq!(text.len(), 3 * ANSWER_CHARACTERS);
        assert!(
            !is_saved_to_a_file(&text),
            "an answer of {ANSWER_CHARACTERS} UTF-16 units is shown whole whatever it weighs"
        );
    }

    /// Detects a preview that is not cut at a line: the model would be shown
    /// half a line of a memory body and `model_saw_full` would count the
    /// fragment as the body having arrived.
    #[test]
    fn a_saved_answer_is_previewed_up_to_the_last_whole_line() {
        let line = format!("{}\n", "a".repeat(99));
        let text = line.repeat(200);
        assert!(is_saved_to_a_file(&text));

        let shown = shown_to_model(&text, Path::new("/saved/answer.txt"));

        assert!(
            shown.starts_with("Output too large ("),
            "the model is shown the harness line first, got {:?}",
            &shown[..shown.len().min(80)]
        );
        assert!(
            shown.contains("Full output saved to: /saved/answer.txt"),
            "the harness line names the file the answer went to, got {shown:?}"
        );
        let preview = preview(&text);
        assert_eq!(
            preview.chars().count(),
            // 20 whole lines of 100 characters, the last newline dropped.
            1999,
            "the preview keeps whole lines only"
        );
        assert!(
            preview.chars().filter(|c| *c == '\n').count() == 19,
            "the preview keeps the newlines of the lines it kept"
        );
    }

    /// Detects a cut that fires for a newline near the start of the preview,
    /// which would show the model a few characters instead of two kilobytes.
    #[test]
    fn a_preview_whose_only_newline_is_early_is_not_cut_back_to_it() {
        let text = format!("first line\n{}", "b".repeat(20_000));

        let preview = preview(&text);

        assert_eq!(
            preview.chars().count(),
            PREVIEW_CHARACTERS,
            "the only newline is at character 10, well before the floor, so nothing is cut"
        );
    }
}
