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
use forgetmenot_server::store::{MemoryId, ScopeId};
use serde_json::{Value, json};

use common::{
    TestServer, additional_context, example_store, example_store_files, hook_fixture,
    permission_decision, permission_decision_reason, post_hook,
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
/// The index entry of `rocket-stages`, from its `description`.
const ROCKET_STAGES_DESCRIPTION: &str =
    "Stage one lights on the pad and later stages count upwards in firing order";

/// The context size reported with an event that is not about staleness.
const SOME_TOKENS: Option<u64> = Some(10_000);

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

/// Detects a staleness threshold that fires late: a critical memory delivered K
/// tokens of context ago is out of the model's reach and must be delivered again.
#[test]
fn a_critical_memory_is_delivered_again_once_the_context_has_grown_by_the_threshold() {
    let stale_tokens = 1_000;
    let server = TestServer::start(example_store_files(), |config| {
        config.stale_tokens = stale_tokens;
    });
    server.hook("alpha", Some(10_000), &hook_fixture("session_start"));

    let (status, answer) = server.hook(
        "alpha",
        Some(10_000 + stale_tokens),
        &neutral_pre_tool_use(),
    );

    assert_eq!(status, 200, "the event must be answered");
    assert!(
        context_of(&answer).contains(BENCH_POWER_BODY),
        "a stale critical memory must be delivered again, got {answer}"
    );
}

/// Detects a staleness threshold that fires early: just short of K the memory is
/// still in the model's reach and delivering it again wastes context.
#[test]
fn a_critical_memory_is_not_delivered_again_just_short_of_the_threshold() {
    let stale_tokens = 1_000;
    let server = TestServer::start(example_store_files(), |config| {
        config.stale_tokens = stale_tokens;
    });
    server.hook("alpha", Some(10_000), &hook_fixture("session_start"));

    let (status, answer) = server.hook(
        "alpha",
        Some(10_000 + stale_tokens - 1),
        &neutral_pre_tool_use(),
    );

    assert_eq!(status, 200, "the event must be answered");
    assert_eq!(
        additional_context(&answer),
        None,
        "below the threshold nothing may be delivered again, got {answer}"
    );
}

/// Detects a compaction that leaves the delivery record standing: everything
/// delivered before it is gone from the context, so the next event has to
/// deliver it all again.
#[test]
fn everything_due_is_delivered_again_after_a_compaction() {
    let server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &hook_fixture("post_compact"));

    let (status, answer) = server.hook(
        "alpha",
        SOME_TOKENS,
        &event_with("user_prompt_submit", &[("prompt", json!("carry on"))]),
    );

    assert_eq!(status, 200, "the event after a compaction must be answered");
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
            last_seen: chrono::Utc::now(),
        }
    }

    /// Detects a withdrawal reported as a deletion when the memory is still in
    /// the store: the model would be told a rule was retired when the session
    /// merely stopped working in its scope.
    #[test]
    fn a_memory_whose_scope_is_no_longer_active_is_withdrawn_as_scope_off() {
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

        let needs = compute_needs(&catalog, &state, Some(10_000), 1_000);

        assert_eq!(
            needs.retracted,
            vec![(id, RetractReason::ScopeOff)],
            "a memory no scope covers must be withdrawn as a scope that went off"
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
            compute_needs(&catalog, &state, Some(10_000), 1_000).is_empty(),
            "nothing is owed while the delivered version is current"
        );

        state
            .delivered
            .get_mut(&id)
            .expect("the memory was delivered")
            .version = "0".repeat(40);
        let needs = compute_needs(&catalog, &state, Some(10_000), 1_000);

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

        let needs = compute_needs(&catalog, &state, Some(1_000_000), 1_000);

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
