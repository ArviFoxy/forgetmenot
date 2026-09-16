//! What one hook event means for a context: which context it belongs to, which
//! texts its triggers are matched against, and what its answer may do.
//!
//! This is the whole of the event-to-text mapping, kept apart from the HTTP
//! handler so that a change to Claude Code's payloads is a change to one file.

use forgetmenot_types::hook::{HookCommon, HookEvent};

use crate::context::{ContextKey, MAIN_AGENT};
use crate::store::scope::TriggerField;
use crate::store::settings::Settings;

/// One event, reduced to what the state machine needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPlan {
    /// The context this event acts on. For `SubagentStart` that is the child,
    /// not the session the event arrived on.
    pub key: ContextKey,
    pub event_name: &'static str,
    /// The texts to match triggers against, in the order they are matched.
    pub texts: Vec<(TriggerField, String)>,
    /// Whether a critical arrival may stop the call, which only `PreToolUse`
    /// can do. Whether it does is the store's to say: see
    /// [`Settings::interrupts`].
    pub may_deny: bool,
    /// The tool about to run, for `PreToolUse` alone, which is what the store's
    /// exemptions are matched against.
    pub tool_name: Option<String>,
    /// Whether the answer also tells the model which scopes it can turn on and
    /// which session key to pass to the MCP tools.
    pub session_start: bool,
    /// The directory this event reports: the event's own `cwd`, except for
    /// `CwdChanged`, where it is the directory the event puts in force. A
    /// context that has not remembered a session directory yet takes this one
    /// as its own.
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
    /// The kind of subagent that started, for `SubagentStart` alone. It is what
    /// names the context when nothing said what the task was.
    pub agent_type: Option<String>,
}

/// What to do about `event`, or `None` for an event name this server does not
/// know, which is answered with an empty object.
///
/// `task` is what the request said the subagent this event comes from was asked
/// to do; no hook event carries it, so it is the client's read of the
/// subagent's metadata file or nothing.
///
/// `settings` are the store's, because how much of a tool result is matched
/// against triggers is part of what the store says about its own behaviour.
pub fn plan(
    event: &HookEvent,
    machine: &str,
    task: Option<&str>,
    settings: &Settings,
) -> Option<EventPlan> {
    let event_name = event.event_name();
    let common = event.common()?;
    let mut plan = EventPlan {
        key: context_key(machine, common),
        event_name,
        texts: Vec::new(),
        may_deny: false,
        tool_name: None,
        session_start: false,
        cwd: common.cwd.clone(),
        matches_directories: false,
        task: task.filter(|task| !task.is_empty()).map(str::to_string),
        agent_type: None,
    };

    match event {
        HookEvent::SessionStart { .. } => {
            plan.session_start = true;
            plan.matches_directories = true;
            push_text(
                &mut plan,
                TriggerField::ShellDirectory,
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
            plan.matches_directories = true;
            plan.tool_name = Some(tool_name.clone());
            push_text(&mut plan, TriggerField::ToolName, Some(tool_name));
            let input = string_leaves(tool_input, usize::MAX);
            push_text(&mut plan, TriggerField::ToolInput, Some(&input));
            push_text(
                &mut plan,
                TriggerField::ShellDirectory,
                common.cwd.as_deref(),
            );
        }
        HookEvent::PostToolUse { tool_output, .. } => {
            let result = string_leaves(tool_output, settings.tool_result_cap());
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
            plan.matches_directories = true;
            plan.cwd = Some(cwd.clone());
            push_text(&mut plan, TriggerField::ShellDirectory, Some(cwd));
        }
        // Claude Code takes no context from the answer to a compaction, and
        // sends a `SessionStart` with source `compact` for the conversation it
        // rebuilds, which is the event everything due is delivered at. So this
        // one is acknowledged and acted on no further.
        HookEvent::PostCompact { .. } => return None,
        HookEvent::SubagentStart {
            agent_id,
            agent_type,
            ..
        } => {
            // The event arrives on the parent session but acts on the child,
            // whose id is the variant's own field.
            plan.key = ContextKey::subagent(machine, &common.session_id, agent_id);
            plan.agent_type = agent_type.clone();
            // The task is what the subagent was told to do, so it is matched
            // against the user-message triggers once, where the subagent
            // begins, rather than again at every event inside it.
            let task = plan.task.clone();
            push_text(&mut plan, TriggerField::UserMessage, task.as_deref());
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

    /// The plan for one event on `machine`, under a store that has said nothing
    /// about its behaviour and a request that read no task.
    fn plan_of(event: &HookEvent, machine: &str) -> Option<EventPlan> {
        plan(event, machine, None, &Settings::default())
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
        let plan = plan_of(
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
            vec![(TriggerField::ShellDirectory, "/work/after".to_string())]
        );
    }

    /// Detects a SubagentStart applied to the parent's context instead of the
    /// child's, which would give the parent the child's deliveries, and a task
    /// that never reaches the triggers, which would leave a subagent working
    /// without the scopes its own task calls for.
    ///
    /// The payload is the shape Claude Code 2.1.270 sends, which carries no
    /// task: the task comes with the request, from the subagent's metadata
    /// file, so it is given here as the client would send it.
    #[test]
    fn subagent_start_acts_on_the_child_context_and_matches_the_task_it_was_given() {
        let plan = plan(
            &event(json!({
                "hook_event_name": "SubagentStart",
                "session_id": "session-1",
                "agent_id": "agent-7",
                "agent_type": "general-purpose"
            })),
            "alpha",
            Some("survey the widgets crate"),
            &Settings::default(),
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
        assert_eq!(
            plan.agent_type.as_deref(),
            Some("general-purpose"),
            "the kind of subagent that started must reach the context that records it"
        );
    }
}
