//! Claude Code hook request and response types.
//!
//! Claude Code runs a hook handler as a child process, writes one JSON object
//! for the event to its stdin and reads one JSON object from its stdout. Three
//! shapes live here:
//!
//! * [`HookEvent`] is the object Claude Code writes to stdin.
//! * [`HookRequest`] is the object the `forgetmenot-hook` client POSTs to the
//!   server's `/hook` endpoint: the hook event plus the two things only the
//!   client can know, the machine name and the size of the session's context.
//! * [`HookResponse`] is the object the handler writes to stdout.

use serde::{Deserialize, Serialize};

/// Body of `POST /hook`, sent by the `forgetmenot-hook` client for every hook
/// event Claude Code delivers to it.
///
/// `hook` is the event JSON as Claude Code wrote it, so the server sees fields
/// this crate does not model yet. `context_tokens` is the context size read
/// from the transcript's last assistant message, `None` when the transcript is
/// missing or carries no usage record yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookRequest {
    /// Name of the machine the Claude Code session runs on, from the client's
    /// `--machine` flag. Claude Code does not report it.
    pub machine: String,
    /// Context size in tokens at the last assistant message of the session.
    pub context_tokens: Option<u64>,
    /// The hook event JSON, verbatim.
    pub hook: serde_json::Value,
}

/// The fields every Claude Code hook event carries, whatever the event is.
///
/// `session_id` is present in all events. The rest are absent in some events
/// and some Claude Code versions, so they are optional; a missing field is not
/// an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookCommon {
    /// Identifier of the Claude Code session, stable for its whole life.
    pub session_id: String,
    /// Path of the session's transcript JSONL file.
    pub transcript_path: Option<String>,
    /// Working directory of the session at the time of the event.
    pub cwd: Option<String>,
    /// Present when the event comes from inside a subagent, absent in the main
    /// session; this is what separates a subagent's context from its parent's.
    pub agent_id: Option<String>,
    /// Permission mode in force, for example `default` or `acceptEdits`.
    pub permission_mode: Option<String>,
}

/// A Claude Code hook event, as written to a hook handler's stdin.
///
/// The enum is tagged by the event's own `hook_event_name` field. Fields
/// Claude Code sends but this crate does not name are ignored, and an event
/// name this crate does not know deserializes to [`HookEvent::Unknown`]
/// instead of failing, so a new Claude Code event cannot break the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum HookEvent {
    /// A session started: `startup`, `resume`, `clear`, `compact` or `fork`,
    /// reported in `source`.
    #[serde(rename = "SessionStart")]
    SessionStart {
        #[serde(flatten)]
        common: HookCommon,
        /// What started the session.
        source: String,
    },
    /// The user submitted a prompt, before the model sees it.
    #[serde(rename = "UserPromptSubmit")]
    UserPromptSubmit {
        #[serde(flatten)]
        common: HookCommon,
        /// The submitted text.
        prompt: String,
    },
    /// The model asked to run a tool and the call has not run yet. This is the
    /// only event whose response may deny the call.
    #[serde(rename = "PreToolUse")]
    PreToolUse {
        #[serde(flatten)]
        common: HookCommon,
        /// Name of the tool, for example `Bash`.
        tool_name: String,
        /// The tool's arguments, shape defined by the tool.
        tool_input: serde_json::Value,
        /// Identifier shared with the matching `PostToolUse` event.
        tool_use_id: Option<String>,
    },
    /// A tool call finished.
    #[serde(rename = "PostToolUse")]
    PostToolUse {
        #[serde(flatten)]
        common: HookCommon,
        /// Name of the tool that ran.
        tool_name: String,
        /// The arguments it ran with.
        tool_input: serde_json::Value,
        /// What the tool returned, shape defined by the tool. Claude Code
        /// 2.1.257 names this field `tool_response` on the wire.
        #[serde(alias = "tool_response")]
        tool_output: serde_json::Value,
    },
    /// The assistant finished its turn.
    #[serde(rename = "Stop")]
    Stop {
        #[serde(flatten)]
        common: HookCommon,
        /// Text of the last assistant message of the turn.
        last_assistant_message: Option<String>,
    },
    /// The session's working directory changed.
    #[serde(rename = "CwdChanged")]
    CwdChanged {
        #[serde(flatten)]
        common: HookCommon,
        /// The directory now in force.
        cwd: String,
        /// The directory left behind. Claude Code 2.1.257 names this field
        /// `old_cwd` on the wire.
        #[serde(alias = "old_cwd")]
        previous_cwd: Option<String>,
    },
    /// A compaction finished, so the context no longer holds what was
    /// delivered before it.
    #[serde(rename = "PostCompact")]
    PostCompact {
        #[serde(flatten)]
        common: HookCommon,
        /// What triggered the compaction, for example `manual` or `auto`.
        trigger: Option<String>,
    },
    /// A subagent started. Its `agent_id` identifies the child context; the
    /// event arrives on the parent session's `session_id`.
    #[serde(rename = "SubagentStart")]
    SubagentStart {
        #[serde(flatten)]
        common: HookCommon,
        /// Identifier of the new subagent context.
        agent_id: String,
        /// The subagent's type, for example `Explore`.
        agent_type: Option<String>,
        /// The task the subagent was given.
        task_description: Option<String>,
    },
    /// Any `hook_event_name` this crate does not know. The name itself is not
    /// kept, which is why [`HookEvent::common`] has nothing to return for it.
    #[serde(other)]
    Unknown,
}

impl HookEvent {
    /// The Claude Code event name this event was tagged with, which is also
    /// what belongs in a [`HookResponse`]'s `hookEventName`.
    pub fn event_name(&self) -> &'static str {
        match self {
            HookEvent::SessionStart { .. } => "SessionStart",
            HookEvent::UserPromptSubmit { .. } => "UserPromptSubmit",
            HookEvent::PreToolUse { .. } => "PreToolUse",
            HookEvent::PostToolUse { .. } => "PostToolUse",
            HookEvent::Stop { .. } => "Stop",
            HookEvent::CwdChanged { .. } => "CwdChanged",
            HookEvent::PostCompact { .. } => "PostCompact",
            HookEvent::SubagentStart { .. } => "SubagentStart",
            HookEvent::Unknown => "Unknown",
        }
    }

    /// The fields shared by all known events, or `None` for an event name this
    /// crate does not know.
    pub fn common(&self) -> Option<&HookCommon> {
        match self {
            HookEvent::SessionStart { common, .. }
            | HookEvent::UserPromptSubmit { common, .. }
            | HookEvent::PreToolUse { common, .. }
            | HookEvent::PostToolUse { common, .. }
            | HookEvent::Stop { common, .. }
            | HookEvent::CwdChanged { common, .. }
            | HookEvent::PostCompact { common, .. }
            | HookEvent::SubagentStart { common, .. } => Some(common),
            HookEvent::Unknown => None,
        }
    }
}

/// What a hook handler writes to stdout for Claude Code to act on.
///
/// Claude Code reads `hookSpecificOutput` and ignores keys it does not know,
/// so the camelCase spelling below is the whole contract: a misspelled key is
/// silently dropped rather than reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookResponse {
    /// The single field Claude Code reads.
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: HookSpecificOutput,
}

/// The payload of a [`HookResponse`].
///
/// `additional_context` is injected into the session's context by Claude Code.
/// `permission_decision` and its reason are read for `PreToolUse` only; a
/// decision of `deny` stops the tool call and shows the reason to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSpecificOutput {
    /// Name of the event being answered; Claude Code requires it to match.
    pub hook_event_name: String,
    /// Text to inject into the session's context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
    /// `allow`, `deny` or `ask`, for `PreToolUse` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_decision: Option<String>,
    /// Why the call was denied, shown to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_decision_reason: Option<String>,
}

impl HookResponse {
    /// A response that asks Claude Code for nothing: the event was seen and
    /// nothing is due.
    pub fn empty(event_name: impl Into<String>) -> Self {
        HookResponse {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: event_name.into(),
                additional_context: None,
                permission_decision: None,
                permission_decision_reason: None,
            },
        }
    }

    /// A response that injects `text` into the session's context.
    pub fn with_context(event_name: impl Into<String>, text: impl Into<String>) -> Self {
        HookResponse {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: event_name.into(),
                additional_context: Some(text.into()),
                permission_decision: None,
                permission_decision_reason: None,
            },
        }
    }

    /// A response that stops a `PreToolUse` call, tells the model why, and
    /// injects `context` so the model can act on it before retrying.
    pub fn deny(
        event_name: impl Into<String>,
        reason: impl Into<String>,
        context: impl Into<String>,
    ) -> Self {
        HookResponse {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: event_name.into(),
                additional_context: Some(context.into()),
                permission_decision: Some("deny".to_string()),
                permission_decision_reason: Some(reason.into()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Detects a renamed or retyped field in PreToolUse, and a loss of
    // tolerance for fields Claude Code sends that this crate does not model:
    // either one makes every real PreToolUse event fail to parse.
    // Expectation source: the Claude Code hook documentation shape, as in the
    // plan's hook contract.
    #[test]
    fn pre_tool_use_event_parses_with_unmodelled_fields_and_keeps_tool_input() {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "session_id": "session-1",
            "transcript_path": "/tmp/forgetmenot-test/session-1.jsonl",
            "cwd": "/tmp/forgetmenot-test/work",
            "permission_mode": "default",
            "prompt_id": "prompt-7",
            "tool_name": "Bash",
            "tool_use_id": "toolu_1",
            "tool_input": { "command": "ls -l", "timeout": 120 }
        });

        let event: HookEvent = serde_json::from_value(payload).expect("PreToolUse must parse");

        let HookEvent::PreToolUse {
            common,
            tool_name,
            tool_input,
            tool_use_id,
        } = event
        else {
            panic!("a PreToolUse payload must parse as the PreToolUse variant");
        };
        assert_eq!(tool_name, "Bash", "tool_name must come from the payload");
        assert_eq!(
            tool_use_id.as_deref(),
            Some("toolu_1"),
            "tool_use_id must come from the payload"
        );
        assert_eq!(
            tool_input,
            json!({ "command": "ls -l", "timeout": 120 }),
            "tool_input must be preserved as the JSON the tool was called with"
        );
        assert_eq!(
            common.session_id, "session-1",
            "the flattened common fields must come from the same object"
        );
        assert_eq!(
            common.permission_mode.as_deref(),
            Some("default"),
            "permission_mode must reach the server"
        );
    }

    // Detects the client or server rejecting an event name added by a newer
    // Claude Code: every hook call would then fail instead of being ignored.
    #[test]
    fn unrecognised_event_name_parses_as_unknown_instead_of_failing() {
        let payload = json!({
            "hook_event_name": "SomeEventAddedLater",
            "session_id": "session-1",
            "whatever": 3
        });

        let event: HookEvent =
            serde_json::from_value(payload).expect("an unknown event name must not be an error");

        assert_eq!(
            event,
            HookEvent::Unknown,
            "an unmodelled event name must fall back to Unknown"
        );
        assert!(
            event.common().is_none(),
            "Unknown carries no common fields, so common() must say so"
        );
    }

    // Detects a serde rename regression in the response: Claude Code ignores
    // keys it does not know, so a snake_case key or a null-valued key would
    // make a deny silently do nothing.
    // Expectation source: the response shape in the plan's hook contract.
    #[test]
    fn deny_response_serialises_the_camel_case_keys_claude_code_reads() {
        let response = HookResponse::deny("PreToolUse", "the reason", "the memory body");

        let encoded = serde_json::to_value(&response).expect("a response must serialise");

        assert_eq!(
            encoded,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "additionalContext": "the memory body",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": "the reason"
                }
            }),
            "deny must serialise to exactly the keys Claude Code reads"
        );
    }

    // Detects a response that carries keys with null values: Claude Code
    // treats a present permissionDecision as a decision, so an empty response
    // must not mention it at all.
    #[test]
    fn empty_response_omits_the_optional_keys_rather_than_sending_null() {
        let encoded = serde_json::to_value(HookResponse::empty("Stop")).expect("must serialise");

        assert_eq!(
            encoded,
            json!({ "hookSpecificOutput": { "hookEventName": "Stop" } }),
            "an empty response must carry the event name and nothing else"
        );
    }

    // Detects the three events whose wire field names this crate renames
    // drifting from what Claude Code actually sends. Expectation source: the
    // payload literals in the Claude Code 2.1.257 binary, read 2026-09-12
    // (`tool_response`, `old_cwd`/`new_cwd`, `trigger`).
    #[test]
    fn events_parse_from_the_field_names_claude_code_2_1_257_sends() {
        let post_tool_use: HookEvent = serde_json::from_value(json!({
            "hook_event_name": "PostToolUse",
            "session_id": "session-1",
            "tool_name": "Bash",
            "tool_input": { "command": "ls" },
            "tool_response": { "stdout": "a\n" },
            "tool_use_id": "toolu_1",
            "duration_ms": 12
        }))
        .expect("PostToolUse must parse");
        let HookEvent::PostToolUse { tool_output, .. } = post_tool_use else {
            panic!("a PostToolUse payload must parse as the PostToolUse variant");
        };
        assert_eq!(
            tool_output,
            json!({ "stdout": "a\n" }),
            "tool_output must be read from the wire field tool_response"
        );

        let cwd_changed: HookEvent = serde_json::from_value(json!({
            "hook_event_name": "CwdChanged",
            "session_id": "session-1",
            "cwd": "/tmp/forgetmenot-test/after",
            "old_cwd": "/tmp/forgetmenot-test/before",
            "new_cwd": "/tmp/forgetmenot-test/after"
        }))
        .expect("CwdChanged must parse");
        let HookEvent::CwdChanged {
            cwd, previous_cwd, ..
        } = cwd_changed
        else {
            panic!("a CwdChanged payload must parse as the CwdChanged variant");
        };
        assert_eq!(
            cwd, "/tmp/forgetmenot-test/after",
            "the directory now in force must be the event's new one"
        );
        assert_eq!(
            previous_cwd.as_deref(),
            Some("/tmp/forgetmenot-test/before"),
            "previous_cwd must be read from the wire field old_cwd"
        );

        let post_compact: HookEvent = serde_json::from_value(json!({
            "hook_event_name": "PostCompact",
            "session_id": "session-1",
            "trigger": "auto",
            "compact_summary": "a summary"
        }))
        .expect("PostCompact must parse");
        assert!(
            matches!(post_compact, HookEvent::PostCompact { trigger: Some(ref t), .. } if t == "auto"),
            "PostCompact must keep its trigger and ignore compact_summary"
        );
    }

    // Detects event_name() drifting from the name the event was tagged with,
    // which would make the server answer with a hookEventName Claude Code
    // rejects.
    #[test]
    fn event_name_matches_the_tag_the_event_was_parsed_from() {
        for name in [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Stop",
            "CwdChanged",
            "PostCompact",
            "SubagentStart",
        ] {
            let payload = json!({
                "hook_event_name": name,
                "session_id": "session-1",
                "source": "startup",
                "prompt": "hello",
                "tool_name": "Bash",
                "tool_input": {},
                "tool_output": {},
                "cwd": "/tmp/forgetmenot-test/work",
                "agent_id": "agent-1"
            });
            let event: HookEvent =
                serde_json::from_value(payload).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                event.event_name(),
                name,
                "event_name() must report the tag the event was parsed from"
            );
            assert!(
                event.common().is_some(),
                "{name}: a known event must expose its common fields"
            );
        }
    }
}
