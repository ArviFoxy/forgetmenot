//! Antigravity's hook events and the answers it reads.
//!
//! Antigravity, Google's command-line agent (`agy`), runs a hook handler as a
//! child process, writes one JSON object for the event to its stdin and reads
//! one JSON object from its stdout. Two shapes live here:
//!
//! * [`AntigravityEvent`] is the object Antigravity writes to stdin.
//! * [`AntigravityReply`] is the object the handler writes to stdout.
//!
//! The field names are the ones Antigravity 1.5.12 sent to a hook on
//! 2026-11-20, which are more than its documentation lists: every event also
//! carried `hookEventName`, which is what the event is read by.
//!
//! Antigravity has six events. Four are modelled, because a memory system can
//! do something at them. The other two read as [`AntigravityEvent::Unknown`]:
//! `PostToolUse` carries the call and not what the tool returned, and its
//! answer can say nothing; `PostInvocation` follows a model call that the next
//! `PreInvocation` or `Stop` reports as well.

use serde::{Deserialize, Serialize};

/// The fields every Antigravity hook event carries, whatever the event is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityCommon {
    /// Identifier of the conversation, stable for its whole life. It is what
    /// the server knows the session by.
    pub conversation_id: String,
    /// The directories the conversation works in, sorted by name. The first is
    /// not the project in any sense: a frontend may add directories of its own,
    /// and one of those may sort first.
    #[serde(default)]
    pub workspace_paths: Vec<String>,
    /// Path of the conversation's transcript JSONL file.
    pub transcript_path: Option<String>,
    /// The latest thing the user sent. Antigravity repeats it in every event of
    /// the turn, not only where the turn begins.
    pub last_user_input: Option<String>,
}

/// The tool call a `PreToolUse` event reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AntigravityToolCall {
    /// Name of the tool, for example `run_command`. Every call of a tool an MCP
    /// server offers arrives as `call_mcp_tool`, with the server and the tool in
    /// its arguments.
    pub name: String,
    /// The tool's arguments, shape defined by the tool.
    #[serde(default)]
    pub args: serde_json::Value,
}

/// An Antigravity hook event, as written to a hook handler's stdin.
///
/// The enum is tagged by the event's `hookEventName` field. Fields Antigravity
/// sends but this crate does not name are ignored, and an event name this crate
/// does not model deserializes to [`AntigravityEvent::Unknown`] instead of
/// failing, so a new Antigravity event cannot break the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "hookEventName")]
pub enum AntigravityEvent {
    /// A conversation began. It fires once, before the first model call, with
    /// the user's first prompt already in `lastUserInput`.
    SessionStart {
        #[serde(flatten)]
        common: AntigravityCommon,
    },
    /// The model is about to be called. A turn calls it once before each of the
    /// model's steps, so this is the event that fires most often after the tool
    /// events.
    PreInvocation {
        #[serde(flatten)]
        common: AntigravityCommon,
        /// The number of this model call within the turn. It restarts at 0
        /// whenever something new reaches the conversation: a prompt, a message
        /// sent while the agent works, or the end of background work. Antigravity
        /// has no event for a submitted prompt, so a 0 here is how one is told.
        #[serde(rename = "invocationNum")]
        invocation_number: Option<u64>,
    },
    /// The model asked to run a tool and the call has not run yet. This is the
    /// only event whose answer may stop the call.
    PreToolUse {
        #[serde(flatten)]
        common: AntigravityCommon,
        /// The call about to run.
        #[serde(rename = "toolCall")]
        tool_call: AntigravityToolCall,
    },
    /// The agent finished its turn. The event does not carry what the model
    /// said last; the client reads that from the transcript.
    Stop {
        #[serde(flatten)]
        common: AntigravityCommon,
    },
    /// Any `hookEventName` this crate does not model. The name itself is not
    /// kept, which is why [`AntigravityEvent::common`] has nothing to return
    /// for it.
    #[serde(other)]
    Unknown,
}

impl AntigravityEvent {
    /// The Antigravity event name this event was tagged with.
    pub fn event_name(&self) -> &'static str {
        match self {
            AntigravityEvent::SessionStart { .. } => "SessionStart",
            AntigravityEvent::PreInvocation { .. } => "PreInvocation",
            AntigravityEvent::PreToolUse { .. } => "PreToolUse",
            AntigravityEvent::Stop { .. } => "Stop",
            AntigravityEvent::Unknown => "Unknown",
        }
    }

    /// The fields shared by all modelled events, or `None` for any other.
    pub fn common(&self) -> Option<&AntigravityCommon> {
        match self {
            AntigravityEvent::SessionStart { common }
            | AntigravityEvent::PreInvocation { common, .. }
            | AntigravityEvent::PreToolUse { common, .. }
            | AntigravityEvent::Stop { common } => Some(common),
            AntigravityEvent::Unknown => None,
        }
    }
}

/// What a hook handler writes to stdout for Antigravity to act on.
///
/// Which keys Antigravity reads depends on the event. `injectSteps` is read at
/// `SessionStart` and `PreInvocation` and puts text in front of the model.
/// `decision` is read at `PreToolUse`, where it is required and says whether
/// the call runs, and at `Stop`, where `continue` makes the agent go on and any
/// other value lets it stop. `reason` explains a decision.
///
/// Every key is left out when it has nothing to say, so the answer to an event
/// that asks nothing is `{}`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AntigravityReply {
    /// Steps to add to the conversation before the model is called.
    #[serde(rename = "injectSteps", default, skip_serializing_if = "Vec::is_empty")]
    pub inject_steps: Vec<InjectedStep>,
    /// `allow` or `deny` at `PreToolUse`; at `Stop`, anything but `continue`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// Why the decision was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One step of an [`AntigravityReply`]: a system message, the only kind this
/// crate sends.
///
/// A system message stays in the conversation, so the model can still read it
/// turns later. Antigravity's other kind for context, the ephemeral message, is
/// shown for a single model call and then gone, which is not what a delivered
/// memory may be: in a session on 2026-11-20 the model reported the system
/// messages it had been sent and not the ephemeral one sent between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectedStep {
    /// Antigravity nests the text under the same key twice.
    #[serde(rename = "systemMessage")]
    pub system_message: SystemMessage,
}

/// The body of a system message step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemMessage {
    /// The text the model is shown.
    #[serde(rename = "systemMessage")]
    pub text: String,
}

impl AntigravityReply {
    /// An answer that asks Antigravity for nothing: `{}`.
    pub fn nothing() -> Self {
        AntigravityReply::default()
    }

    /// An answer that puts `text` in front of the model as a system message.
    pub fn system_message(text: impl Into<String>) -> Self {
        AntigravityReply {
            inject_steps: vec![InjectedStep {
                system_message: SystemMessage { text: text.into() },
            }],
            ..AntigravityReply::default()
        }
    }

    /// An answer that lets a tool call run under Antigravity's own permission
    /// rules: `allow` defers to them and approves nothing by itself.
    pub fn allow() -> Self {
        AntigravityReply {
            decision: Some("allow".to_string()),
            ..AntigravityReply::default()
        }
    }

    /// An answer that stops a tool call and says why.
    pub fn deny(reason: impl Into<String>) -> Self {
        AntigravityReply {
            decision: Some("deny".to_string()),
            reason: Some(reason.into()),
            ..AntigravityReply::default()
        }
    }

    /// An answer that lets the agent end its turn.
    pub fn let_stop() -> Self {
        AntigravityReply {
            decision: Some("stop".to_string()),
            ..AntigravityReply::default()
        }
    }

    /// The answer that leaves the event named `event_name` to run its course:
    /// the call runs, the turn ends, nothing is injected.
    ///
    /// It is what the client writes when it could not reach the server, so that
    /// a memory server that is down never stops the agent's tools, and what the
    /// server writes for an event it has nothing to say about.
    pub fn neutral(event_name: &str) -> Self {
        match event_name {
            "PreToolUse" => AntigravityReply::allow(),
            "Stop" => AntigravityReply::let_stop(),
            _ => AntigravityReply::nothing(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The fields every event of the conversation below carries, as Antigravity
    /// 1.5.12 sent them to a hook on 2026-11-20, with its identifiers and paths
    /// replaced.
    fn common() -> serde_json::Value {
        json!({
            "conversationId": "0b6f7c7e-1d2a-4f39-9c55-2a8e4d1f6b10",
            "cwd": "/home/dev/.frontend/attachments",
            "workspacePaths": ["/home/dev/.frontend/attachments", "/home/dev/widgets"],
            "transcriptPath": "/home/dev/.profile/brain/0b6f7c7e/.system_generated/logs/transcript_full.jsonl",
            "artifactDirectoryPath": "/home/dev/.profile/brain/0b6f7c7e",
            "modelName": "gemini-example",
            "lastUserInput": "tighten the widget tolerances"
        })
    }

    /// `common()` with the event's own fields added.
    fn payload(event: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut payload = common();
        let fields = payload.as_object_mut().expect("an object");
        fields.insert("hookEventName".to_string(), json!(event));
        for (name, value) in extra.as_object().expect("an object") {
            fields.insert(name.clone(), value.clone());
        }
        payload
    }

    // Detects a renamed field in the events this crate models, and a loss of
    // tolerance for the fields it does not: either makes every real event of
    // that name fail to parse or lose the session it belongs to.
    // Expectation source: the payloads Antigravity 1.5.12 delivered to a
    // logging hook on 2026-11-20.
    #[test]
    fn the_four_modelled_events_parse_from_the_fields_antigravity_sends() {
        let session_start: AntigravityEvent =
            serde_json::from_value(payload("SessionStart", json!({}))).expect("SessionStart");
        let common = session_start.common().expect("a modelled event");
        assert_eq!(
            common.conversation_id,
            "0b6f7c7e-1d2a-4f39-9c55-2a8e4d1f6b10"
        );
        assert_eq!(
            common.workspace_paths,
            vec!["/home/dev/.frontend/attachments", "/home/dev/widgets"]
        );
        assert_eq!(
            common.last_user_input.as_deref(),
            Some("tighten the widget tolerances")
        );

        let pre_invocation: AntigravityEvent = serde_json::from_value(payload(
            "PreInvocation",
            json!({ "invocationNum": 0, "initialNumSteps": 8 }),
        ))
        .expect("PreInvocation");
        assert!(matches!(
            pre_invocation,
            AntigravityEvent::PreInvocation {
                invocation_number: Some(0),
                ..
            }
        ));

        let pre_tool_use: AntigravityEvent = serde_json::from_value(payload(
            "PreToolUse",
            json!({
                "stepIdx": 4,
                "toolCall": {
                    "id": "call_1",
                    "name": "run_command",
                    "args": { "CommandLine": "ls", "WaitMsBeforeAsync": 5000 }
                }
            }),
        ))
        .expect("PreToolUse");
        let AntigravityEvent::PreToolUse { tool_call, .. } = pre_tool_use else {
            panic!("a PreToolUse payload must parse as the PreToolUse variant");
        };
        assert_eq!(tool_call.name, "run_command");
        assert_eq!(
            tool_call.args,
            json!({ "CommandLine": "ls", "WaitMsBeforeAsync": 5000 })
        );

        let stop: AntigravityEvent = serde_json::from_value(payload(
            "Stop",
            json!({
                "executionNum": 0,
                "terminationReason": "NO_TOOL_CALL",
                "fullyIdle": true,
                "continuationActive": false,
                "error": ""
            }),
        ))
        .expect("Stop");
        assert_eq!(stop.event_name(), "Stop");
    }

    // Detects the two events this crate leaves alone, or an event name added by
    // a newer Antigravity, being refused: the client would then fail on every
    // such event instead of leaving it be.
    #[test]
    fn every_other_event_parses_as_unknown_instead_of_failing() {
        for payload in [
            payload(
                "PostToolUse",
                json!({ "toolCall": { "name": "run_command" }, "error": "" }),
            ),
            payload("PostInvocation", json!({ "invocationNum": 0 })),
            payload("SomeEventAddedLater", json!({})),
        ] {
            let event: AntigravityEvent =
                serde_json::from_value(payload).expect("no event may fail to parse");
            assert_eq!(event, AntigravityEvent::Unknown);
            assert!(event.common().is_none());
        }
    }

    // Detects a reply whose keys Antigravity does not read: it ignores a key it
    // does not know, so a misspelled or differently nested key delivers nothing
    // and reports nothing.
    // Expectation source: the output examples of Antigravity's hooks
    // documentation, and the replies a logging hook sent on 2026-11-20, which
    // Antigravity acted on.
    #[test]
    fn replies_serialise_to_the_keys_antigravity_reads() {
        assert_eq!(
            serde_json::to_value(AntigravityReply::system_message("the memory body"))
                .expect("a reply serialises"),
            json!({ "injectSteps": [{ "systemMessage": { "systemMessage": "the memory body" } }] })
        );
        assert_eq!(
            serde_json::to_value(AntigravityReply::deny("the reason")).expect("a reply serialises"),
            json!({ "decision": "deny", "reason": "the reason" })
        );
        assert_eq!(
            serde_json::to_value(AntigravityReply::nothing()).expect("a reply serialises"),
            json!({})
        );
    }

    // Detects a neutral reply that stops or changes anything: it is what is
    // written when the server cannot be reached, so a `PreToolUse` without a
    // decision, which Antigravity requires, or a `Stop` answered `continue`,
    // which makes the agent go on, would turn an outage into a stuck agent.
    #[test]
    fn the_neutral_reply_lets_each_event_run_its_course() {
        assert_eq!(
            serde_json::to_value(AntigravityReply::neutral("PreToolUse")).expect("serialises"),
            json!({ "decision": "allow" })
        );
        let stop = AntigravityReply::neutral("Stop");
        assert!(stop.decision.is_some_and(|decision| decision != "continue"));
        for event in ["SessionStart", "PreInvocation", "PostToolUse", "Unknown"] {
            assert_eq!(
                AntigravityReply::neutral(event),
                AntigravityReply::nothing()
            );
        }
    }
}
