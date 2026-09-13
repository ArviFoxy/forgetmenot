//! Scenario replays: ordered sequences of hook events, session operations and
//! store commits across two machines, two sessions and subagents, each step
//! with the deliveries and the decision the state machine implies.
//!
//! A scenario is a YAML file under `tests/fixtures/scenarios/`. Every file in
//! that directory is replayed, so a new property is a new file and no change
//! here. Each file carries the reasoning for its expectations in its own
//! `reasoning` field: the expectations are written by hand from the plan's state
//! machine and the example store, never captured from a run, which is what makes
//! a replay evidence of anything.
//!
//! What a step is checked against is deliberately not the renderer's section
//! labels, which the plan calls wording rather than contract. A memory counts as
//! delivered in full when the rendered text carries the lines of its body, as an
//! index line when one line carries its id and its description without its body,
//! and as withdrawn when one line carries its id and the reason. All three are
//! read out of the store at the moment of the check, so rewording a label, or
//! reordering the sections, leaves every scenario green while a body that never
//! arrives, or arrives in the wrong form, turns one red.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use forgetmenot_server::context::ContextKey;
use forgetmenot_server::operations;
use forgetmenot_server::store::catalog::Catalog;
use forgetmenot_server::store::{MemoryId, ScopeId};
use serde::Deserialize;
use serde_json::Value;

use common::{TestServer, additional_context, example_store_files, permission_decision};

// ---------------------------------------------------------------------------
// The scenario format
// ---------------------------------------------------------------------------

/// One replay. `deny_unknown_fields` everywhere in this format on purpose: a
/// misspelled key would otherwise drop an expectation silently and leave a
/// scenario that can never fail.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    /// Names the property; must equal the file stem so a failure names the file.
    name: String,
    /// Why each step expects what it expects, written from the state machine.
    reasoning: String,
    /// The stale threshold K this scenario runs with, when it is about staleness.
    #[serde(default)]
    stale_tokens: Option<u64>,
    steps: Vec<Step>,
}

/// One step: exactly one action, and optionally what the answer must be.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    #[serde(default)]
    hook: Option<HookStep>,
    #[serde(default)]
    operation: Option<OperationStep>,
    #[serde(default)]
    commit: Option<CommitStep>,
    #[serde(default)]
    expect: Option<Expect>,
}

/// One hook event, as the client POSTs it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookStep {
    machine: String,
    /// The context size the client read from the transcript; absent is the case
    /// where no transcript was readable.
    #[serde(default)]
    context_tokens: Option<u64>,
    /// The hook payload, exactly as Claude Code writes it.
    event: Value,
}

/// One call of the operations layer, which is what the MCP session tools are
/// adapters over; the MCP wire itself is tested elsewhere.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationStep {
    name: OperationName,
    /// The calling context, as `<machine>/<session-id>[/<agent-id>]`.
    session_key: String,
    /// The scope, for `session_scope_on` and `session_scope_off`.
    #[serde(default)]
    scope: Option<String>,
    /// The session inherited from, for `session_inherit`.
    #[serde(default)]
    from: Option<String>,
    /// The memory, for `memory_get`.
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OperationName {
    SessionScopeOn,
    SessionScopeOff,
    SessionInherit,
    MemoryGet,
}

/// A write to the store made outside the context under test, as a person with a
/// shell makes it: one file written, or one file removed when `content` is
/// absent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitStep {
    /// The repository path the commit touches.
    path: String,
    /// The file's new text; absent removes the file.
    #[serde(default)]
    content: Option<String>,
}

/// What the answer to a step must be.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    decision: Decision,
    /// The memories whose bodies the answer must carry, and no others.
    #[serde(default)]
    full: Vec<String>,
    /// The memories the answer must name with their description, and no others.
    #[serde(default)]
    index: Vec<String>,
    /// The withdrawals the answer must report, and no others.
    #[serde(default)]
    retracted: Vec<RetractedExpectation>,
    /// The scopes the answer must offer as ones the session can turn on, and no
    /// others. Only a session start offers any, so this is absent elsewhere.
    #[serde(default)]
    available: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetractedExpectation {
    id: String,
    /// `deleted` or `scope off`.
    reason: String,
}

/// What the answer does about the event.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Decision {
    /// The tool call is stopped.
    Deny,
    /// The call goes ahead and text is injected.
    Context,
    /// The call goes ahead and nothing is injected.
    None,
}

impl Decision {
    fn describe(&self) -> &'static str {
        match self {
            Decision::Deny => "deny",
            Decision::Context => "context",
            Decision::None => "none",
        }
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

impl Step {
    /// What the step does, for a failure message.
    fn describe(&self) -> String {
        if let Some(hook) = &self.hook {
            let event = hook.event.get("hook_event_name").and_then(Value::as_str);
            let session = hook.event.get("session_id").and_then(Value::as_str);
            return format!(
                "hook {} on {}/{}",
                event.unwrap_or("?"),
                hook.machine,
                session.unwrap_or("?")
            );
        }
        if let Some(operation) = &self.operation {
            return format!(
                "operation {:?} on {}",
                operation.name, operation.session_key
            );
        }
        if let Some(commit) = &self.commit {
            return match commit.content {
                Some(_) => format!("commit {}", commit.path),
                None => format!("remove {}", commit.path),
            };
        }
        "an empty step".to_string()
    }
}

/// Replay one scenario on its own server and store, and report the first step
/// whose answer is not what the scenario says it must be.
fn run_scenario(scenario: &Scenario) -> Result<(), String> {
    let stale_tokens = scenario.stale_tokens;
    let server = TestServer::start(example_store_files(), |config| {
        if let Some(stale_tokens) = stale_tokens {
            config.stale_tokens = stale_tokens;
        }
    });
    if scenario.reasoning.trim().is_empty() {
        return Err("the scenario must say why its expectations are what they are".to_string());
    }

    for (index, step) in scenario.steps.iter().enumerate() {
        let described = step.describe();
        let answer = apply(&server, step)
            .map_err(|problem| format!("step {index} ({described}): {problem}"))?;
        let Some(expectation) = &step.expect else {
            continue;
        };
        let Some(answer) = answer else {
            return Err(format!(
                "step {index} ({described}): only a hook step has an answer to expect anything of"
            ));
        };
        check(&server.store().catalog(), expectation, &answer)
            .map_err(|problem| format!("step {index} ({described}): {problem}"))?;
    }
    Ok(())
}

/// Carry out one step, returning the hook answer when it was a hook step.
fn apply(server: &TestServer, step: &Step) -> Result<Option<Value>, String> {
    match (&step.hook, &step.operation, &step.commit) {
        (Some(hook), None, None) => {
            let (status, answer) = server.hook(&hook.machine, hook.context_tokens, &hook.event);
            if status != 200 {
                return Err(format!("the hook answered HTTP {status}: {answer}"));
            }
            Ok(Some(answer))
        }
        (None, Some(operation), None) => {
            run_operation(server, operation)?;
            Ok(None)
        }
        (None, None, Some(commit)) => {
            run_commit(server, commit)?;
            Ok(None)
        }
        _ => Err("a step must carry exactly one of hook, operation and commit".to_string()),
    }
}

/// Call the operations layer for one step.
fn run_operation(server: &TestServer, operation: &OperationStep) -> Result<(), String> {
    let state = server.state();
    let key = parse_context_key(&operation.session_key)?;
    let scope = || -> Result<ScopeId, String> {
        operation
            .scope
            .as_deref()
            .map(ScopeId::new)
            .ok_or_else(|| format!("{:?} needs a scope", operation.name))
    };
    let outcome = match operation.name {
        OperationName::SessionScopeOn => server
            .run(operations::session_scope_on(&state, &key, &scope()?))
            .map(|_| ()),
        OperationName::SessionScopeOff => server
            .run(operations::session_scope_off(&state, &key, &scope()?))
            .map(|_| ()),
        OperationName::SessionInherit => {
            let from = operation
                .from
                .as_deref()
                .ok_or_else(|| "session_inherit needs a from".to_string())?;
            let from = parse_context_key(from)?;
            server
                .run(operations::session_inherit(&state, &key, &from))
                .map(|_| ())
        }
        OperationName::MemoryGet => {
            let id = operation
                .id
                .as_deref()
                .ok_or_else(|| "memory_get needs an id".to_string())?;
            server
                .run(operations::memory_get(
                    &state,
                    &MemoryId::new(id),
                    Some(&key),
                ))
                .map(|_| ())
        }
    };
    outcome.map_err(|error| format!("the operation was refused: {error}"))
}

/// Put one write into the store for a step.
fn run_commit(server: &TestServer, commit: &CommitStep) -> Result<(), String> {
    let path = commit.path.clone();
    match &commit.content {
        Some(content) => server.commit(
            &format!("scenario: write {path}"),
            vec![(path.clone(), Some(content.clone().into_bytes()))],
        ),
        None => server.commit(
            &format!("scenario: remove {path}"),
            vec![(path.clone(), None)],
        ),
    }
    Ok(())
}

/// A context key from the form the MCP tools take.
fn parse_context_key(text: &str) -> Result<ContextKey, String> {
    match text.split('/').collect::<Vec<&str>>().as_slice() {
        [machine, session] => Ok(ContextKey::main(*machine, *session)),
        [machine, session, agent] => Ok(ContextKey::subagent(*machine, *session, *agent)),
        _ => Err(format!(
            "{text:?} is not a session key of the form machine/session-id[/agent-id]"
        )),
    }
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// What a hook answer says about one memory, read out of the rendered text
/// without depending on any section label.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    /// Every non-empty line of the memory's body appears in the text.
    full: bool,
    /// One line carries the memory's id and its description, and its body does
    /// not appear, which is what an index line is.
    index: bool,
    /// The reason a line gives for withdrawing the memory, if any.
    retracted: Option<String>,
}

/// The two reasons a delivery is withdrawn. Source: the plan's MCP section,
/// where the retracted line says which of the two happened.
const RETRACT_REASONS: [&str; 2] = ["deleted", "scope off"];

/// What the text says about the memory `entry`.
fn seen_in(text: &str, entry: &forgetmenot_server::store::catalog::MemoryEntry) -> Seen {
    seen_for(
        text,
        entry.id.as_str(),
        entry.document.description(),
        &entry.document.body,
    )
}

/// What the text says about a memory with this id, description and body.
///
/// Taken apart from the catalog entry because a deleted memory has none: it can
/// still be named in a withdrawal, and that is the one thing left to read about
/// it.
fn seen_for(text: &str, id: &str, description: &str, body: &str) -> Seen {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let body_lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let full = !body_lines.is_empty() && body_lines.iter().all(|line| lines.contains(line));
    let index = !description.is_empty()
        && lines
            .iter()
            .any(|line| line.contains(id) && line.contains(description));
    let retracted = RETRACT_REASONS
        .iter()
        .find(|reason| {
            lines
                .iter()
                .any(|line| line.contains(id) && line.contains(**reason))
        })
        .map(|reason| (*reason).to_string());
    Seen {
        full,
        index,
        retracted,
    }
}

/// The scope ids the answer offers as ones the session can turn on: the ids of
/// the store's scopes that appear as a line of their own.
fn available_in(text: &str, catalog: &Catalog) -> BTreeSet<String> {
    let lines: BTreeSet<&str> = text.lines().map(str::trim).collect();
    catalog
        .scopes()
        .map(|scope| scope.id.as_str().to_string())
        .filter(|id| lines.contains(id.as_str()))
        .collect()
}

/// Check one answer against one expectation.
fn check(catalog: &Catalog, expectation: &Expect, answer: &Value) -> Result<(), String> {
    let text = additional_context(answer).unwrap_or_default();

    let decision = match (permission_decision(answer), text.is_empty()) {
        (Some("deny"), _) => Decision::Deny,
        (None, false) => Decision::Context,
        (None, true) => Decision::None,
        (Some(other), _) => return Err(format!("the answer carries the decision {other:?}")),
    };
    if decision != expectation.decision {
        return Err(format!(
            "decision: expected {}, got {} in {answer}",
            expectation.decision.describe(),
            decision.describe()
        ));
    }

    // A memory expected to be delivered that is not in the store would make the
    // expectations below vacuous, so it is a failure of the scenario rather than
    // of the server. A withdrawal is the one expectation that may name a memory
    // the store no longer has, because that is what a deletion leaves.
    let named = expectation.full.iter().chain(expectation.index.iter());
    for id in named {
        if catalog.memory(&MemoryId::new(id.clone())).is_none() {
            return Err(format!(
                "the scenario names {id}, which is not in the store"
            ));
        }
    }
    for entry in &expectation.retracted {
        if !RETRACT_REASONS.contains(&entry.reason.as_str()) {
            return Err(format!(
                "the scenario gives the reason {:?}, which is not one of {RETRACT_REASONS:?}",
                entry.reason
            ));
        }
        // Checked here for a memory the store no longer has; one still in the
        // store is checked with everything else below.
        if catalog.memory(&MemoryId::new(entry.id.clone())).is_none() {
            let seen = seen_for(text, &entry.id, "", "");
            if seen.retracted.as_deref() != Some(entry.reason.as_str()) {
                return Err(format!(
                    "retracted: {} is reported as {:?}, expected {:?}; the text was {text:?}",
                    entry.id, seen.retracted, entry.reason
                ));
            }
        }
    }

    for entry in catalog.memories() {
        let id = entry.id.as_str().to_string();
        let seen = seen_in(text, entry);
        let wanted_full = expectation.full.contains(&id);
        let wanted_index = expectation.index.contains(&id);
        let wanted_reason = expectation
            .retracted
            .iter()
            .find(|wanted| wanted.id == id)
            .map(|wanted| wanted.reason.clone());

        if seen.full != wanted_full {
            return Err(format!(
                "full: {id} {} the answer's text, which was {text:?}",
                if wanted_full {
                    "must arrive in full in"
                } else {
                    "must not have its body in"
                }
            ));
        }
        if seen.index != wanted_index {
            return Err(format!(
                "index: {id} {} the answer's text, which was {text:?}",
                if wanted_index {
                    "must be named with its description in"
                } else {
                    "must not be named with its description in"
                }
            ));
        }
        if seen.retracted != wanted_reason {
            return Err(format!(
                "retracted: {id} is reported as {:?}, expected {:?}; the text was {text:?}",
                seen.retracted, wanted_reason
            ));
        }
    }

    if let Some(wanted) = &expectation.available {
        for id in wanted {
            if catalog.scope(&ScopeId::new(id.clone())).is_none() {
                return Err(format!(
                    "the scenario names the scope {id}, which is not in the store"
                ));
            }
        }
        let wanted: BTreeSet<String> = wanted.iter().cloned().collect();
        let offered = available_in(text, catalog);
        if offered != wanted {
            return Err(format!(
                "available: expected the scopes {wanted:?} to be offered, got {offered:?}; \
                 the text was {text:?}"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Every scenario file, in name order.
fn scenario_files() -> Vec<PathBuf> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scenarios");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("reading {} failed: {error}", directory.display()))
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no scenarios in {}; this test would then assert nothing",
        directory.display()
    );
    files
}

fn parse_scenario(text: &str) -> Result<Scenario, String> {
    yaml_serde::from_str(text).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// Detects every multi-event property the scenario files state: the decision and
/// the deliveries after a sequence that no single event can show. Each scenario
/// names the failure it detects in its own `reasoning`.
///
/// Every scenario is replayed even when an earlier one failed, and every failure
/// is reported, so one broken property does not hide the others.
#[test]
fn every_scenario_replays_with_the_decisions_and_deliveries_the_state_machine_implies() {
    let mut failures = Vec::new();
    let mut names = BTreeSet::new();

    for path in scenario_files() {
        let stem = path
            .file_stem()
            .expect("a file with an extension has a stem")
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {} failed: {error}", path.display()));
        let scenario = match parse_scenario(&text) {
            Ok(scenario) => scenario,
            Err(problem) => {
                failures.push(format!("{stem}: the scenario does not parse: {problem}"));
                continue;
            }
        };
        if scenario.name != stem {
            failures.push(format!(
                "{stem}: the scenario calls itself {:?}, so a failure would name the wrong file",
                scenario.name
            ));
        }
        if !names.insert(scenario.name.clone()) {
            failures.push(format!("{stem}: two scenarios share this name"));
        }
        if let Err(problem) = run_scenario(&scenario) {
            failures.push(format!("{}: {problem}", scenario.name));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of the replayed scenarios did not hold:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Detects a harness that reads a rendered answer wrongly: if "delivered in
/// full" and "named in an index line" were not told apart, or a withdrawal were
/// read as a delivery, every scenario would pass on a renderer that sends the
/// wrong form. This is the harness checked against a real answer once.
///
/// Expectation source: the state machine. A session start on `alpha/session-1`
/// is owed the global critical memory in full, the two knowledge memories due to
/// it as index lines, nothing withdrawn, and the three scopes of the example
/// store as ones it can turn on.
#[test]
fn the_harness_reads_a_real_answer_the_way_the_state_machine_describes_it() {
    let server = TestServer::start(example_store_files(), |_| {});
    let (status, answer) = server.hook(
        "alpha",
        Some(40_000),
        &serde_json::json!({
            "hook_event_name": "SessionStart",
            "session_id": "session-1",
            "cwd": "/home/dev/widgets",
            "source": "startup"
        }),
    );
    assert_eq!(status, 200, "the session start must be answered");
    let catalog = server.store().catalog();
    let text = additional_context(&answer).unwrap_or_default();

    let full: BTreeSet<&str> = catalog
        .memories()
        .filter(|entry| seen_in(text, entry).full)
        .map(|entry| entry.id.as_str())
        .collect();
    let index: BTreeSet<&str> = catalog
        .memories()
        .filter(|entry| seen_in(text, entry).index)
        .map(|entry| entry.id.as_str())
        .collect();
    let retracted: Vec<&str> = catalog
        .memories()
        .filter(|entry| seen_in(text, entry).retracted.is_some())
        .map(|entry| entry.id.as_str())
        .collect();

    assert_eq!(
        full,
        BTreeSet::from(["bench-power"]),
        "only the global critical memory arrives in full, got {text:?}"
    );
    assert_eq!(
        index,
        BTreeSet::from(["reading-list", "sessions/alpha/session-1/notes"]),
        "the two knowledge memories due arrive as index lines, got {text:?}"
    );
    assert!(
        retracted.is_empty(),
        "nothing is withdrawn at a session start, got {retracted:?}"
    );
    assert_eq!(
        available_in(text, &catalog),
        BTreeSet::from([
            "rocketry".to_string(),
            "widgets".to_string(),
            "workshop".to_string()
        ]),
        "every scope of the example store can be turned on, got {text:?}"
    );
}

/// Detects a harness that reports success whatever the server answered, which
/// would make every scenario file decoration. The scenario below is wrong on
/// purpose in two ways at once, and the failure has to name the step and the
/// field.
#[test]
fn the_harness_reports_a_step_whose_expectation_does_not_hold() {
    let wrong_decision = parse_scenario(
        "name: wrong-decision\n\
         reasoning: a session start delivers the global critical memory, so \"none\" is wrong\n\
         steps:\n\
         - hook:\n    \
             machine: alpha\n    \
             context_tokens: 40000\n    \
             event:\n      \
               hook_event_name: SessionStart\n      \
               session_id: session-1\n      \
               cwd: /home/dev/widgets\n      \
               source: startup\n  \
           expect:\n    \
             decision: none\n",
    )
    .expect("the scenario parses");

    let problem = run_scenario(&wrong_decision).expect_err("a wrong expectation must be reported");

    assert!(
        problem.contains("step 0"),
        "the failure must name the step, got {problem:?}"
    );
    assert!(
        problem.contains("decision"),
        "the failure must name the field that differs, got {problem:?}"
    );
}

/// Detects an expectation that names a memory the store does not hold: it would
/// otherwise be checked against nothing, and a scenario with a typo in every id
/// would pass on a server that delivers nothing at all.
#[test]
fn the_harness_rejects_an_expectation_naming_a_memory_the_store_does_not_hold() {
    let scenario = parse_scenario(
        "name: unknown-memory\n\
         reasoning: the id is misspelled on purpose\n\
         steps:\n\
         - hook:\n    \
             machine: alpha\n    \
             context_tokens: 40000\n    \
             event:\n      \
               hook_event_name: SessionStart\n      \
               session_id: session-1\n      \
               cwd: /home/dev/widgets\n      \
               source: startup\n  \
           expect:\n    \
             decision: context\n    \
             full: [bench-powr]\n",
    )
    .expect("the scenario parses");

    let problem = run_scenario(&scenario).expect_err("an unknown memory must be reported");

    assert!(
        problem.contains("bench-powr"),
        "the failure must name the id that is not in the store, got {problem:?}"
    );
}

/// Detects a format that ignores keys it does not know: `expected:` instead of
/// `expect:` would leave a scenario that replays events and checks nothing, and
/// it would look exactly like a passing one.
#[test]
fn the_harness_rejects_a_scenario_with_a_key_it_does_not_know() {
    let problem = parse_scenario(
        "name: misspelled-key\n\
         reasoning: expect is misspelled on purpose\n\
         steps:\n\
         - hook:\n    \
             machine: alpha\n    \
             context_tokens: 40000\n    \
             event:\n      \
               hook_event_name: Stop\n      \
               session_id: session-1\n  \
           expected:\n    \
             decision: none\n",
    )
    .expect_err("a key the format does not know must be reported");

    assert!(
        problem.contains("expected"),
        "the failure must name the key, got {problem:?}"
    );
}

/// Detects a harness keyed on the renderer's section labels. The plan calls the
/// labels wording and not contract, so rewording them is a change the spec
/// permits and every scenario has to stay green through it; a harness that
/// matched on `== critical:` would turn the whole suite red on a reworded label
/// and say nothing about delivery.
///
/// The text below carries the same deliveries as a real answer under different
/// labels and different line markers, built from the bodies and descriptions of
/// the example store.
#[test]
fn the_harness_reads_deliveries_whatever_the_section_labels_say() {
    let store = common::example_store();
    let catalog = store.catalog();
    let delivered_in_full = catalog
        .memory(&MemoryId::new("bench-power"))
        .expect("the example store holds bench-power");
    let named_in_an_index_line = catalog
        .memory(&MemoryId::new("reading-list"))
        .expect("the example store holds reading-list");
    let withdrawn = catalog
        .memory(&MemoryId::new("widget-naming"))
        .expect("the example store holds widget-naming");

    let reworded = format!(
        "forgetmenot for alpha/session-1\n\
         >>> rule {} <<<\n{}\n\
         >>> worth knowing <<<\n* {}. {}\n\
         >>> no longer in force <<<\n* {} -- deleted\n\
         >>> you may also work in <<<\nwidgets\n",
        delivered_in_full.id,
        delivered_in_full.document.body.trim_end(),
        named_in_an_index_line.id,
        named_in_an_index_line.document.description(),
        withdrawn.id,
    );

    assert_eq!(
        seen_in(&reworded, delivered_in_full),
        Seen {
            full: true,
            index: false,
            retracted: None
        },
        "a body under another label is still a delivery in full"
    );
    assert_eq!(
        seen_in(&reworded, named_in_an_index_line),
        Seen {
            full: false,
            index: true,
            retracted: None
        },
        "an id and a description on one line under another label is still an index line"
    );
    assert_eq!(
        seen_in(&reworded, withdrawn),
        Seen {
            full: false,
            index: false,
            retracted: Some("deleted".to_string())
        },
        "a withdrawal under another label is still a withdrawal"
    );
    assert_eq!(
        available_in(&reworded, &catalog),
        BTreeSet::from(["widgets".to_string()]),
        "a scope offered under another label is still offered"
    );
}
