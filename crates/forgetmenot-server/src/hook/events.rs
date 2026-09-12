//! What one hook event means for a context: which context it belongs to, which
//! texts its triggers are matched against, and what it does to the delivery
//! record.
//!
//! This is the whole of the event-to-text mapping, kept apart from the HTTP
//! handler so that a change to Claude Code's payloads is a change to one file.

use forgetmenot_types::hook::{HookCommon, HookEvent};

use crate::context::{ContextKey, MAIN_AGENT};
use crate::store::scope::TriggerField;

/// The largest tool result matched against triggers. A result larger than this
/// is truncated: matching is linear in the text, and a 10 MB file read must not
/// cost a 10 MB regex pass on the hot path.
pub const TOOL_RESULT_CAP: usize = 256 * 1024;

/// What an event does to the context's delivery record before anything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reset {
    /// Leave the record alone.
    Keep,
    /// Start over: the session is new, so nothing has been delivered into it.
    Fresh,
    /// The scopes stay, the record goes: a compaction removed from the context
    /// everything that had been delivered.
    ClearDelivered,
}

/// One event, reduced to what the state machine needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPlan {
    /// The context this event acts on. For `SubagentStart` that is the child,
    /// not the session the event arrived on.
    pub key: ContextKey,
    pub event_name: &'static str,
    /// The texts to match triggers against, in the order they are matched.
    pub texts: Vec<(TriggerField, String)>,
    pub reset: Reset,
    /// Whether anything is delivered at this event.
    ///
    /// False for `PostCompact` only: Claude Code is rebuilding the context at
    /// that moment, so what is owed is delivered at the next event, where the
    /// injection point is the one every other event uses.
    pub deliver: bool,
    /// Whether a critical arrival may stop the call, which only `PreToolUse`
    /// can do.
    pub may_deny: bool,
    /// Whether the answer also tells the model which scopes it can turn on and
    /// which session key to pass to the MCP tools.
    pub session_start: bool,
}

/// What to do about `event`, or `None` for an event name this server does not
/// know, which is answered with an empty object.
pub fn plan(event: &HookEvent, machine: &str) -> Option<EventPlan> {
    let event_name = event.event_name();
    let common = event.common()?;
    let mut plan = EventPlan {
        key: context_key(machine, common),
        event_name,
        texts: Vec::new(),
        reset: Reset::Keep,
        deliver: true,
        may_deny: false,
        session_start: false,
    };

    match event {
        HookEvent::SessionStart { source, .. } => {
            plan.session_start = true;
            plan.reset = reset_for_source(source);
            push_text(
                &mut plan,
                TriggerField::WorkingDirectory,
                common.cwd.as_deref(),
            );
        }
        HookEvent::UserPromptSubmit { prompt, .. } => {
            push_text(&mut plan, TriggerField::UserMessage, Some(prompt));
        }
        HookEvent::PreToolUse {
            tool_name,
            tool_input,
            ..
        } => {
            plan.may_deny = true;
            push_text(&mut plan, TriggerField::ToolName, Some(tool_name));
            let input = string_leaves(tool_input, usize::MAX);
            push_text(&mut plan, TriggerField::ToolInput, Some(&input));
            push_text(
                &mut plan,
                TriggerField::WorkingDirectory,
                common.cwd.as_deref(),
            );
        }
        HookEvent::PostToolUse { tool_output, .. } => {
            let result = string_leaves(tool_output, TOOL_RESULT_CAP);
            push_text(&mut plan, TriggerField::ToolResult, Some(&result));
        }
        HookEvent::Stop {
            last_assistant_message,
            ..
        } => {
            push_text(
                &mut plan,
                TriggerField::AssistantMessage,
                last_assistant_message.as_deref(),
            );
        }
        HookEvent::CwdChanged { cwd, .. } => {
            // The directory now in force is the variant's own field; the
            // flattened `cwd` of a CwdChanged event is not read.
            push_text(&mut plan, TriggerField::WorkingDirectory, Some(cwd));
        }
        HookEvent::PostCompact { .. } => {
            plan.reset = Reset::ClearDelivered;
            plan.deliver = false;
        }
        HookEvent::SubagentStart {
            agent_id,
            task_description,
            ..
        } => {
            // The event arrives on the parent session but acts on the child,
            // whose id is the variant's own field.
            plan.key = ContextKey::subagent(machine, &common.session_id, agent_id);
            push_text(
                &mut plan,
                TriggerField::UserMessage,
                task_description.as_deref(),
            );
        }
        HookEvent::Unknown => return None,
    }
    Some(plan)
}

/// The context an event belongs to: the subagent's when the event carries an
/// agent id, else the session's own.
fn context_key(machine: &str, common: &HookCommon) -> ContextKey {
    match common.agent_id.as_deref() {
        Some(agent_id) if agent_id != MAIN_AGENT => {
            ContextKey::subagent(machine, &common.session_id, agent_id)
        }
        _ => ContextKey::main(machine, &common.session_id),
    }
}

/// What a session start does to the delivery record.
///
/// An unrecognised source is treated as a start from nothing: a source this
/// server does not know is more likely to be a new kind of fresh session than a
/// resumption, and delivering a memory again costs less than not delivering it.
fn reset_for_source(source: &str) -> Reset {
    match source {
        "resume" => Reset::Keep,
        // The context was rebuilt from a summary, so the scopes still hold but
        // nothing delivered before it is still there.
        "compact" => Reset::ClearDelivered,
        _ => Reset::Fresh,
    }
}

fn push_text(plan: &mut EventPlan, field: TriggerField, text: Option<&str>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(payload: serde_json::Value) -> HookEvent {
        serde_json::from_value(payload).expect("the payload parses as an event")
    }

    /// Detects a tool input flattened in a way that loses a nested value, which
    /// would stop a trigger matching the text a tool was actually called with.
    #[test]
    fn every_string_in_a_tool_input_is_matched_against_triggers() {
        let text = string_leaves(
            &json!({
                "command": "cargo test",
                "timeout": 120,
                "paths": ["crates/widgets", "crates/rocketry"],
                "nested": { "description": "rebuild the widgets" }
            }),
            usize::MAX,
        );
        for expected in [
            "cargo test",
            "crates/widgets",
            "crates/rocketry",
            "rebuild the widgets",
        ] {
            assert!(
                text.lines().any(|line| line == expected),
                "{expected:?} must be one of the matched lines, got {text:?}"
            );
        }
    }

    /// Detects a cap applied per leaf rather than to the whole text, which would
    /// let a tool result of any size reach the regex pass, and a cap that cuts a
    /// multi-byte character in half.
    #[test]
    fn a_large_tool_result_is_capped_and_stays_valid_text() {
        let payload = json!({ "stdout": "e\u{00e9}".repeat(1000) });
        let text = string_leaves(&payload, 9);
        assert!(
            text.len() <= 9,
            "the text must respect the cap, got {}",
            text.len()
        );
        assert!(
            "e\u{00e9}".repeat(1000).starts_with(&text),
            "the capped text must be a prefix of the original, got {text:?}"
        );
    }

    /// Detects a CwdChanged event read from the flattened `cwd` of the common
    /// fields instead of the directory the event reports as now in force: a
    /// directory trigger would then fire for the directory that was left.
    #[test]
    fn cwd_changed_matches_the_directory_now_in_force() {
        let plan = plan(
            &event(json!({
                "hook_event_name": "CwdChanged",
                "session_id": "session-1",
                "cwd": "/work/after",
                "old_cwd": "/work/before"
            })),
            "alpha",
        )
        .expect("CwdChanged is a known event");
        assert_eq!(
            plan.texts,
            vec![(TriggerField::WorkingDirectory, "/work/after".to_string())]
        );
    }

    /// Detects a SubagentStart applied to the parent's context instead of the
    /// child's, which would give the parent the child's deliveries.
    #[test]
    fn subagent_start_acts_on_the_child_context() {
        let plan = plan(
            &event(json!({
                "hook_event_name": "SubagentStart",
                "session_id": "session-1",
                "agent_id": "agent-7",
                "task_description": "survey the widgets crate"
            })),
            "alpha",
        )
        .expect("SubagentStart is a known event");
        assert_eq!(
            plan.key,
            ContextKey::subagent("alpha", "session-1", "agent-7")
        );
        assert_eq!(
            plan.texts,
            vec![(
                TriggerField::UserMessage,
                "survey the widgets crate".to_string()
            )]
        );
    }

    /// Detects a session start that wipes the delivery record when the session
    /// was resumed, or keeps it when the session is new; both would make the
    /// next event deliver the wrong set.
    #[test]
    fn each_session_start_source_resets_what_it_should() {
        assert_eq!(reset_for_source("startup"), Reset::Fresh);
        assert_eq!(reset_for_source("clear"), Reset::Fresh);
        assert_eq!(reset_for_source("fork"), Reset::Fresh);
        assert_eq!(reset_for_source("resume"), Reset::Keep);
        assert_eq!(reset_for_source("compact"), Reset::ClearDelivered);
    }
}
