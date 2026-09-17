//! Tests of `POST /hook`: the state machine, the decision at a tool call, and
//! the text delivered into a session's context.
//!
//! Everything is asserted through the HTTP answer, because that is all Claude
//! Code sees. The expectations come from the plan's state machine and from the
//! example store committed in this repository, whose memories and triggers are
//! the fixture every test here shares.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use forgetmenot_server::context::{
    ContextKey, ContextState, Form, RetractReason, Shown, compute_needs, initial_active,
};
use forgetmenot_server::stats::Table;
use forgetmenot_server::store::{MemoryId, ScopeId};
use serde_json::{Value, json};

use common::{
    TestServer, additional_context, example_store, example_store_files,
    example_store_with_settings, example_store_without_settings, hook_fixture, permission_decision,
    permission_decision_reason, post_hook,
};

/// The reason a stopped tool call is given. Source: the plan's hook contract,
/// where this text is fixed so that the model can tell an automatic keyword
/// match apart from a human refusing the call.
const DENY_REASON: &str = "This tool call was not executed. Its input matched a memory trigger and a critical memory is new or changed for this context. This is an automatic keyword match, not a review, approval or denial of the call. Read the memory in the additional context. Issue the call again if it is still what you intend.";

// Lines that appear in one memory's body and nowhere else in the example store,
// so that "delivered in full" can be told apart from "named in an index line".
const BENCH_POWER_BODY: &str = "Switch the bench supply off at the wall";
const WIDGET_NAMING_BODY: &str = "Downstream drawings cite part numbers by value";
const ROCKET_STAGES_BODY: &str = "Test reports and telemetry both use this numbering";
const READING_LIST_BODY: &str = "The bench notes, the parts catalogue and the test log";
/// The index entry of `reading-list`, from its `description`.
const READING_LIST_DESCRIPTION: &str = "The workshop references are all on paper in the binder";
/// The index entry of `rocket-stages`, from its `description`.
const ROCKET_STAGES_DESCRIPTION: &str =
    "Stage one lights on the pad and later stages count upwards in firing order";

/// The context size reported with an event that is not about staleness.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// The reminder threshold K the example store's `config.yml` asks for. Source:
/// `examples/store/config.yml`, which is the fixture these tests run on.
const REMINDER_TOKENS: u64 = 200_000;

/// Whether any line of `text`, trimmed, is exactly `wanted`. Used for the list
/// of scope ids, which is one id per line; it does not depend on the wording of
/// the section label above it.
fn has_line(text: &str, wanted: &str) -> bool {
    text.lines().any(|line| line.trim() == wanted)
}

/// Whether `text` carries an index line for `id`: one line naming the memory and
/// its description, without its body.
fn has_index_line(text: &str, id: &str, description: &str) -> bool {
    text.lines()
        .any(|line| line.contains(id) && line.contains(description))
}

/// Whether `text` carries a line saying `id` was withdrawn for `reason`.
fn has_retracted_line(text: &str, id: &str, reason: &str) -> bool {
    text.lines()
        .any(|line| line.contains(id) && line.contains(reason))
}

/// The text the answer injects, or the empty string when it injects nothing.
fn context_of(answer: &Value) -> &str {
    additional_context(answer).unwrap_or_default()
}

/// One hook payload with some fields replaced.
fn event_with(fixture: &str, fields: &[(&str, Value)]) -> Value {
    let mut event = hook_fixture(fixture);
    let object = event.as_object_mut().expect("a hook payload is an object");
    for (key, value) in fields {
        object.insert((*key).to_string(), value.clone());
    }
    event
}

/// A `PreToolUse` whose input matches no trigger in the example store.
fn neutral_pre_tool_use() -> Value {
    event_with(
        "pre_tool_use_read",
        &[
            ("tool_name", json!("Read")),
            (
                "tool_input",
                json!({ "file_path": "/home/dev/notes/README.md" }),
            ),
        ],
    )
}

/// A version of `widget-naming` with `body_marker` in its body and the given
/// extra `metadata` lines, as a file to commit.
fn widget_naming_file(extra_metadata: &[&str], body_marker: &str) -> (String, Option<Vec<u8>>) {
    let mut text = String::from(
        "---\nname: widget-naming\ndescription: A widget part number is never reused or renumbered once it has shipped\nmetadata:\n  kind: critical\n  scopes:\n  - widgets\n  source: user\n",
    );
    for line in extra_metadata {
        text.push_str("  ");
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("---\n# Widget part numbers are immutable\n\n");
    text.push_str(body_marker);
    text.push('\n');
    (
        "memories/widget-naming.md".to_string(),
        Some(text.into_bytes()),
    )
}

/// A prompt that turns the `widgets` scope on through its user_message trigger.
fn widget_prompt() -> Value {
    event_with(
        "user_prompt_submit",
        &[("prompt", json!("rename the widget brackets"))],
    )
}

/// Detects a session start that injects nothing, hides the scopes the model can
/// turn on, or omits the session key the MCP tools need: the model would then
/// start with no memory and no way to ask for one.
#[test]
fn a_session_start_delivers_the_global_critical_memory_and_names_what_it_can_turn_on() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    assert_eq!(status, 200, "a session start must be answered");
    let text = context_of(&answer);
    assert!(
        text.contains(BENCH_POWER_BODY),
        "the global critical memory must arrive in full, got {text:?}"
    );
    for scope in ["widgets", "rocketry", "workshop"] {
        assert!(
            has_line(text, scope),
            "the scope {scope} must be offered as one the session can turn on, got {text:?}"
        );
    }
    assert!(
        text.contains("session_key: alpha/session-1"),
        "the session key the MCP tools take must be printed, got {text:?}"
    );
}

/// Detects a trigger that does not reach the scope's memories, and an implied
/// scope that is not closed over: a knowledge memory must arrive as its index
/// line and a critical one in full.
#[test]
fn a_matching_user_message_delivers_the_scopes_critical_memory_in_full_and_knowledge_as_an_index() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &widget_prompt());

    assert_eq!(status, 200, "a user prompt must be answered");
    let text = context_of(&answer);
    assert!(
        text.contains(WIDGET_NAMING_BODY),
        "the triggered scope's critical memory must arrive in full, got {text:?}"
    );
    assert!(
        has_index_line(text, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "the implied scope's knowledge memory must arrive as an index line, got {text:?}"
    );
    assert!(
        !text.contains(ROCKET_STAGES_BODY),
        "a knowledge memory must not arrive in full, got {text:?}"
    );
}

/// Detects a deny loop: if the memory is not recorded as delivered when the call
/// is stopped, the model's re-issued call is stopped again and the session cannot
/// make progress.
#[test]
fn a_stopped_tool_call_is_allowed_when_it_is_issued_again() {
    let server = TestServer::start(example_store_files(), |_| {});
    let call = hook_fixture("pre_tool_use_bash");

    let (status, first) = server.hook("alpha", SOME_TOKENS, &call);
    assert_eq!(status, 200, "a tool call must be answered");
    assert_eq!(
        permission_decision(&first),
        Some("deny"),
        "a new critical memory must stop the call, got {first}"
    );
    assert_eq!(
        permission_decision_reason(&first),
        Some(DENY_REASON),
        "the reason must be the fixed text, got {first}"
    );
    assert!(
        context_of(&first).contains(WIDGET_NAMING_BODY),
        "the stopped call must carry the memory to read, got {first}"
    );

    let (status, second) = server.hook("alpha", SOME_TOKENS, &call);
    assert_eq!(status, 200, "the re-issued call must be answered");
    assert_eq!(
        permission_decision(&second),
        None,
        "the re-issued call must not be stopped, got {second}"
    );
    assert!(
        !context_of(&second).contains(WIDGET_NAMING_BODY),
        "the memory must not be delivered twice, got {second}"
    );
}

/// Detects a delivery record that is not consulted: an event with nothing new,
/// changed or stale must attach nothing at all.
#[test]
fn a_tool_call_with_nothing_owed_attaches_nothing() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &neutral_pre_tool_use());

    assert_eq!(status, 200, "a tool call must be answered");
    assert_eq!(
        additional_context(&answer),
        None,
        "nothing is owed, so nothing may be injected, got {answer}"
    );
    assert_eq!(
        permission_decision(&answer),
        None,
        "nothing is owed, so the call must not be stopped, got {answer}"
    );
}

/// Detects a catalog that is not rebuilt when the store's head moves: an edit
/// committed outside the server would never reach a session that had already
/// been given the memory.
#[test]
fn an_edit_committed_outside_the_server_is_delivered_again_at_the_next_event() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &widget_prompt());

    server.commit(
        "tighten the widget rule",
        vec![widget_naming_file(
            &[],
            "The rule now also covers prototypes.",
        )],
    );
    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("stop"));

    assert_eq!(status, 200, "the next event must be answered");
    assert!(
        context_of(&answer).contains("The rule now also covers prototypes."),
        "the edited memory must be delivered again in full, got {answer}"
    );
}

/// Detects a changed critical memory that does not stop the next tool call,
/// which would let the model act on a rule it has only the old version of.
#[test]
fn a_changed_critical_memory_stops_the_next_tool_call() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &widget_prompt());

    server.commit(
        "tighten the widget rule",
        vec![widget_naming_file(
            &[],
            "The rule now also covers prototypes.",
        )],
    );
    let (status, answer) = server.hook("alpha", SOME_TOKENS, &neutral_pre_tool_use());

    assert_eq!(status, 200, "a tool call must be answered");
    assert_eq!(
        permission_decision(&answer),
        Some("deny"),
        "a changed critical memory must stop the call, got {answer}"
    );
}

/// Detects a reader that still acts on the legacy `archived` key, which earlier
/// versions of this server wrote to retire a memory. A store written by one of
/// those versions must not have memories silently withheld, or withdrawn from a
/// session that was given them, over a key this server does not interpret.
#[test]
fn a_memory_carrying_the_legacy_archived_key_is_delivered_like_any_other() {
    let server = TestServer::start(example_store_files(), |_| {});
    let marker = "The rule covers prototypes as well.";
    server.commit(
        "restore the store as an older server left it",
        vec![widget_naming_file(&["archived: true"], marker)],
    );

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &widget_prompt());

    assert_eq!(status, 200, "the event must be answered");
    let text = context_of(&answer);
    assert!(
        text.contains(marker),
        "a memory carrying the legacy key must be delivered in full, got {text:?}"
    );
    assert!(
        !has_retracted_line(text, "widget-naming", "deleted"),
        "a memory carrying the legacy key must not be withdrawn, got {text:?}"
    );
}

/// Detects a reminder threshold that fires late, a `reminder_tokens` in the
/// store that never reaches the state machine, and a reminder that covers only
/// the critical memories: everything delivered K tokens of context ago is out of
/// the model's reach, and each kind has to come back in the form it is delivered
/// in, the rule in full and the knowledge memory as its index line.
#[test]
fn everything_due_is_delivered_again_once_the_context_has_grown_by_the_stores_threshold() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", Some(10_000), &hook_fixture("session_start"));

    let (status, answer) = server.hook(
        "alpha",
        Some(10_000 + REMINDER_TOKENS),
        &neutral_pre_tool_use(),
    );

    assert_eq!(status, 200, "the event must be answered");
    let text = context_of(&answer);
    assert!(
        text.contains(BENCH_POWER_BODY),
        "a critical memory out of reach must be delivered again in full, got {text:?}"
    );
    assert!(
        has_index_line(text, "reading-list", READING_LIST_DESCRIPTION),
        "a knowledge memory out of reach must be delivered again as its index line, got {text:?}"
    );
    assert!(
        !text.contains(READING_LIST_BODY),
        "a knowledge memory must not be delivered in full by a reminder, got {text:?}"
    );
}

/// Detects a reminder threshold that fires early: just short of K everything
/// delivered is still in the model's reach and delivering any of it again wastes
/// the context the reminder exists to protect.
#[test]
fn nothing_is_delivered_again_just_short_of_the_stores_threshold() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", Some(10_000), &hook_fixture("session_start"));

    let (status, answer) = server.hook(
        "alpha",
        Some(10_000 + REMINDER_TOKENS - 1),
        &neutral_pre_tool_use(),
    );

    assert_eq!(status, 200, "the event must be answered");
    assert_eq!(
        additional_context(&answer),
        None,
        "below the threshold neither form may be delivered again, got {answer}"
    );
}

/// Detects a reminder threshold that applies when no store asks for one: a store
/// with no settings file must never repeat a memory it has already delivered,
/// however far the context has grown, because nobody asked for the context to be
/// spent that way.
#[test]
fn no_reminder_is_delivered_when_the_store_has_no_settings_file() {
    let server = TestServer::start(example_store_without_settings(), |_| {});
    server.hook("alpha", Some(10_000), &hook_fixture("session_start"));

    let (status, answer) = server.hook(
        "alpha",
        Some(10_000 + 100 * REMINDER_TOKENS),
        &neutral_pre_tool_use(),
    );

    assert_eq!(status, 200, "the event must be answered");
    assert_eq!(
        additional_context(&answer),
        None,
        "with no settings file reminders are off, so nothing may be repeated, got {answer}"
    );
}

/// Detects an interrupt that is held on regardless of the store's settings: a
/// store that has turned it off wants the memory in the context and the call to
/// run, and stopping the call anyway makes every session pay for a setting it
/// turned off.
#[test]
fn a_store_that_turns_the_interrupt_off_is_given_the_memory_without_the_call_being_stopped() {
    let server = TestServer::start(
        example_store_with_settings("interrupt_on_critical: false\n"),
        |_| {},
    );

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_bash"));

    assert_eq!(status, 200, "a tool call must be answered");
    assert_eq!(
        permission_decision(&answer),
        None,
        "with the interrupt off the call must not be stopped, got {answer}"
    );
    assert!(
        context_of(&answer).contains(WIDGET_NAMING_BODY),
        "the critical memory must still be delivered in full, got {answer}"
    );
}

/// Detects an exemption list that is not consulted, or one that releases every
/// tool: a tool the store exempts must run while the memory is attached, and a
/// tool it does not exempt must still be stopped.
#[test]
fn a_tool_the_store_exempts_is_not_stopped_while_another_tool_still_is() {
    let server = TestServer::start(
        example_store_with_settings("interrupt_exempt_tools: [Bash]\n"),
        |_| {},
    );
    let exempt = event_with("pre_tool_use_bash", &[("session_id", json!("session-a"))]);
    let held = event_with(
        "pre_tool_use_bash",
        &[
            ("session_id", json!("session-b")),
            ("tool_name", json!("Write")),
        ],
    );

    let (_, exempt_answer) = server.hook("alpha", SOME_TOKENS, &exempt);
    let (_, held_answer) = server.hook("alpha", SOME_TOKENS, &held);

    assert_eq!(
        permission_decision(&exempt_answer),
        None,
        "an exempt tool must not be stopped, got {exempt_answer}"
    );
    assert!(
        context_of(&exempt_answer).contains(WIDGET_NAMING_BODY),
        "an exempt tool must still carry the memory, got {exempt_answer}"
    );
    assert_eq!(
        permission_decision(&held_answer),
        Some("deny"),
        "a tool the store does not exempt must still be stopped, got {held_answer}"
    );
}

/// Detects a subagent that is given its parent's scopes although the store asked
/// for subagents to start from nothing: a subagent would be handed rules its own
/// task never touches, which is the context the setting exists to save.
#[test]
fn a_subagent_starts_with_the_implicit_scopes_alone_when_the_store_says_so() {
    let server = TestServer::start(
        example_store_with_settings("subagents_inherit_scopes: false\n"),
        |_| {},
    );
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &widget_prompt());

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("subagent_start"));

    assert_eq!(status, 200, "a subagent start must be answered");
    let text = context_of(&answer);
    assert!(
        !text.contains(WIDGET_NAMING_BODY),
        "the subagent must not be given the scope its session turned on, got {text:?}"
    );
    assert!(
        text.contains(BENCH_POWER_BODY),
        "the subagent must still be given what the implicit scopes owe it, got {text:?}"
    );
}

/// Detects a knowledge index delivered although the store asked for knowledge to
/// be fetched on demand: the index lines are what that setting exists to keep
/// out of the context, while the critical memories must be unaffected.
#[test]
fn no_knowledge_index_line_is_delivered_when_the_store_turns_the_index_off() {
    let server = TestServer::start(
        example_store_with_settings("deliver_knowledge_index: false\n"),
        |_| {},
    );

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &widget_prompt());

    assert_eq!(status, 200, "a user prompt must be answered");
    let text = context_of(&answer);
    assert!(
        !has_index_line(text, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "no knowledge index line may be delivered, got {text:?}"
    );
    assert!(
        text.contains(WIDGET_NAMING_BODY),
        "a critical memory must still be delivered in full, got {text:?}"
    );
}

/// Detects a tool result matched past the limit the store set, and a limit that
/// throws the result away before it: matching is linear in the text, so a limit
/// that is not applied makes a large tool result cost a regex pass over all of
/// it, and one applied too soon silently stops triggers firing on results the
/// store meant to match.
#[test]
fn a_trigger_past_the_stores_tool_result_limit_does_not_fire_while_one_before_it_does() {
    let limit = 64;
    let marker = "SUPERNOVA";
    let scope = (
        "scopes/results.yaml".to_string(),
        Some(
            format!("id: results\ntriggers:\n- on: tool_result\n  pattern: '{marker}'\n")
                .into_bytes(),
        ),
    );
    let memory = (
        "memories/result-rule.md".to_string(),
        Some(
            format!(
                "---\nname: result-rule\ndescription: What to do when a test run reports a \
                 {marker}\nmetadata:\n  kind: critical\n  scopes:\n  - results\n---\n\
                 # The {marker} rule\n\nStop the run and read the log.\n"
            )
            .into_bytes(),
        ),
    );
    let settings = (
        "config.yml".to_string(),
        Some(format!("tool_result_match_limit: {limit}\n").into_bytes()),
    );
    let server = TestServer::start(vec![scope, memory, settings], |_| {});
    let result_with_marker_at = |offset: usize, session: &str| {
        json!({
            "hook_event_name": "PostToolUse",
            "session_id": session,
            "cwd": "/home/dev/widgets",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "tool_output": format!("{}{marker} in the log\n", "-".repeat(offset)),
        })
    };

    let (_, past) = server.hook("alpha", SOME_TOKENS, &result_with_marker_at(limit, "past"));
    let (_, before) = server.hook(
        "alpha",
        SOME_TOKENS,
        &result_with_marker_at(limit / 2, "before"),
    );

    assert_eq!(
        additional_context(&past),
        None,
        "a trigger past the limit must not fire, got {past}"
    );
    assert!(
        context_of(&before).contains("Stop the run and read the log."),
        "a trigger before the limit must fire and deliver its memory, got {before}"
    );
}

/// Text carrying the trigger pattern of every scope of the example store, in the
/// shape the store's own session tools answer in: scope ids one per line and a
/// directory. Source: the patterns in `examples/store/scopes/*.yaml`.
const EVERY_SCOPE_PATTERN: &str =
    "active:\n- widgets\navailable:\n- rocketry\n- workshop\ndirectory: /home/dev/workshop/bench\n";

/// A `PostToolUse` from `tool` in `session` whose result is
/// [`EVERY_SCOPE_PATTERN`].
fn result_of(tool: &str, session: &str) -> Value {
    event_with(
        "post_tool_use",
        &[
            ("session_id", json!(session)),
            ("tool_name", json!(tool)),
            ("tool_output", json!(EVERY_SCOPE_PATTERN)),
        ],
    )
}

/// Detects the result of one of the store's own MCP tools being matched against
/// triggers: `session_scopes` answers with every scope id the store has, so
/// matching its answer activates scopes the work never touched, and a
/// `session_scope_off` is turned straight back on by its own answer. Source: the
/// trigger fields of `examples/store/scopes/*.yaml`, where `rocketry` names no
/// field and so is matched against a tool result.
#[test]
fn a_result_of_the_stores_own_tool_activates_no_scope_while_the_same_text_from_bash_does() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, exempt) = server.hook(
        "alpha",
        SOME_TOKENS,
        &result_of("mcp__forgetmenot__session_scopes", "session-store"),
    );
    let (_, matched) = server.hook("alpha", SOME_TOKENS, &result_of("Bash", "session-bash"));

    assert_eq!(status, 200, "a finished tool call must be answered");
    let exempt_text = context_of(&exempt);
    assert!(
        !has_index_line(exempt_text, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "the store's own answer must not activate rocketry, got {exempt_text:?}"
    );
    assert!(
        !exempt_text.contains(WIDGET_NAMING_BODY),
        "the store's own answer must not activate widgets, got {exempt_text:?}"
    );
    let matched_text = context_of(&matched);
    assert!(
        has_index_line(matched_text, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "the same text from a shell command must still activate rocketry, got {matched_text:?}"
    );
}

/// Detects the input of one of the store's own MCP tools being matched against
/// triggers: a `memory_put` carries the whole body of a memory, so matching it
/// activates every scope that body talks about and holds the call that writes it
/// against a memory the session was never working on. Source: the `tool_input`
/// trigger of `examples/store/scopes/widgets.yaml`.
#[test]
fn an_input_to_the_stores_own_tool_activates_no_scope() {
    let server = TestServer::start(example_store_files(), |_| {});
    // The session start settles what the implicit scopes owe this session, so
    // that whatever the tool calls below deliver or hold for is the trigger's
    // doing alone.
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    let call_of = |tool: &str| {
        event_with(
            "pre_tool_use_read",
            &[
                ("tool_name", json!(tool)),
                (
                    "tool_input",
                    json!({ "id": "widget-naming", "body": EVERY_SCOPE_PATTERN }),
                ),
            ],
        )
    };

    let (status, exempt) = server.hook(
        "alpha",
        SOME_TOKENS,
        &call_of("mcp__forgetmenot__memory_put"),
    );
    let (_, matched) = server.hook("alpha", SOME_TOKENS, &call_of("Bash"));

    assert_eq!(status, 200, "a tool call must be answered");
    assert_eq!(
        additional_context(&exempt),
        None,
        "a call into the store must deliver nothing its input matched, got {exempt}"
    );
    assert_eq!(
        permission_decision(&exempt),
        None,
        "a call into the store must not be held for what its own input said, got {exempt}"
    );
    assert!(
        context_of(&matched).contains(WIDGET_NAMING_BODY),
        "the same input to a shell command must deliver the widgets memory, got {matched}"
    );
}

/// Detects a `trigger_exempt_tools` list that is not consulted, or one that
/// exempts every tool: a store that names `Read` wants what it reads kept out of
/// the matching, and a tool it did not name must still be matched. Source: the
/// `trigger_exempt_tools` setting, whose match is on the whole tool name.
#[test]
fn a_tool_the_store_exempts_is_not_matched_while_another_tool_still_is() {
    let server = TestServer::start(
        example_store_with_settings("trigger_exempt_tools:\n- Read\n"),
        |_| {},
    );

    let (status, exempt) = server.hook("alpha", SOME_TOKENS, &result_of("Read", "session-read"));
    let (_, matched) = server.hook("alpha", SOME_TOKENS, &result_of("Grep", "session-grep"));

    assert_eq!(status, 200, "a finished tool call must be answered");
    let exempt_text = context_of(&exempt);
    assert!(
        !has_index_line(exempt_text, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "the result of an exempt tool must activate no scope, got {exempt_text:?}"
    );
    let matched_text = context_of(&matched);
    assert!(
        has_index_line(matched_text, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "the result of a tool the store did not name must still be matched, got {matched_text:?}"
    );
}

/// Detects an exemption tied to one server name, and one that ignores the
/// `mcp__` form: the name this server is registered under is the user's to
/// choose, so `mcp__memory__session_scopes` is the same tool, while a bare
/// `session_scopes` is some other tool that happens to share the name and its
/// result is matched like any other. Source: the `mcp__<server>__<tool>` naming
/// Claude Code gives an MCP tool.
#[test]
fn the_stores_own_tool_under_another_server_name_is_still_exempt() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, renamed) = server.hook(
        "alpha",
        SOME_TOKENS,
        &result_of("mcp__memory__session_scopes", "session-renamed"),
    );
    let (_, bare) = server.hook(
        "alpha",
        SOME_TOKENS,
        &result_of("session_scopes", "session-bare"),
    );

    assert_eq!(status, 200, "a finished tool call must be answered");
    let renamed_text = context_of(&renamed);
    assert!(
        !has_index_line(renamed_text, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "the store's own tool under any server name must activate no scope, got {renamed_text:?}"
    );
    let bare_text = context_of(&bare);
    assert!(
        has_index_line(bare_text, "rocket-stages", ROCKET_STAGES_DESCRIPTION),
        "a tool named without the mcp prefix must still be matched, got {bare_text:?}"
    );
}

/// A store whose only files are one global critical memory with `body` and a
/// settings file with `answer_file_threshold` set to `threshold`, so that the
/// whole answer to a session start is that memory and nothing else.
fn store_with_answer_threshold(threshold: &str, body: &str) -> Vec<(String, Option<Vec<u8>>)> {
    vec![
        (
            "memories/long-rule.md".to_string(),
            Some(
                format!(
                    "---\nname: long-rule\ndescription: A rule long enough to test the answer \
                     notice\nmetadata:\n  kind: critical\n  scopes:\n  - global\n---\n{body}\n"
                )
                .into_bytes(),
            ),
        ),
        (
            "config.yml".to_string(),
            Some(format!("answer_file_threshold: {threshold}\n").into_bytes()),
        ),
    ]
}

/// The first line of the answer's text, which is the only part of a persisted
/// answer that Claude Code is sure to show the model.
fn first_line(answer: &Value) -> &str {
    context_of(answer).lines().next().unwrap_or("")
}

/// Detects a notice that is missing, that comes after content Claude Code's
/// preview cut would remove, or that replaces the answer: past the threshold
/// the model sees only the first lines and the file path, so the first line
/// has to tell it to read the file, and the file has to still hold the memory.
/// Source: the setting's documented meaning and the preview rule in the README.
#[test]
fn an_answer_past_the_stores_threshold_opens_with_the_notice_and_still_carries_the_memory() {
    let body = "Stop the run and read the log. ".repeat(20);
    let server = TestServer::start(store_with_answer_threshold("200", &body), |_| {});

    let (_, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let opening = first_line(&answer);
    assert!(
        opening.contains("READ THE FILE FIRST") && opening.contains("Read tool"),
        "the first line must tell the model to read the file, got {opening:?}"
    );
    assert!(
        context_of(&answer).contains(body.trim_end()),
        "the memory must still be in the answer in full, got {}",
        context_of(&answer)
    );
}

/// Detects a notice sent whatever the length: an answer Claude Code shows whole
/// must not send the model looking for a file Claude Code never wrote.
/// Source: Claude Code persists an answer only past the threshold.
#[test]
fn an_answer_within_the_stores_threshold_carries_no_notice() {
    let body = "Stop the run and read the log.";
    let server = TestServer::start(store_with_answer_threshold("100000", body), |_| {});

    let (_, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    assert!(
        !context_of(&answer).contains("READ THE FILE"),
        "an answer within the threshold must carry no notice, got {}",
        context_of(&answer)
    );
    assert!(
        first_line(&answer).starts_with("[forgetmenot] context"),
        "the answer must open with its context line as before, got {:?}",
        first_line(&answer)
    );
}

/// Detects the null setting read as a threshold of zero: null means no notice,
/// however long the answer.
#[test]
fn a_store_that_turns_the_threshold_off_sends_no_notice_for_a_long_answer() {
    let body = "Stop the run and read the log. ".repeat(20);
    let server = TestServer::start(store_with_answer_threshold("null", &body), |_| {});

    let (_, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    assert!(
        !context_of(&answer).contains("READ THE FILE"),
        "a null threshold must send no notice, got {}",
        context_of(&answer)
    );
}

/// Detects the answer measured in bytes: Claude Code compares its own string
/// length, in UTF-16 code units, so an answer of three-byte characters that is
/// past the threshold in bytes and within it in characters is shown whole and
/// must carry no notice.
#[test]
fn an_answer_past_the_threshold_in_bytes_but_not_in_characters_carries_no_notice() {
    // 120 characters of three bytes each: 360 bytes, 120 UTF-16 code units.
    let body = "\u{20AC}".repeat(120);
    let context_line_and_headers = 200;
    let threshold = (120 + context_line_and_headers).to_string();
    let server = TestServer::start(store_with_answer_threshold(&threshold, &body), |_| {});

    let (_, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let text = context_of(&answer);
    assert!(
        text.len() as u64 > 120 + context_line_and_headers,
        "the fixture must be past the threshold in bytes, got {} bytes",
        text.len()
    );
    assert!(
        !text.contains("READ THE FILE"),
        "an answer within the threshold in characters must carry no notice, got {text}"
    );
}

/// Detects a compaction that leaves the delivery record standing, in either
/// form: nothing delivered before a compaction is in the rebuilt conversation,
/// so a critical memory has to arrive in full again and a knowledge memory as
/// its index line again.
///
/// Source: Claude Code 2.1.x sends `SessionStart` with `"source": "compact"`
/// for the conversation it rebuilds, and injects the answer to it.
#[test]
fn everything_due_is_delivered_again_at_the_session_start_of_a_compaction() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, answer) = server.hook(
        "alpha",
        SOME_TOKENS,
        &event_with("session_start", &[("source", json!("compact"))]),
    );

    assert_eq!(
        status, 200,
        "the session start of a compaction must be answered"
    );
    let text = context_of(&answer);
    assert!(
        text.contains(BENCH_POWER_BODY),
        "the critical memory must be delivered again after a compaction, got {text:?}"
    );
    assert!(
        has_index_line(
            text,
            "reading-list",
            "The workshop references are all on paper in the binder"
        ),
        "the knowledge index must be delivered again after a compaction, got {text:?}"
    );
}

/// Detects the whole store delivered twice for one compaction: Claude Code runs
/// the session start of the rebuilt conversation and then the compaction event,
/// and a second event that begins the delivery record makes the first prompt
/// afterwards hear everything again. Also detects a session start that drops the
/// scopes its session works in, which would leave the rebuilt conversation
/// without the memories of those scopes.
///
/// Source: Claude Code 2.1.x runs `SessionStart` with `"source": "compact"` and
/// injects its answer into the rebuilt conversation, then runs `PostCompact`,
/// whose answer it takes no context from.
#[test]
fn a_compaction_delivers_everything_once_at_its_session_start() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &widget_prompt());

    let (_, rebuilt) = server.hook(
        "alpha",
        SOME_TOKENS,
        &event_with("session_start", &[("source", json!("compact"))]),
    );
    let (_, compaction) = server.hook("alpha", SOME_TOKENS, &hook_fixture("post_compact"));
    let (_, prompt) = server.hook(
        "alpha",
        SOME_TOKENS,
        &event_with("user_prompt_submit", &[("prompt", json!("carry on"))]),
    );

    let text = context_of(&rebuilt);
    assert!(
        text.contains(BENCH_POWER_BODY),
        "the rebuilt conversation must be given the global critical memory, got {text:?}"
    );
    assert!(
        text.contains(WIDGET_NAMING_BODY),
        "the session keeps the scopes it works in, so their critical memory must arrive too, got {text:?}"
    );
    assert_eq!(
        compaction,
        json!({}),
        "a compaction takes no context, so it must be answered with the empty object, got {compaction}"
    );
    assert_eq!(
        additional_context(&prompt),
        None,
        "everything was delivered at the session start, so the prompt must hear none of it again, got {prompt}"
    );
}

/// Detects the scopes of one session reaching another: scopes belong to the
/// session that turned them on, so a session id first seen at a session start
/// works in the three implicit scopes and no others.
#[test]
fn a_new_session_id_starts_from_the_implicit_scopes() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &widget_prompt());

    let (status, answer) = server.hook(
        "alpha",
        SOME_TOKENS,
        &event_with("session_start", &[("session_id", json!("session-9"))]),
    );

    assert_eq!(status, 200, "a session start must be answered");
    let text = context_of(&answer);
    assert!(
        text.contains(BENCH_POWER_BODY),
        "the new session must be given the global critical memory, got {text:?}"
    );
    assert!(
        !text.contains(WIDGET_NAMING_BODY),
        "a scope another session turned on must not reach a new session, got {text:?}"
    );
}

/// Detects an answer to a compaction that carries a `hookSpecificOutput`:
/// Claude Code validates the object it reads and takes no payload for
/// `PostCompact`, so the hook would be reported as failing after every
/// compaction, whether or not anything was owed.
///
/// Expectation source: Claude Code 2.1.x refused
/// `{"hookSpecificOutput": {"hookEventName": "PostCompact"}}` with
/// `hookSpecificOutput.hookEventName: expected one of "PreToolUse" | …`, and
/// leaves every top-level key optional, so `{}` is valid for every event
/// (observed in a session on 2026-09-13).
#[test]
fn a_compaction_is_answered_with_the_empty_object_and_nothing_else() {
    let server = TestServer::start(example_store_files(), |_| {});
    // A session start first, so that the compaction arrives at a context the
    // server has already delivered to rather than one it is seeing for the
    // first time.
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("post_compact"));

    assert_eq!(status, 200, "a compaction must be acknowledged");
    assert_eq!(
        answer,
        json!({}),
        "a compaction must be answered with the empty object, got {answer}"
    );
}

/// Detects a subagent that starts with only the implicit scopes: it would not
/// see the memories the session it was spawned from is working with.
#[test]
fn a_subagent_inherits_the_scopes_its_session_had_turned_on() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &widget_prompt());

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("subagent_start"));

    assert_eq!(status, 200, "a subagent start must be answered");
    assert!(
        context_of(&answer).contains(WIDGET_NAMING_BODY),
        "the subagent must be given the memory of the scope its session turned on, got {answer}"
    );
}

/// Detects a subagent sharing its parent's delivery record: the child has its
/// own context and must be stopped once for a memory the parent already has,
/// while the parent is not affected by the child being told.
#[test]
fn a_subagents_first_tool_call_is_stopped_once_and_the_session_is_unaffected() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &widget_prompt());
    let in_subagent = event_with(
        "pre_tool_use_in_subagent",
        &[("tool_input", json!({ "pattern": "pub fn" }))],
    );

    let (_, first) = server.hook("alpha", SOME_TOKENS, &in_subagent);
    assert_eq!(
        permission_decision(&first),
        Some("deny"),
        "the subagent's first call must be stopped for the inherited memory, got {first}"
    );
    assert!(
        context_of(&first).contains(WIDGET_NAMING_BODY),
        "the stopped call must carry the inherited memory, got {first}"
    );

    let (_, second) = server.hook("alpha", SOME_TOKENS, &in_subagent);
    assert_eq!(
        permission_decision(&second),
        None,
        "the subagent's re-issued call must not be stopped, got {second}"
    );

    let (_, in_session) = server.hook("alpha", SOME_TOKENS, &neutral_pre_tool_use());
    assert_eq!(
        permission_decision(&in_session),
        None,
        "the session must not be stopped by what its subagent was told, got {in_session}"
    );
    assert_eq!(
        additional_context(&in_session),
        None,
        "the session must not be delivered anything by its subagent's events, got {in_session}"
    );
}

/// Detects a machine qualifier that is ignored: a directory trigger written for
/// one machine must not fire on another, where the same path means something
/// else.
#[test]
fn a_machine_qualified_directory_trigger_fires_only_on_its_machine() {
    let server = TestServer::start(example_store_files(), |_| {});
    let start = event_with("session_start", &[("cwd", json!("/workshop/bench"))]);

    let (_, on_its_machine) = server.hook("alpha", SOME_TOKENS, &start);
    let (_, on_another_machine) = server.hook("beta", SOME_TOKENS, &start);

    assert!(
        !has_line(context_of(&on_its_machine), "workshop"),
        "the scope the directory turned on must not be offered as available, got {:?}",
        context_of(&on_its_machine)
    );
    assert!(
        has_line(context_of(&on_another_machine), "workshop"),
        "on another machine the scope must stay off and be offered, got {:?}",
        context_of(&on_another_machine)
    );
}

// ------------------------------------------------------- the two directories
//
// The two directory fields differ only once the agent has run `cd`: until then
// the session directory and the shell directory are the same string, so every
// test below moves the shell and then asks which of the two a trigger saw.
// Source: the README's account of the two fields, and the hook payloads Claude
// Code sends, where `cwd` is the shell's directory at the time of the event.

/// The directory the session is started in.
const STARTED_IN: &str = "/start/project";
/// The directory the agent's shell moves to.
const MOVED_TO: &str = "/moved/elsewhere";

/// A line that appears only in the body of the `bench-vice` memory built below,
/// so that "the directory trigger fired" can be told apart from anything the
/// example store delivers on its own.
const BENCH_VICE_BODY: &str = "The vice jaws are shimmed with copper";

/// One scope whose only trigger is `on: <field>` with `pattern`, and one
/// critical memory in it, as files to commit. A pair rather than a whole store,
/// so that a test can either start a server with them or add them to a store a
/// server is already reading.
fn directory_scope_files(field: &str, pattern: &str) -> Vec<(String, Option<Vec<u8>>)> {
    vec![
        (
            "scopes/bench.yaml".to_string(),
            Some(format!("id: bench\ntriggers:\n- on: {field}\n  pattern: '{pattern}'\n").into_bytes()),
        ),
        (
            "memories/bench-vice.md".to_string(),
            Some(
                format!(
                    "---\nname: bench-vice\ndescription: The bench vice is shimmed and the shims stay with it\nmetadata:\n  kind: critical\n  scopes:\n  - bench\n  source: user\n---\n# The bench vice\n\n{BENCH_VICE_BODY}\n"
                )
                .into_bytes(),
            ),
        ),
    ]
}

/// `files` with the directory scope added, for a server that is to start with it.
fn with_directory_scope(
    files: Vec<(String, Option<Vec<u8>>)>,
    field: &str,
    pattern: &str,
) -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = files;
    files.extend(directory_scope_files(field, pattern));
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// A session start in `cwd`, which is the directory the session is started in.
fn session_start_in(cwd: &str) -> Value {
    event_with("session_start", &[("cwd", json!(cwd))])
}

/// A prompt sent from `cwd`, whose text matches no trigger in the example
/// store. A prompt is matched against no directory at all, so it leaves the two
/// directories to the events that do match them.
fn prompt_from(cwd: &str) -> Value {
    event_with(
        "user_prompt_submit",
        &[("cwd", json!(cwd)), ("prompt", json!("carry on"))],
    )
}

/// A tool call from `cwd` whose tool and input match no trigger in the example
/// store, so that only the directories can fire anything.
fn tool_call_from(cwd: &str) -> Value {
    event_with(
        "pre_tool_use_read",
        &[
            ("cwd", json!(cwd)),
            ("tool_name", json!("Read")),
            (
                "tool_input",
                json!({ "file_path": "/home/dev/notes/README.md" }),
            ),
        ],
    )
}

/// A tool call inside the subagent of the `subagent_start` fixture, from `cwd`,
/// whose input matches no trigger in the example store.
fn subagent_tool_call_from(cwd: &str) -> Value {
    event_with(
        "pre_tool_use_in_subagent",
        &[
            ("cwd", json!(cwd)),
            ("tool_input", json!({ "pattern": "pub fn" })),
        ],
    )
}

/// Detects a `session_directory` trigger matched against the directory the
/// shell is in at the time of the event: the session directory is where
/// `claude` was started and does not move, so a trigger on it must still fire
/// at a tool call the agent makes from somewhere else.
///
/// The scope is committed after the session start because at a session start
/// the two directories are the same string; firing there would say nothing
/// about which of them was matched.
#[test]
fn a_session_directory_trigger_fires_at_a_tool_call_from_another_directory() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &session_start_in(STARTED_IN));
    server.commit(
        "add a scope triggered by the session directory",
        directory_scope_files("session_directory", "/start(/|$)"),
    );

    let (_, answer) = server.hook("alpha", SOME_TOKENS, &tool_call_from(MOVED_TO));

    assert!(
        context_of(&answer).contains(BENCH_VICE_BODY),
        "a session_directory trigger on {STARTED_IN} must fire at a call made from {MOVED_TO}, got {:?}",
        context_of(&answer)
    );
}

/// Detects a `session_directory` trigger matched against the directory the
/// shell moved to, which would turn a scope on for a session that was never
/// started there: the whole point of the field is that it names where the
/// session began.
#[test]
fn a_session_directory_trigger_does_not_fire_on_the_directory_the_shell_moved_to() {
    let server = TestServer::start(
        with_directory_scope(example_store_files(), "session_directory", "/moved(/|$)"),
        |_| {},
    );

    let (_, start) = server.hook("alpha", SOME_TOKENS, &session_start_in(STARTED_IN));
    let (_, call) = server.hook("alpha", SOME_TOKENS, &tool_call_from(MOVED_TO));

    assert!(
        !context_of(&start).contains(BENCH_VICE_BODY),
        "the scope must stay off at a session start in {STARTED_IN}, got {:?}",
        context_of(&start)
    );
    assert!(
        !context_of(&call).contains(BENCH_VICE_BODY),
        "the scope must stay off at a call made from {MOVED_TO} by a session started in {STARTED_IN}, got {:?}",
        context_of(&call)
    );

    // The same trigger on a session that did start there, so that "it never
    // fires" cannot pass for "it fired on the wrong directory".
    let elsewhere = event_with(
        "session_start",
        &[("cwd", json!(MOVED_TO)), ("session_id", json!("session-2"))],
    );
    let (_, started_there) = server.hook("alpha", SOME_TOKENS, &elsewhere);
    assert!(
        context_of(&started_there).contains(BENCH_VICE_BODY),
        "the same trigger must fire for a session started in {MOVED_TO}, got {:?}",
        context_of(&started_there)
    );
}

/// Detects a `shell_directory` trigger that is left behind in the directory the
/// session started in: the field is the working directory of the session's
/// shell, so a scope written for the directory the agent moved into would never
/// turn on.
#[test]
fn a_shell_directory_trigger_fires_at_a_tool_call_in_the_directory_the_shell_moved_to() {
    let server = TestServer::start(
        with_directory_scope(example_store_files(), "shell_directory", "/moved(/|$)"),
        |_| {},
    );

    let (_, start) = server.hook("alpha", SOME_TOKENS, &session_start_in(STARTED_IN));
    let (_, call) = server.hook("alpha", SOME_TOKENS, &tool_call_from(MOVED_TO));

    assert!(
        !context_of(&start).contains(BENCH_VICE_BODY),
        "the scope must stay off while the shell is in {STARTED_IN}, got {:?}",
        context_of(&start)
    );
    assert!(
        context_of(&call).contains(BENCH_VICE_BODY),
        "a shell_directory trigger on {MOVED_TO} must fire at a call made from there, got {:?}",
        context_of(&call)
    );
}

/// Detects a `shell_directory` trigger matched against the directory the
/// session started in rather than the one the shell is in now, which would keep
/// a scope turning on for work the agent has moved away from.
///
/// The session's first event is a prompt, which is matched against no
/// directory, so the trigger's one chance to fire is the tool call.
#[test]
fn a_shell_directory_trigger_does_not_fire_on_the_directory_the_session_started_in() {
    let server = TestServer::start(
        with_directory_scope(example_store_files(), "shell_directory", "/start(/|$)"),
        |_| {},
    );
    server.hook("alpha", SOME_TOKENS, &prompt_from(STARTED_IN));

    let (_, moved_away) = server.hook("alpha", SOME_TOKENS, &tool_call_from(MOVED_TO));
    assert!(
        !context_of(&moved_away).contains(BENCH_VICE_BODY),
        "the scope must stay off at a call made from {MOVED_TO}, got {:?}",
        context_of(&moved_away)
    );

    // The same trigger with the shell back where the session began, so that "it
    // never fires" cannot pass for "it ignored the shell's directory".
    let (_, back_again) = server.hook("alpha", SOME_TOKENS, &tool_call_from(STARTED_IN));
    assert!(
        context_of(&back_again).contains(BENCH_VICE_BODY),
        "the same trigger must fire at a call made from {STARTED_IN}, got {:?}",
        context_of(&back_again)
    );
}

/// Detects a subagent given a session directory of its own, or none at all: a
/// subagent runs in its parent's session, so the directory `claude` was started
/// in is the same for both, whatever the subagent's shell is doing.
///
/// Subagents are set to inherit no scopes, so the only way the scope can reach
/// the subagent is the session directory it took from its parent.
#[test]
fn a_subagent_takes_the_session_directory_of_the_session_it_runs_in() {
    let server = TestServer::start(
        with_directory_scope(
            example_store_with_settings("subagents_inherit_scopes: false\n"),
            "session_directory",
            "/start(/|$)",
        ),
        |_| {},
    );
    server.hook("alpha", SOME_TOKENS, &session_start_in(STARTED_IN));
    server.hook(
        "alpha",
        SOME_TOKENS,
        &event_with("cwd_changed", &[("cwd", json!(MOVED_TO))]),
    );
    server.hook(
        "alpha",
        SOME_TOKENS,
        &event_with("subagent_start", &[("cwd", json!(MOVED_TO))]),
    );

    let (_, in_subagent) = server.hook("alpha", SOME_TOKENS, &subagent_tool_call_from(MOVED_TO));

    assert!(
        context_of(&in_subagent).contains(BENCH_VICE_BODY),
        "the subagent's session directory must be the {STARTED_IN} its session was started in, got {:?}",
        context_of(&in_subagent)
    );
}

/// Detects a context whose first event is not a session start being left
/// without a session directory: the server misses the start of a session that
/// was already running when it came up, and such a session must still match
/// directory triggers rather than silently matching nothing.
#[test]
fn a_context_first_seen_at_a_prompt_takes_that_events_directory_as_its_session_directory() {
    let server = TestServer::start(
        with_directory_scope(example_store_files(), "session_directory", "/start(/|$)"),
        |_| {},
    );
    server.hook("alpha", SOME_TOKENS, &prompt_from(STARTED_IN));

    let (_, answer) = server.hook("alpha", SOME_TOKENS, &tool_call_from(MOVED_TO));

    assert!(
        context_of(&answer).contains(BENCH_VICE_BODY),
        "the directory of the first event seen must serve as the session directory, got {:?}",
        context_of(&answer)
    );
}

/// Detects a session directory that is not written to the context snapshot: the
/// server would come back up having forgotten where every running session
/// began, and their directory triggers would stop firing.
#[test]
fn a_restart_keeps_the_directory_each_session_was_started_in() {
    let mut server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &session_start_in(STARTED_IN));
    server.commit(
        "add a scope triggered by the session directory",
        directory_scope_files("session_directory", "/start(/|$)"),
    );

    server.restart();
    let (_, answer) = server.hook("alpha", SOME_TOKENS, &tool_call_from(MOVED_TO));

    assert!(
        context_of(&answer).contains(BENCH_VICE_BODY),
        "the session directory must survive a restart, got {:?}",
        context_of(&answer)
    );
}

/// A line that appears only in the body of the `lathe-safety` memory built
/// below, so that "the scope turned on" can be told apart from anything the
/// example store delivers on its own.
const LATHE_SAFETY_BODY: &str = "The chuck key never stays in the chuck";

/// The example store with one more scope, whose only trigger is a
/// machine-qualified `user_message` one, and one critical memory in it.
fn store_with_a_machine_qualified_message_trigger() -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = example_store_files();
    files.push((
        "scopes/lathe.yaml".to_string(),
        Some(
            b"id: lathe\ntriggers:\n- on: user_message\n  pattern: '\\blathe\\b'\n  machine: alpha\n"
                .to_vec(),
        ),
    ));
    files.push((
        "memories/lathe-safety.md".to_string(),
        Some(
            format!(
                "---\nname: lathe-safety\ndescription: The lathe is only started with the chuck key out\nmetadata:\n  kind: critical\n  scopes:\n  - lathe\n  source: user\n---\n# Lathe safety\n\n{LATHE_SAFETY_BODY}\n"
            )
            .into_bytes(),
        ),
    ));
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// Detects a machine qualifier ignored on a trigger that names a text field, and
/// one refused there: `machine` is a plain conjunct on every trigger, so a
/// message trigger written for one machine must fire on that machine and stay
/// silent on every other one, for the same message.
#[test]
fn a_machine_qualified_message_trigger_fires_only_on_its_machine() {
    let server = TestServer::start(store_with_a_machine_qualified_message_trigger(), |_| {});
    let prompt = event_with(
        "user_prompt_submit",
        &[("prompt", json!("set up the lathe"))],
    );

    let (_, on_its_machine) = server.hook("alpha", SOME_TOKENS, &prompt);
    let (_, on_another_machine) = server.hook("beta", SOME_TOKENS, &prompt);

    assert!(
        context_of(&on_its_machine).contains(LATHE_SAFETY_BODY),
        "the message trigger must fire on the machine it names, got {:?}",
        context_of(&on_its_machine)
    );
    assert!(
        !context_of(&on_another_machine).contains(LATHE_SAFETY_BODY),
        "the same message must not fire the trigger on another machine, got {:?}",
        context_of(&on_another_machine)
    );
}

/// Detects a delivery record that is lost on restart, which would repeat every
/// memory the session already holds the next time the server starts.
#[test]
fn a_restart_keeps_what_each_context_has_already_been_given() {
    let mut server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &widget_prompt());

    server.restart();
    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("stop"));

    assert_eq!(
        status, 200,
        "the first event after a restart must be answered"
    );
    assert_eq!(
        additional_context(&answer),
        None,
        "a restart must not make the session hear everything again, got {answer}"
    );
}

/// Detects a critical section that does not cover computing and recording a
/// delivery together: parallel tool calls would each be stopped, or several
/// would carry the same memory.
#[test]
fn parallel_tool_calls_on_one_context_are_stopped_exactly_once() {
    let server = TestServer::start(example_store_files(), |_| {});
    let url = server.url();
    let call = hook_fixture("pre_tool_use_bash");

    let answers: Vec<Value> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..32)
            .map(|_| {
                let url = url.clone();
                let call = call.clone();
                scope.spawn(move || {
                    let (status, answer) = post_hook(&url, "alpha", SOME_TOKENS, &call);
                    assert_eq!(status, 200, "every parallel call must be answered");
                    answer
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("the request thread finishes"))
            .collect()
    });

    let stopped = answers
        .iter()
        .filter(|answer| permission_decision(answer) == Some("deny"))
        .count();
    assert_eq!(
        stopped, 1,
        "exactly one of 32 parallel calls may be stopped, {stopped} were"
    );
    let carried = answers
        .iter()
        .filter(|answer| {
            permission_decision(answer).is_none() && context_of(answer).contains(WIDGET_NAMING_BODY)
        })
        .count();
    assert_eq!(
        carried, 0,
        "a call that was not stopped must not carry the memory, {carried} did"
    );
}

/// Detects a server that rejects an event name a newer Claude Code sends: the
/// hook would fail on every one of those events instead of being ignored.
#[test]
fn an_event_name_the_server_does_not_know_is_answered_with_an_empty_object() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("unknown_event"));

    assert_eq!(status, 200, "an unknown event must be acknowledged");
    assert_eq!(answer, json!({}), "the answer must ask for nothing");
}

/// Detects a body that is not a request being treated as one, which would make
/// a client bug look like an empty event.
#[test]
fn a_body_that_is_not_a_hook_request_is_rejected() {
    let server = TestServer::start(example_store_files(), |_| {});

    let (malformed, _) = server.post_raw("/hook", "{not json");
    let (incomplete, _) = server.post_raw("/hook", "{\"machine\": \"alpha\"}");

    assert_eq!(malformed, 400, "a body that is not JSON must be rejected");
    assert_eq!(
        incomplete, 400,
        "a body without the hook event must be rejected"
    );
}

// ------------------------------------------- what a scope itself delivers
//
// The expectations of this section come from the two tickets: a scope may carry
// a `message`, delivered under the rules of a critical memory but printed as the
// scope and its text (issue #7); and whether a scope that activates with nothing
// to deliver is named at all is the store's `announce_empty_scopes` (issue #18).

/// The message the scope of the issue carries, and one that replaces it.
const BROAD_EXCEPT_MESSAGE: &str =
    "Do not catch Exception broadly; catch the exception type the code can handle.";
const BROAD_EXCEPT_REVISED: &str =
    "Catch the exception type the code can handle, and log the rest.";

/// A scope whose only content is a message, fired by a tool input.
fn scope_with_message(message: &str) -> (String, Option<Vec<u8>>) {
    (
        "scopes/broad-except.yaml".to_string(),
        Some(
            format!(
                "id: broad-except\nmessage: '{message}'\ntriggers:\n- on: tool_input\n  \
                 pattern: 'except Exception as \\w+'\n"
            )
            .into_bytes(),
        ),
    )
}

/// A scope with a trigger and nothing to deliver: no message, and no memory of
/// the example store names it.
fn scope_with_nothing_to_deliver() -> (String, Option<Vec<u8>>) {
    (
        "scopes/paperwork.yaml".to_string(),
        Some(
            "id: paperwork\ntriggers:\n- on: tool_input\n  pattern: '\\binvoices?\\b'\n"
                .to_string()
                .into_bytes(),
        ),
    )
}

/// The example store with one more scope file, and `config.yml` as given.
fn example_store_plus(
    scope: (String, Option<Vec<u8>>),
    settings: Option<&str>,
) -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = match settings {
        Some(text) => example_store_with_settings(text),
        None => example_store_files(),
    };
    files.push(scope);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// A `PreToolUse` writing python that catches `Exception`, which the message
/// scope's trigger matches and no trigger of the example store does.
fn broad_except_call() -> Value {
    event_with(
        "pre_tool_use_bash",
        &[
            ("tool_name", json!("Edit")),
            (
                "tool_input",
                json!({
                    "file_path": "/home/dev/parser/read.py",
                    "new_string": "except Exception as error:\n    return None\n",
                }),
            ),
        ],
    )
}

/// A `PreToolUse` that turns the scope with nothing to deliver on.
fn invoice_call() -> Value {
    event_with(
        "pre_tool_use_bash",
        &[
            ("tool_name", json!("Edit")),
            (
                "tool_input",
                json!({ "file_path": "/home/dev/books/invoices.md", "new_string": "paid\n" }),
            ),
        ],
    )
}

/// Detects a scope message that is not delivered, is delivered as an ordinary
/// critical memory with its id and scope list, does not hold the call it arrived
/// at, or is delivered a second time once the context has it.
#[test]
fn a_scope_message_arrives_headed_by_its_scope_and_holds_the_call_once() {
    let server = TestServer::start(
        example_store_plus(scope_with_message(BROAD_EXCEPT_MESSAGE), None),
        |_| {},
    );
    // The session start takes the memories of the implicit scopes, so what the
    // tool call is answered with is the message alone.
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, held) = server.hook("alpha", SOME_TOKENS, &broad_except_call());

    assert_eq!(status, 200, "the tool call must be answered");
    let text = context_of(&held);
    assert!(
        has_line(text, "== scope: broad-except =="),
        "the message must be headed by its scope, got {text:?}"
    );
    assert!(
        text.contains(BROAD_EXCEPT_MESSAGE),
        "the message must arrive in full, got {text:?}"
    );
    assert!(
        !text.contains("critical: scopes/broad-except"),
        "a message must not be printed as a memory to fetch by id, got {text:?}"
    );
    assert_eq!(
        permission_decision(&held),
        Some("deny"),
        "a message the context has not seen must hold the call, got {held}"
    );

    let (_, reissued) = server.hook("alpha", SOME_TOKENS, &broad_except_call());
    assert_eq!(
        permission_decision(&reissued),
        None,
        "the reissued call must be allowed, got {reissued}"
    );
    assert!(
        !context_of(&reissued).contains(BROAD_EXCEPT_MESSAGE),
        "a message the context holds must not be delivered again, got {reissued}"
    );
}

/// Detects a message whose new text never reaches a context that was given the
/// old one: the agent would keep following a rule the store has changed.
#[test]
fn an_edited_scope_message_is_delivered_again_and_holds_the_next_call() {
    let server = TestServer::start(
        example_store_plus(scope_with_message(BROAD_EXCEPT_MESSAGE), None),
        |_| {},
    );
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &broad_except_call());

    server.commit(
        "revise the exception message",
        vec![scope_with_message(BROAD_EXCEPT_REVISED)],
    );
    let (status, answer) = server.hook("alpha", SOME_TOKENS, &broad_except_call());

    assert_eq!(status, 200, "the tool call must be answered");
    let text = context_of(&answer);
    assert!(
        text.contains(BROAD_EXCEPT_REVISED),
        "the new text must be delivered, got {text:?}"
    );
    assert_eq!(
        permission_decision(&answer),
        Some("deny"),
        "a changed message must hold the call like a changed critical memory, got {answer}"
    );
}

/// Detects a scope that activates with nothing to deliver being announced when
/// the store has not asked for it: the answer would carry a scope id at every
/// trigger that fires and would report context where there is none.
#[test]
fn an_activation_that_delivers_nothing_is_answered_with_the_empty_object_by_default() {
    let server = TestServer::start(
        example_store_plus(scope_with_nothing_to_deliver(), None),
        |_| {},
    );
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &invoice_call());

    assert_eq!(status, 200, "the tool call must be answered");
    assert_eq!(
        answer,
        json!({}),
        "an activation with nothing to deliver must ask for nothing, got {answer}"
    );
    let (_, next) = server.hook("alpha", SOME_TOKENS, &neutral_pre_tool_use());
    assert_eq!(
        permission_decision(&next),
        None,
        "nothing was delivered, so no call may be held, got {next}"
    );
}

/// Detects an announced scope treated as a delivery: it must not carry memory
/// text, must not hold the call, and must not be counted among what the store
/// has delivered to the session.
#[test]
fn an_announced_scope_is_named_alone_and_counted_as_no_delivery() {
    let server = TestServer::start(
        example_store_plus(
            scope_with_nothing_to_deliver(),
            Some("announce_empty_scopes: true\n"),
        ),
        |_| {},
    );
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    let delivered_at_start = server.stats().count_rows(Table::Deliveries);

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &invoice_call());

    assert_eq!(status, 200, "the tool call must be answered");
    let text = context_of(&answer);
    assert!(
        has_line(text, "== scopes activated ==") && has_line(text, "paperwork"),
        "the scope that activated must be named under the activation heading, got {text:?}"
    );
    assert!(
        !text.contains(BENCH_POWER_BODY) && !text.contains(READING_LIST_DESCRIPTION),
        "naming a scope must deliver no memory again, got {text:?}"
    );
    assert_eq!(
        permission_decision(&answer),
        None,
        "nothing was delivered, so the call must not be held, got {answer}"
    );
    assert_eq!(
        server.stats().count_rows(Table::Deliveries),
        delivered_at_start,
        "naming a scope is not a delivery and must add no delivery row"
    );

    let (_, next) = server.hook("alpha", SOME_TOKENS, &neutral_pre_tool_use());
    assert_eq!(
        permission_decision(&next),
        None,
        "the call after an announcement must be allowed, got {next}"
    );
}

/// Detects a scope listed as delivering nothing when it just delivered a
/// memory, which would name every scope of every event the memories already
/// speak for.
#[test]
fn a_scope_that_delivers_a_memory_is_not_also_named_as_activated() {
    let server = TestServer::start(
        example_store_with_settings("announce_empty_scopes: true\n"),
        |_| {},
    );
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let (status, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_bash"));

    assert_eq!(status, 200, "the tool call must be answered");
    let text = context_of(&answer);
    assert!(
        text.contains(WIDGET_NAMING_BODY),
        "the scope's critical memory must arrive, got {text:?}"
    );
    assert!(
        !text.contains("== scopes activated =="),
        "every scope this event turned on delivered something, so none may be named, got {text:?}"
    );
}

/// Tests of the state machine at its own interface, for the two reasons a
/// delivery is withdrawn and for the condition staleness needs, none of which
/// any single HTTP answer can distinguish yet.
mod state_machine {
    use super::*;

    /// A state that was given `id` at the store's current version, with the
    /// given scopes active and context size.
    fn delivered_state(
        catalog: &forgetmenot_server::store::catalog::Catalog,
        active: BTreeSet<ScopeId>,
        id: &MemoryId,
        form: Form,
        tokens: Option<u64>,
    ) -> ContextState {
        let version = catalog
            .memory(id)
            .expect("the memory is in the example store")
            .version
            .to_string();
        ContextState {
            active,
            delivered: BTreeMap::from([(
                id.clone(),
                Shown {
                    version,
                    form,
                    tokens,
                },
            )]),
            parent: None,
            session_directory: None,
            session_title: None,
            first_prompt: None,
            task: None,
            agent_type: None,
            last_seen: chrono::Utc::now(),
        }
    }

    /// Detects a withdrawal reported as a deletion when the memory is still in
    /// the store: the model would be told a rule was retired when the session
    /// merely stopped working in its scope.
    #[test]
    fn a_memory_no_active_scope_covers_is_withdrawn_as_out_of_scope_not_as_deleted() {
        let store = example_store();
        let catalog = store.catalog();
        let id = MemoryId::new("widget-naming");
        let state = delivered_state(
            &catalog,
            initial_active("alpha", "session-1"),
            &id,
            Form::Full,
            Some(10_000),
        );

        let needs = compute_needs(&catalog, &state, Some(10_000));

        assert_eq!(
            needs.retracted,
            vec![(id, RetractReason::NoActiveScope)],
            "a memory no active scope covers must be withdrawn as one no active scope covers"
        );
    }

    /// Detects an index line that is never sent again after the memory's text
    /// changes: the description the model holds would stay the old one.
    #[test]
    fn a_knowledge_memory_is_sent_again_when_its_version_changes() {
        let store = example_store();
        let catalog = store.catalog();
        let id = MemoryId::new("rocket-stages");
        let mut state = delivered_state(
            &catalog,
            BTreeSet::from([ScopeId::new("rocketry")]),
            &id,
            Form::Index,
            Some(10_000),
        );
        assert!(
            compute_needs(&catalog, &state, Some(10_000)).is_empty(),
            "nothing is owed while the delivered version is current"
        );

        state
            .delivered
            .get_mut(&id)
            .expect("the memory was delivered")
            .version = "0".repeat(40);
        let needs = compute_needs(&catalog, &state, Some(10_000));

        assert_eq!(
            needs.changed,
            vec![id],
            "a memory delivered at another version must count as changed"
        );
    }

    /// Detects staleness judged without knowing the context size a memory was
    /// delivered at, which would make every delivery stale at once.
    #[test]
    fn staleness_is_not_judged_without_the_context_size_of_the_delivery() {
        let store = example_store();
        let catalog = store.catalog();
        let id = MemoryId::new("bench-power");
        let state = delivered_state(
            &catalog,
            initial_active("alpha", "session-1"),
            &id,
            Form::Full,
            None,
        );

        let needs = compute_needs(&catalog, &state, Some(1_000_000));

        assert!(
            needs.stale.is_empty(),
            "without the size at delivery nothing may be called stale, got {:?}",
            needs.stale
        );
    }

    /// Detects a context key printed in a form the MCP tools cannot take, which
    /// is what the session start line tells the model to pass them.
    #[test]
    fn the_session_key_printed_for_a_subagent_names_the_agent() {
        assert_eq!(
            ContextKey::subagent("alpha", "session-1", "agent-7").to_string(),
            "alpha/session-1/agent-7"
        );
    }
}
