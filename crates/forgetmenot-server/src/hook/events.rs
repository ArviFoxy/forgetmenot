//! What one hook event means for a context: which context it belongs to, which
//! texts its triggers are matched against, and what its answer may do.
//!
//! This is the whole of the event-to-text mapping, kept apart from the HTTP
//! handler so that a change to Claude Code's payloads is a change to one file.

use forgetmenot_types::hook::{HookCommon, HookEvent};

use crate::context::{ContextKey, MAIN_AGENT};
use crate::mcp::FORGETMENOT_TOOL_NAMES;
use crate::render::ANSWER_MARKER;
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
    /// Whether the answer also tells the model which session key to pass to
    /// the MCP tools.
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

/// What to do about `event`, or `None` for an event name this server does not
/// know, which is answered with an empty object.
///
/// `task` is what the request said the subagent this event comes from was asked
/// to do and `parent_agent_id` is the subagent the request said spawned it; no
/// hook event carries either, so both are the client's read of the subagent's
/// metadata file or nothing.
///
/// `settings` are the store's, because how much of a tool result is matched
/// against triggers is part of what the store says about its own behaviour.
pub fn plan(
    event: &HookEvent,
    machine: &str,
    task: Option<&str>,
    parent_agent_id: Option<&str>,
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
        parent: None,
        tokens_are_the_parents: false,
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
            if !triggers_exempt(settings, tool_name) {
                push_text(&mut plan, TriggerField::ToolName, Some(tool_name));
                let input = string_leaves(tool_input, usize::MAX);
                push_text(&mut plan, TriggerField::ToolInput, Some(&input));
            }
            push_text(
                &mut plan,
                TriggerField::ShellDirectory,
                common.cwd.as_deref(),
            );
        }
        HookEvent::PostToolUse {
            tool_name,
            tool_output,
            ..
        } => {
            if !triggers_exempt(settings, tool_name) {
                let result = string_leaves(tool_output, settings.tool_result_cap());
                if !carries_own_answer(&result) {
                    push_text(&mut plan, TriggerField::ToolResult, Some(&result));
                }
            }
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
            // whose id is the variant's own field. The size it reports is the
            // parent's for the same reason: it was read from the transcript of
            // the session the event arrived on.
            plan.key = ContextKey::subagent(machine, &common.session_id, agent_id);
            plan.tokens_are_the_parents = true;
            plan.agent_type = agent_type.clone();
            // A subagent may spawn a subagent, and the event says only which
            // session it arrived on, so the spawner is the agent the request
            // named and the session when it named none.
            plan.parent = Some(match parent_agent_id.filter(|id| !id.is_empty()) {
                Some(parent) => ContextKey::subagent(machine, &common.session_id, parent),
                None => ContextKey::main(machine, &common.session_id),
            });
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

/// Whether `text` is this server's own answer handed back through a tool.
///
/// An answer past the store's threshold is written to a file by Claude Code,
/// which shows the model a preview and the path; the notice at the top of such
/// an answer tells the model to read that file, so the whole answer comes back
/// as the result of a `Read` — every scope id of the available list and the
/// text of every critical memory delivered. Matched against the tool-result
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

/// Whether the name, the input and the result of `tool_name` are kept out of the
/// texts the triggers are matched against.
///
/// Two tools are exempt: this server's own MCP tools, whose inputs and answers
/// carry scope ids, memory ids, session keys and whole memory bodies, so
/// matching them activates scopes from the memory system's traffic rather than
/// from the work; and whatever the store lists in `trigger_exempt_tools`. The
/// directories a tool call reports are matched either way, because a directory
/// says where the session is working whatever tool named it.
pub fn triggers_exempt(settings: &Settings, tool_name: &str) -> bool {
    is_forgetmenot_tool(tool_name)
        || settings
            .trigger_exempt_tools
            .iter()
            .any(|exempt| exempt == tool_name)
}

/// Whether `tool_name` is one of this server's own MCP tools as a client names
/// it: `mcp__`, the name the user registered this server under, `__`, and the
/// tool's own name. The server name is the user's to choose, so any non-empty
/// one counts; the tool name is taken as the last segment, since no tool of this
/// server has `__` in its name. A bare tool name with no `mcp__` prefix is some
/// other tool that happens to share a name.
fn is_forgetmenot_tool(tool_name: &str) -> bool {
    let Some(rest) = tool_name.strip_prefix("mcp__") else {
        return false;
    };
    let Some((server, tool)) = rest.rsplit_once("__") else {
        return false;
    };
    !server.is_empty() && FORGETMENOT_TOOL_NAMES.contains(&tool)
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
    /// about its behaviour and a request that read no task and no spawner.
    fn plan_of(event: &HookEvent, machine: &str) -> Option<EventPlan> {
        plan(event, machine, None, None, &Settings::default())
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

    /// A `PostToolUse` from `tool` whose result is `output`.
    fn tool_result(tool: &str, output: &str) -> HookEvent {
        event(json!({
            "hook_event_name": "PostToolUse",
            "session_id": "session-1",
            "cwd": "/home/dev/widgets",
            "tool_name": tool,
            "tool_input": { "command": "cargo test" },
            "tool_output": output
        }))
    }

    /// The texts of `plan` that the triggers of `field` are matched against.
    fn texts_on(plan: &EventPlan, field: TriggerField) -> Vec<&str> {
        plan.texts
            .iter()
            .filter(|(each, _)| *each == field)
            .map(|(_, text)| text.as_str())
            .collect()
    }

    /// Detects this server's own MCP tools being matched against triggers, and
    /// an exemption tied to one server name or blind to the `mcp__` form: a
    /// `memory_put` carries a whole memory body and a `session_scopes` answer
    /// names every scope the store has, so matching either activates scopes
    /// from the memory system's own traffic rather than from the work. The name
    /// the server is registered under is the user's to choose, while a bare
    /// tool name with no prefix is some other tool that happens to share it.
    ///
    /// Source: the `mcp__<server>__<tool>` naming Claude Code gives an MCP
    /// tool.
    #[test]
    fn the_stores_own_tools_contribute_no_text_whatever_server_name_they_carry() {
        let scopes = "active:\n- widgets\navailable:\n- rocketry\n";
        for tool in [
            "mcp__forgetmenot__session_scopes",
            "mcp__memory__session_scopes",
        ] {
            let plan = plan_of(&tool_result(tool, scopes), "alpha").expect("PostToolUse is known");
            assert!(
                plan.texts.is_empty(),
                "{tool} is this server's own tool, so nothing it said is matched, got {:?}",
                plan.texts
            );
        }

        for tool in ["session_scopes", "Bash"] {
            let plan = plan_of(&tool_result(tool, scopes), "alpha").expect("PostToolUse is known");
            assert_eq!(
                texts_on(&plan, TriggerField::ToolResult),
                vec![scopes],
                "{tool} is not this server's tool, so its result is matched like any other"
            );
        }

        let write = plan_of(
            &event(json!({
                "hook_event_name": "PreToolUse",
                "session_id": "session-1",
                "cwd": "/home/dev/widgets",
                "tool_name": "mcp__forgetmenot__memory_put",
                "tool_input": { "id": "widget-naming", "body": "the widgets rule" }
            })),
            "alpha",
        )
        .expect("PreToolUse is known");
        assert!(
            texts_on(&write, TriggerField::ToolInput).is_empty()
                && texts_on(&write, TriggerField::ToolName).is_empty(),
            "a call into the store must not be matched against what it is writing, got {:?}",
            write.texts
        );
    }

    /// Detects a `trigger_exempt_tools` list that is not consulted, or one that
    /// exempts every tool: a store that names a tool wants what that tool reads
    /// kept out of the matching, and a tool it did not name must still be
    /// matched.
    #[test]
    fn a_tool_the_store_exempts_contributes_no_text_while_another_still_does() {
        let settings = Settings {
            trigger_exempt_tools: vec!["Read".to_string()],
            ..Settings::default()
        };
        let output = "the widgets rule is in the binder";

        let exempt = plan(&tool_result("Read", output), "alpha", None, None, &settings)
            .expect("PostToolUse is known");
        let matched = plan(&tool_result("Grep", output), "alpha", None, None, &settings)
            .expect("PostToolUse is known");

        assert!(
            exempt.texts.is_empty(),
            "the store named Read, so what it read is kept out, got {:?}",
            exempt.texts
        );
        assert_eq!(
            texts_on(&matched, TriggerField::ToolResult),
            vec![output],
            "a tool the store did not name must still be matched"
        );
    }

    /// Detects the server's own answer, read back out of the file Claude Code
    /// saved it to, being matched against the tool-result triggers: the answer
    /// lists every scope on offer and prints every critical memory delivered,
    /// so one such result activates every scope the store has at once. Detects
    /// too a check that fires on any mention of this server, which would silence
    /// the work's own results whenever a log line said `forgetmenot`.
    ///
    /// The marker arrives the way a `Read` prints it: numbered, indented and
    /// behind the notice that told the model to read the file.
    #[test]
    fn the_answer_read_back_through_a_tool_contributes_no_text_while_other_results_still_do() {
        let answer = "     1\t[forgetmenot] READ THE FILE FIRST. This hook answer is long.\n     \
                      2\t\n     3\t[forgetmenot] context alpha/session-1\n     4\t== scope: \
                      rocketry ==\n     5\t-- critical: rocket-stages --\n";
        let read_back =
            plan_of(&tool_result("Read", answer), "alpha").expect("PostToolUse is known");
        assert!(
            read_back.texts.is_empty(),
            "the answer handed back through a tool is the store's own traffic, got {:?}",
            read_back.texts
        );

        let mention = "forgetmenot: the rocket stages are in the binder";
        let work = plan_of(&tool_result("Read", mention), "alpha").expect("PostToolUse is known");
        assert_eq!(
            texts_on(&work, TriggerField::ToolResult),
            vec![mention],
            "a result that names this server without carrying its answer is matched like any \
             other"
        );
    }

    /// Detects a tool result matched past the limit the store set, and a limit
    /// that throws the result away before it: matching is linear in the text,
    /// so a limit that is not applied makes a large tool result cost a regex
    /// pass over all of it, and one applied too soon silently stops triggers
    /// firing on results the store meant to match.
    #[test]
    fn a_tool_result_is_matched_only_up_to_the_limit_the_store_set() {
        let limit = 64;
        let settings = Settings {
            tool_result_match_limit: limit,
            ..Settings::default()
        };
        let output = "SUPERNOVA in the log. ".repeat(20);

        let plan = plan(
            &tool_result("Bash", &output),
            "alpha",
            None,
            None,
            &settings,
        )
        .expect("PostToolUse is known");

        let matched = texts_on(&plan, TriggerField::ToolResult);
        assert_eq!(matched.len(), 1, "one result is one text, got {matched:?}");
        assert_eq!(
            matched[0].len() as u64,
            limit,
            "the matched text must stop at the limit, got {:?}",
            matched[0]
        );
        assert!(
            output.starts_with(matched[0]),
            "what is matched must be the start of the result, got {:?}",
            matched[0]
        );
    }

    /// Detects a plan that gives the shell's directory as anything but the
    /// directory the event reports, and one that invents a session directory of
    /// its own: where the session began is the context's to say and reaches the
    /// triggers only once the context has been read, so a plan that carried one
    /// would match a directory no event named.
    #[test]
    fn the_shell_directory_a_plan_matches_is_the_one_its_event_reports() {
        for payload in [
            json!({
                "hook_event_name": "SessionStart",
                "session_id": "session-1",
                "cwd": "/start/project",
                "source": "startup"
            }),
            json!({
                "hook_event_name": "PreToolUse",
                "session_id": "session-1",
                "cwd": "/start/project",
                "tool_name": "Read",
                "tool_input": { "file_path": "/home/dev/notes/README.md" }
            }),
        ] {
            let plan = plan_of(&event(payload), "alpha").expect("the event is known");
            assert_eq!(
                texts_on(&plan, TriggerField::ShellDirectory),
                vec!["/start/project"],
                "the shell's directory is the one the event reports, got {:?}",
                plan.texts
            );
            assert!(
                texts_on(&plan, TriggerField::SessionDirectory).is_empty(),
                "the directory the session began in is not the plan's to name, got {:?}",
                plan.texts
            );
        }
    }

    /// Detects an event other than a tool call planned as able to stop one: a
    /// prompt or a finished call has nothing to stop, and answering one with a
    /// permission decision would have Claude Code reject the answer.
    #[test]
    fn only_a_tool_call_about_to_run_may_be_stopped() {
        let call = plan_of(
            &event(json!({
                "hook_event_name": "PreToolUse",
                "session_id": "session-1",
                "tool_name": "Bash",
                "tool_input": { "command": "cargo test" }
            })),
            "alpha",
        )
        .expect("PreToolUse is known");
        assert!(call.may_deny, "a call about to run can still be stopped");
        assert_eq!(
            call.tool_name.as_deref(),
            Some("Bash"),
            "the tool the store's exemptions are matched against must be named"
        );

        for payload in [
            json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "session-1",
                "prompt": "rename the widget brackets"
            }),
            json!({
                "hook_event_name": "SessionStart",
                "session_id": "session-1",
                "source": "startup"
            }),
            json!({
                "hook_event_name": "Stop",
                "session_id": "session-1",
                "last_assistant_message": "done"
            }),
        ] {
            let plan = plan_of(&event(payload), "alpha").expect("the event is known");
            assert!(
                !plan.may_deny && plan.tool_name.is_none(),
                "a {} has no call to stop, got {plan:?}",
                plan.event_name
            );
        }

        let finished = plan_of(&tool_result("Bash", "ok"), "alpha").expect("PostToolUse is known");
        assert!(
            !finished.may_deny,
            "a call that has already run cannot be stopped, got {finished:?}"
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
            None,
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

    /// Detects a `SubagentStart` planned as spawned by the session when another
    /// subagent spawned it: the new context would inherit the session's scopes
    /// instead of its spawner's, so a scope the spawner turned on for the work
    /// it is delegating would not reach the agent doing it. Detects too a plan
    /// that takes the spawner from nothing, which would put every subagent of a
    /// session under a context that never spawned it.
    ///
    /// The event is the same either way: it arrives on the session and names the
    /// child alone. What separates the two is `parentAgentId` in the child's
    /// metadata file, which Claude Code writes only for a subagent another
    /// subagent spawned, so the request carries it only then.
    #[test]
    fn a_subagent_start_is_spawned_by_the_agent_the_request_names_and_by_the_session_otherwise() {
        let event = event(json!({
            "hook_event_name": "SubagentStart",
            "session_id": "session-1",
            "agent_id": "agent-7",
            "agent_type": "general-purpose"
        }));

        let nested = plan(
            &event,
            "alpha",
            Some("survey the widgets crate"),
            Some("agent-3"),
            &Settings::default(),
        )
        .expect("SubagentStart is a known event");
        assert_eq!(
            nested.parent,
            Some(ContextKey::subagent("alpha", "session-1", "agent-3")),
            "the agent the request named is the context this subagent was spawned by"
        );

        let top_level = plan(
            &event,
            "alpha",
            Some("survey the widgets crate"),
            None,
            &Settings::default(),
        )
        .expect("SubagentStart is a known event");
        assert_eq!(
            top_level.parent,
            Some(ContextKey::main("alpha", "session-1")),
            "a request that names no agent is a subagent the session itself spawned"
        );
    }

    /// Detects an event other than a `SubagentStart` planned as spawning a
    /// context: every other event acts on a context that already exists, and a
    /// spawner on one of them would move a live context under whichever agent
    /// the request happened to name.
    #[test]
    fn an_event_inside_a_subagent_spawns_nothing() {
        let plan = plan(
            &event(json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "session-1",
                "agent_id": "agent-7",
                "prompt": "start with the rail"
            })),
            "alpha",
            Some("survey the widgets crate"),
            Some("agent-3"),
            &Settings::default(),
        )
        .expect("UserPromptSubmit is a known event");

        assert_eq!(
            plan.parent, None,
            "a prompt inside a subagent acts on a context that is already there"
        );
        assert_eq!(
            plan.key,
            ContextKey::subagent("alpha", "session-1", "agent-7"),
            "the event still acts on the subagent it comes from"
        );
    }

    /// Detects a `SubagentStart` planned as carrying the size of the context it
    /// acts on: the client read that size from the parent's transcript, so
    /// taking it for the child's would start every count in the child at the
    /// parent's whole context size, and the child's own events, which report a
    /// size starting near zero, would never reach it.
    ///
    /// Source: the rule that a count starts at the first event of the context
    /// that carries that context's own size. Every other event acts on the
    /// context whose transcript the size came from, which is why only this one
    /// is marked.
    #[test]
    fn only_a_subagent_start_reports_a_size_that_is_not_its_contexts_own() {
        let started = plan(
            &event(json!({
                "hook_event_name": "SubagentStart",
                "session_id": "session-1",
                "agent_id": "agent-7",
                "agent_type": "general-purpose"
            })),
            "alpha",
            Some("survey the widgets crate"),
            None,
            &Settings::default(),
        )
        .expect("SubagentStart is a known event");
        assert!(
            started.tokens_are_the_parents,
            "the size a SubagentStart carries was read from the parent's transcript"
        );

        let inside = plan_of(
            &event(json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "session-1",
                "agent_id": "agent-7",
                "prompt": "start with the rail"
            })),
            "alpha",
        )
        .expect("UserPromptSubmit is a known event");
        assert!(
            !inside.tokens_are_the_parents,
            "an event inside the subagent reports the subagent's own size"
        );
    }
}
