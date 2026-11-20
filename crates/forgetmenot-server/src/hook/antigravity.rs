//! What one Antigravity hook event means for a context, and how the answer to
//! it is written.
//!
//! This is the whole of the mapping from Antigravity's events to a plan and
//! from an answer to Antigravity's reply, kept apart from the HTTP handler so
//! that a change to Antigravity's payloads is a change to one file.
//!
//! Antigravity's events are not Claude Code's under other names, and three
//! differences shape this file:
//!
//! * No event reports a submitted prompt. `PreInvocation` fires before every
//!   model call and numbers the calls of a turn from 0, so the call numbered 0
//!   is where a prompt is read.
//! * Text reaches the model through the answers to `SessionStart` and
//!   `PreInvocation` alone. The answer to `PreToolUse` says whether the call
//!   runs and carries no text, so a call held for a pinned memory is refused
//!   with a reason, and the memory itself arrives at the `PreInvocation` that
//!   follows the refusal.
//! * No event carries what a tool returned, and no event says that the
//!   working directory changed or that a subagent started.

use forgetmenot_types::antigravity::{AntigravityEvent, AntigravityReply, AntigravityToolCall};
use forgetmenot_types::hook::Harness;

use super::plan::{Answer, EventPlan, listed_exempt, push_text, string_leaves};
use crate::context::ContextKey;
use crate::mcp::FORGETMENOT_TOOL_NAMES;
use crate::store::scope::TriggerField;
use crate::store::settings::Settings;

/// Why a tool call was stopped. Fixed text: the model has to be able to tell
/// this apart from a human refusing the call. It differs from Claude Code's in
/// where it points: here the memory is not in this answer.
pub const DENY_REASON: &str = "This tool call was not executed. Its input matched a memory trigger and a pinned memory is new or changed for this context. This is an automatic keyword match, not a review, approval or denial of the call. The memory arrives in a system message before your next step. Read it, then issue the call again if it is still what you intend.";

/// The name every call of an MCP server's tool arrives under. The server and
/// the tool are in the call's arguments.
const MCP_TOOL_CALL: &str = "call_mcp_tool";

/// The argument of [`MCP_TOOL_CALL`] that names the tool.
const MCP_TOOL_NAME_ARGUMENT: &str = "ToolName";

/// The characters of a first prompt that are kept to say what a session is,
/// the same number the client keeps of a Claude Code session's.
const PROMPT_CHARACTERS: usize = 200;

/// What to do about `event`, or `None` for an event this server does nothing
/// at, which is answered with an empty object.
///
/// `last_assistant_message` is what the request said the model's last message
/// was: `Stop` does not carry it, so it is the client's read of the transcript
/// or nothing.
///
/// `settings` are the store's, because which tools are kept away from the
/// triggers is part of what the store says about its own behaviour.
pub fn plan(
    event: &AntigravityEvent,
    machine: &str,
    last_assistant_message: Option<&str>,
    settings: &Settings,
) -> Option<EventPlan> {
    let common = event.common()?;
    let directories = &common.workspace_paths;
    let mut plan = EventPlan {
        // Antigravity names a conversation and nothing inside it, so every
        // event of a conversation belongs to the one context.
        key: ContextKey::main(machine, &common.conversation_id),
        harness: Harness::Antigravity,
        event_name: event.event_name(),
        texts: Vec::new(),
        delivers: false,
        may_deny: false,
        tool_name: None,
        session_start: false,
        cwd: (!directories.is_empty()).then(|| directories.join("\n")),
        matches_directories: false,
        task: None,
        first_prompt: None,
        agent_type: None,
        parent: None,
        tokens_are_the_parents: false,
    };

    match event {
        AntigravityEvent::SessionStart { .. } => {
            plan.session_start = true;
            plan.delivers = true;
            plan.matches_directories = true;
            push_directories(&mut plan, directories);
            // The prompt that opened the conversation is matched at the
            // `PreInvocation` that follows at once; here it only names the
            // session.
            plan.first_prompt = common
                .last_user_input
                .as_deref()
                .map(typed)
                .filter(|prompt| !prompt.is_empty())
                .map(|prompt| prompt.chars().take(PROMPT_CHARACTERS).collect());
        }
        AntigravityEvent::PreInvocation {
            invocation_number, ..
        } => {
            plan.delivers = true;
            // A turn's later model calls repeat the prompt that began it, and
            // matching it again would restart every count a trigger starts.
            if *invocation_number == Some(0) {
                let prompt = common.last_user_input.as_deref().map(typed);
                push_text(&mut plan, TriggerField::UserMessage, prompt);
            }
        }
        AntigravityEvent::PreToolUse { tool_call, .. } => {
            plan.may_deny = true;
            plan.matches_directories = true;
            plan.tool_name = Some(tool_call.name.clone());
            if !triggers_exempt(settings, tool_call) {
                push_text(&mut plan, TriggerField::ToolName, Some(&tool_call.name));
                let input = string_leaves(&tool_call.args, usize::MAX);
                push_text(&mut plan, TriggerField::ToolInput, Some(&input));
            }
            push_directories(&mut plan, directories);
        }
        AntigravityEvent::Stop { .. } => {
            push_text(
                &mut plan,
                TriggerField::AssistantMessage,
                last_assistant_message,
            );
        }
        AntigravityEvent::Unknown => return None,
    }
    Some(plan)
}

/// Whether the name and the input of `tool_call` are kept out of the texts the
/// triggers are matched against.
///
/// Two kinds of call are exempt: a call of one of this server's own MCP tools,
/// whose inputs carry scope ids, memory ids, session keys and whole memory
/// bodies, so matching them activates scopes from the memory system's traffic
/// rather than from the work; and a call of whatever the store lists in
/// `trigger_exempt_tools`. The directories a tool call reports are matched
/// either way.
pub fn triggers_exempt(settings: &Settings, tool_call: &AntigravityToolCall) -> bool {
    is_forgetmenot_tool(tool_call) || listed_exempt(settings, &tool_call.name)
}

/// Whether `tool_call` calls one of this server's own MCP tools. The name the
/// server is registered under is the user's to choose, so any server counts
/// and the tool's name decides.
fn is_forgetmenot_tool(tool_call: &AntigravityToolCall) -> bool {
    tool_call.name == MCP_TOOL_CALL
        && tool_call
            .args
            .get(MCP_TOOL_NAME_ARGUMENT)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|tool| FORGETMENOT_TOOL_NAMES.contains(&tool))
}

/// Match each workspace directory against the shell-directory triggers. A
/// conversation has no single directory, and the one that is the project is
/// not marked, so every one of them counts.
fn push_directories(plan: &mut EventPlan, directories: &[String]) {
    for directory in directories {
        push_text(plan, TriggerField::ShellDirectory, Some(directory));
    }
}

/// The opening of the block the T3 Code frontend puts in front of the first
/// prompt of a thread: its own instructions to the model, some 5,000
/// characters of them.
const T3_CODE_INSTRUCTIONS_OPEN: &str = "<t3_code_instructions>";

/// The end of that block.
const T3_CODE_INSTRUCTIONS_CLOSE: &str = "</t3_code_instructions>";

/// The tags T3 Code puts around what the user typed, after its instructions.
const T3_CODE_REQUEST_OPEN: &str = "<user_request>";
const T3_CODE_REQUEST_CLOSE: &str = "</user_request>";

/// The part of `input` the user typed.
///
/// Antigravity reports a prompt as the frontend sent it, and T3 Code sends the
/// first prompt of a thread as its own instructions, then the user's text in
/// `<user_request>` tags, then a note on the runtime. Matched whole, that
/// prompt would fire every trigger a word of those instructions happens to
/// match, in every thread. An input that does not open with T3 Code's block is
/// the user's as it stands, and so is one whose block is not followed by the
/// tags: text is never dropped on a guess.
pub fn typed(input: &str) -> &str {
    let Some(after_open) = input.trim_start().strip_prefix(T3_CODE_INSTRUCTIONS_OPEN) else {
        return input;
    };
    let Some((_, after_instructions)) = after_open.split_once(T3_CODE_INSTRUCTIONS_CLOSE) else {
        return input;
    };
    let Some((_, after_request_open)) = after_instructions.split_once(T3_CODE_REQUEST_OPEN) else {
        return input;
    };
    // The user's own text may contain the closing tag, so the last one closes.
    match after_request_open.rsplit_once(T3_CODE_REQUEST_CLOSE) {
        Some((request, _)) => request.trim(),
        None => input,
    }
}

/// The reply Antigravity is sent for the event of `plan`.
///
/// It starts from the reply that lets the event run its course, because
/// `PreToolUse` requires a decision whatever the answer. A held call is refused
/// with the fixed reason; text goes in as a system message, which only the
/// events that deliver are ever given.
pub fn reply(plan: &EventPlan, answer: Answer) -> AntigravityReply {
    if answer.hold {
        return AntigravityReply::deny(DENY_REASON);
    }
    match answer.text {
        Some(text) => AntigravityReply::system_message(text),
        None => AntigravityReply::neutral(plan.event_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CONVERSATION: &str = "0b6f7c7e-1d2a-4f39-9c55-2a8e4d1f6b10";
    const ATTACHMENTS: &str = "/home/dev/.frontend/attachments";
    const PROJECT: &str = "/home/dev/widgets";

    /// An event of the conversation above, with `extra` beside the common
    /// fields and `prompt` as the latest thing the user sent.
    fn event(name: &str, prompt: &str, extra: serde_json::Value) -> AntigravityEvent {
        let mut payload = json!({
            "hookEventName": name,
            "conversationId": CONVERSATION,
            "cwd": ATTACHMENTS,
            "workspacePaths": [ATTACHMENTS, PROJECT],
            "transcriptPath": "/home/dev/.profile/brain/transcript_full.jsonl",
            "modelName": "gemini-example",
            "lastUserInput": prompt
        });
        let fields = payload.as_object_mut().expect("an object");
        for (key, value) in extra.as_object().expect("an object") {
            fields.insert(key.clone(), value.clone());
        }
        serde_json::from_value(payload).expect("the payload parses as an event")
    }

    /// The plan for `event` on machine `alpha`, under a store that has said
    /// nothing about its behaviour.
    fn plan_of(event: &AntigravityEvent) -> Option<EventPlan> {
        plan(event, "alpha", None, &Settings::default())
    }

    /// The texts of `plan` that the triggers of `field` are matched against.
    fn texts_on(plan: &EventPlan, field: TriggerField) -> Vec<&str> {
        plan.texts
            .iter()
            .filter(|(each, _)| *each == field)
            .map(|(_, text)| text.as_str())
            .collect()
    }

    fn tool_call(name: &str, args: serde_json::Value) -> AntigravityEvent {
        event(
            "PreToolUse",
            "tighten the tolerances",
            json!({ "stepIdx": 4, "toolCall": { "id": "call_1", "name": name, "args": args } }),
        )
    }

    /// Detects a prompt matched at every model call of a turn, and one never
    /// matched: Antigravity repeats the prompt in every event, so the first
    /// would restart a scope's forgetting count dozens of times a turn, and the
    /// second would leave a session with no user-message trigger at all.
    #[test]
    fn a_prompt_is_matched_at_the_first_model_call_of_a_turn_and_at_no_other() {
        let first = plan_of(&event(
            "PreInvocation",
            "tighten the tolerances",
            json!({ "invocationNum": 0, "initialNumSteps": 8 }),
        ))
        .expect("PreInvocation is planned");
        assert_eq!(
            texts_on(&first, TriggerField::UserMessage),
            vec!["tighten the tolerances"]
        );
        assert!(
            first.delivers,
            "a model call is where text can be delivered"
        );

        let later = plan_of(&event(
            "PreInvocation",
            "tighten the tolerances",
            json!({ "invocationNum": 3, "initialNumSteps": 14 }),
        ))
        .expect("PreInvocation is planned");
        assert!(later.texts.is_empty(), "got {:?}", later.texts);
        assert!(
            later.delivers,
            "what a tool call made due is delivered here"
        );
    }

    /// Detects the frontend's own instructions matched as the user's words:
    /// T3 Code puts some 5,000 characters of them in front of the first prompt
    /// of every thread, so a trigger on a word they contain would fire in every
    /// thread. Also detects text dropped from a prompt that only looks wrapped.
    ///
    /// Source: the first prompt T3 Code 0.1.10 sent Antigravity on 2026-11-20
    /// was `<t3_code_instructions>…</t3_code_instructions>`, then the text in
    /// `<user_request>` tags, then a `<runtime_info>` block.
    #[test]
    fn only_what_the_user_typed_is_read_from_a_prompt_t3_code_wrapped() {
        let wrapped = "<t3_code_instructions>\n## T3 Code interaction mode: Default\n\
                       Prefer worktrees.\n</t3_code_instructions>\n\
                       <user_request>\ntighten the tolerances\n</user_request>\n\
                       <runtime_info>Link files by absolute path.</runtime_info>";
        assert_eq!(typed(wrapped), "tighten the tolerances");

        for own in [
            "tighten the tolerances",
            "what does <user_request> mean in the logs?",
            "<t3_code_instructions> is a tag I saw; what is it?",
            "<t3_code_instructions>cut off before the request",
        ] {
            assert_eq!(
                typed(own),
                own,
                "an unwrapped prompt is the user's whole text"
            );
        }

        let start = plan_of(&event("SessionStart", wrapped, json!({}))).expect("planned");
        assert_eq!(
            start.first_prompt.as_deref(),
            Some("tighten the tolerances")
        );
    }

    /// Detects a session start that does not begin the session, or that matches
    /// only the first workspace directory: a frontend's own directory can sort
    /// before the project, and then no directory trigger would ever see the
    /// project.
    #[test]
    fn a_session_start_begins_the_session_and_matches_every_workspace_directory() {
        let start =
            plan_of(&event("SessionStart", "tighten the tolerances", json!({}))).expect("planned");

        assert!(start.session_start && start.delivers && start.matches_directories);
        assert_eq!(start.key, ContextKey::main("alpha", CONVERSATION));
        assert_eq!(
            texts_on(&start, TriggerField::ShellDirectory),
            vec![ATTACHMENTS, PROJECT]
        );
        assert_eq!(
            start.cwd.as_deref(),
            Some(format!("{ATTACHMENTS}\n{PROJECT}").as_str()),
            "the session's directories are all of them, one per line"
        );
        assert!(
            texts_on(&start, TriggerField::UserMessage).is_empty(),
            "the prompt is matched at the model call that follows, not twice"
        );
    }

    /// Detects a tool call whose command is not matched, one that claims it can
    /// deliver, and one that may not be held: the first loses every tool-input
    /// trigger, the second records as delivered a memory the model was never
    /// shown, since Antigravity reads no text from this answer.
    #[test]
    fn a_tool_call_is_matched_and_may_be_held_but_delivers_nothing() {
        let call = plan_of(&tool_call(
            "run_command",
            json!({ "CommandLine": "cargo test -p widgets", "WaitMsBeforeAsync": 5000 }),
        ))
        .expect("PreToolUse is planned");

        assert!(call.may_deny && !call.delivers);
        assert_eq!(call.tool_name.as_deref(), Some("run_command"));
        assert_eq!(texts_on(&call, TriggerField::ToolName), vec!["run_command"]);
        assert_eq!(
            texts_on(&call, TriggerField::ToolInput),
            vec!["cargo test -p widgets"]
        );
        assert_eq!(
            texts_on(&call, TriggerField::ShellDirectory),
            vec![ATTACHMENTS, PROJECT]
        );
    }

    /// Detects this server's own MCP tools being matched against triggers: a
    /// `memory_put` carries a whole memory body, so matching it activates
    /// scopes from the memory system's own traffic rather than from the work.
    /// Another server's tool, and a tool the store lists, are told apart too.
    ///
    /// Source: Antigravity 1.5.12 reported a call of this server's
    /// `memory_get` as `call_mcp_tool` with `ServerName`, `ToolName` and
    /// `Arguments`, on 2026-11-20.
    #[test]
    fn the_stores_own_tools_contribute_no_text_whatever_server_name_they_carry() {
        for server in ["forgetmenot", "memory"] {
            let own = plan_of(&tool_call(
                "call_mcp_tool",
                json!({
                    "ServerName": server,
                    "ToolName": "memory_put",
                    "Arguments": { "id": "widget-naming", "body": "Name widgets by function." }
                }),
            ))
            .expect("PreToolUse is planned");
            assert!(
                texts_on(&own, TriggerField::ToolInput).is_empty()
                    && texts_on(&own, TriggerField::ToolName).is_empty(),
                "got {:?}",
                own.texts
            );
            assert_eq!(texts_on(&own, TriggerField::ShellDirectory).len(), 2);
        }

        let other = plan_of(&tool_call(
            "call_mcp_tool",
            json!({ "ServerName": "tracker", "ToolName": "create_issue", "Arguments": { "title": "widgets" } }),
        ))
        .expect("PreToolUse is planned");
        assert_eq!(
            texts_on(&other, TriggerField::ToolInput),
            vec!["tracker\ncreate_issue\nwidgets"]
        );

        let settings = Settings {
            trigger_exempt_tools: vec!["read_file".to_string()],
            ..Settings::default()
        };
        let listed = plan(
            &tool_call("read_file", json!({ "Path": "/home/dev/widgets/notes.md" })),
            "alpha",
            None,
            &settings,
        )
        .expect("PreToolUse is planned");
        assert!(texts_on(&listed, TriggerField::ToolInput).is_empty());
    }

    /// Detects the end of a turn matching nothing, and claiming to deliver:
    /// the event carries no message, so the text comes with the request, and
    /// its answer can put nothing in front of the model.
    #[test]
    fn the_end_of_a_turn_matches_the_message_the_client_read() {
        let stop = event(
            "Stop",
            "tighten the tolerances",
            json!({ "executionNum": 0, "terminationReason": "NO_TOOL_CALL", "fullyIdle": true }),
        );

        let with_message = plan(
            &stop,
            "alpha",
            Some("The widgets are tighter now."),
            &Settings::default(),
        )
        .expect("Stop is planned");
        assert_eq!(
            texts_on(&with_message, TriggerField::AssistantMessage),
            vec!["The widgets are tighter now."]
        );
        assert!(!with_message.delivers && !with_message.may_deny);

        let without = plan_of(&stop).expect("Stop is planned");
        assert!(without.texts.is_empty());
    }

    /// Detects an event this server does nothing at being planned, which would
    /// create a context for it and answer it as if something had been decided.
    #[test]
    fn the_events_nothing_is_done_at_are_not_planned() {
        for name in ["PostToolUse", "PostInvocation", "SomeEventAddedLater"] {
            assert_eq!(
                plan_of(&event(name, "tighten the tolerances", json!({}))),
                None
            );
        }
    }

    /// Detects a reply that leaves a tool call without a decision, a held call
    /// that runs, and text sent where Antigravity reads none: Antigravity
    /// requires a decision at every `PreToolUse`, and ignores the keys it does
    /// not read for the event.
    #[test]
    fn each_event_is_answered_in_the_keys_antigravity_reads_for_it() {
        let call = plan_of(&tool_call("run_command", json!({ "CommandLine": "ls" })))
            .expect("PreToolUse is planned");
        assert_eq!(
            reply(
                &call,
                Answer {
                    text: None,
                    hold: false
                }
            ),
            AntigravityReply::allow()
        );
        assert_eq!(
            reply(
                &call,
                Answer {
                    text: None,
                    hold: true
                }
            ),
            AntigravityReply::deny(DENY_REASON)
        );

        let model_call = plan_of(&event(
            "PreInvocation",
            "tighten the tolerances",
            json!({ "invocationNum": 0 }),
        ))
        .expect("PreInvocation is planned");
        assert_eq!(
            reply(
                &model_call,
                Answer {
                    text: Some("the memory".to_string()),
                    hold: false
                }
            ),
            AntigravityReply::system_message("the memory")
        );
        assert_eq!(
            reply(
                &model_call,
                Answer {
                    text: None,
                    hold: false
                }
            ),
            AntigravityReply::nothing()
        );

        let stop = plan_of(&event("Stop", "tighten the tolerances", json!({}))).expect("planned");
        assert_eq!(
            reply(
                &stop,
                Answer {
                    text: None,
                    hold: false
                }
            ),
            AntigravityReply::let_stop()
        );
    }
}
