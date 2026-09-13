//! Tests of the compiled trigger index: which triggers fire, for which
//! machine, and how long matching takes.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use forgetmenot_server::store::ScopeId;
use forgetmenot_server::store::scope::TriggerField;
use forgetmenot_server::triggers::{TriggerIndex, TriggerIndexBuilder};

use common::store_with;

/// The longest a hook may spend matching one string against every trigger in
/// the store. The hook budget is 50 ms end to end including git and rendering,
/// so matching gets a small share of it.
const MATCH_TIME_LIMIT: Duration = Duration::from_millis(20);

/// The largest tool result the hook will match, from the state machine's cap.
const LARGEST_MATCHED_TEXT: usize = 256 * 1024;

/// The number of patterns a large store is expected to hold.
const MANY_PATTERNS: usize = 200;

fn index_with(triggers: &[(&str, TriggerField, &str, Option<&str>)]) -> TriggerIndex {
    let mut builder = TriggerIndexBuilder::new();
    for (scope, field, pattern, machine) in triggers {
        builder
            .push(
                ScopeId::new(*scope),
                *field,
                pattern,
                machine.map(str::to_string),
            )
            .expect("the fixture patterns compile");
    }
    builder.build(BTreeMap::new()).expect("the index compiles")
}

fn fired_scopes(
    index: &TriggerIndex,
    field: TriggerField,
    text: &str,
    machine: &str,
) -> Vec<String> {
    index
        .fire(field, text, machine)
        .into_iter()
        .map(|hit| hit.scope.as_str().to_string())
        .collect()
}

/// Detects a trigger that never fires, which would leave its scope unreachable
/// and its memories undeliverable.
#[test]
fn a_matching_pattern_fires_and_reports_which_pattern_matched() {
    let index = index_with(&[("widgets", TriggerField::UserMessage, r"\bwidget\b", None)]);
    let hits = index.fire(
        TriggerField::UserMessage,
        "how do I size this widget?",
        "alpha",
    );
    assert_eq!(
        hits.len(),
        1,
        "the matching trigger did not fire once: {hits:?}"
    );
    assert_eq!(
        hits[0].scope.as_str(),
        "widgets",
        "the wrong scope was named"
    );
    assert_eq!(
        hits[0].pattern, r"\bwidget\b",
        "the matching pattern was not reported"
    );
    assert_eq!(
        hits[0].field,
        TriggerField::UserMessage,
        "the wrong field was reported"
    );
}

/// Detects a pattern matched loosely, for example as a substring search: a
/// scope that turns on for text its author did not intend delivers memories
/// into unrelated work and trains the reader to ignore them.
#[test]
fn a_non_matching_text_fires_nothing() {
    let index = index_with(&[("widgets", TriggerField::UserMessage, r"\bwidget\b", None)]);
    assert!(
        index
            .fire(
                TriggerField::UserMessage,
                "how do I size this bracket?",
                "alpha"
            )
            .is_empty(),
        "a trigger fired on text its pattern does not match"
    );
    assert!(
        index
            .fire(
                TriggerField::UserMessage,
                "widgets are plural here",
                "alpha"
            )
            .is_empty(),
        "a word-boundary pattern matched inside a longer word"
    );
}

/// Detects a field index that routes patterns to the wrong automaton, which
/// would match a tool result against the patterns meant for user messages.
#[test]
fn a_pattern_registered_for_one_field_does_not_fire_for_another() {
    let index = index_with(&[("widgets", TriggerField::ToolInput, r"\bwidget\b", None)]);
    assert_eq!(
        fired_scopes(
            &index,
            TriggerField::ToolInput,
            "edit the widget file",
            "alpha"
        ),
        vec!["widgets".to_string()],
        "the trigger did not fire for its own field"
    );
    for field in TriggerField::ALL {
        if field == TriggerField::ToolInput {
            continue;
        }
        assert!(
            index
                .fire(field, "edit the widget file", "alpha")
                .is_empty(),
            "a tool_input trigger fired for {field}"
        );
    }
}

/// Detects a machine qualifier that is ignored: the same path means different
/// things on different machines, so a directory trigger firing everywhere would
/// activate a scope for work that has nothing to do with it.
#[test]
fn a_machine_qualified_trigger_fires_only_for_its_machine() {
    let index = index_with(&[(
        "workshop",
        TriggerField::WorkingDirectory,
        "/workshop(/|$)",
        Some("alpha"),
    )]);
    assert_eq!(
        fired_scopes(
            &index,
            TriggerField::WorkingDirectory,
            "/home/user/workshop",
            "alpha"
        ),
        vec!["workshop".to_string()],
        "the trigger did not fire on its own machine"
    );
    assert!(
        index
            .fire(
                TriggerField::WorkingDirectory,
                "/home/user/workshop",
                "beta"
            )
            .is_empty(),
        "a trigger qualified for alpha fired for beta"
    );
}

/// Detects an unqualified trigger being restricted to some machine, which would
/// make most of a store's triggers dead on every other machine.
#[test]
fn an_unqualified_trigger_fires_for_every_machine() {
    let index = index_with(&[("widgets", TriggerField::UserMessage, r"\bwidget\b", None)]);
    for machine in ["alpha", "beta"] {
        assert_eq!(
            fired_scopes(&index, TriggerField::UserMessage, "the widget", machine),
            vec!["widgets".to_string()],
            "an unqualified trigger did not fire for {machine}"
        );
    }
}

/// Detects a catalog that builds the trigger index without the implies closure,
/// which would turn a scope on without the scopes it declares it needs.
#[test]
fn fire_closed_adds_the_scopes_the_fired_scope_implies() {
    let store = store_with(&[
        (
            "scopes/widgets.yaml",
            b"id: widgets\nimplies: [rocketry]\ntriggers:\n  - on: user_message\n    pattern: '\\bwidget\\b'\n",
        ),
        (
            "scopes/rocketry.yaml",
            b"id: rocketry\nimplies: [metrology]\n",
        ),
        ("scopes/metrology.yaml", b"id: metrology\n"),
    ]);
    let catalog = store.catalog();
    let activated =
        catalog
            .triggers()
            .fire_closed(TriggerField::UserMessage, "size this widget", "alpha");
    assert_eq!(
        activated,
        BTreeSet::from([
            ScopeId::new("widgets"),
            ScopeId::new("rocketry"),
            ScopeId::new("metrology"),
        ]),
        "the fired scope did not close over what it implies"
    );
}

/// Detects matching that costs more than one pass over the text, for example a
/// loop that runs each pattern separately or a pattern engine that backtracks.
/// At 200 patterns and a 256 KiB tool result that is the difference between a
/// hook the harness waits on and one it does not notice.
///
/// The budget is wide enough for an unoptimised build of the regex crate and
/// narrow enough that per-pattern matching, which would be two orders of
/// magnitude slower here, cannot fit inside it.
#[test]
fn matching_a_large_text_against_many_patterns_stays_within_the_time_limit() {
    let mut builder = TriggerIndexBuilder::new();
    for number in 0..MANY_PATTERNS {
        builder
            .push(
                ScopeId::new(format!("scope-{number}")),
                TriggerField::ToolResult,
                &format!(r"\bmarker{number}\b"),
                None,
            )
            .expect("the generated patterns compile");
    }
    let index = builder.build(BTreeMap::new()).expect("the index compiles");

    let mut text = String::with_capacity(LARGEST_MATCHED_TEXT + 64);
    while text.len() < LARGEST_MATCHED_TEXT {
        text.push_str("ordinary prose about brackets and measurements and nothing else. ");
    }
    // One match, so the time measured includes collecting a hit rather than
    // only the fast path of a text that matches nothing.
    text.push_str(" marker7 ");

    let started = Instant::now();
    let hits = index.fire(TriggerField::ToolResult, &text, "alpha");
    let elapsed = started.elapsed();

    assert_eq!(
        hits.len(),
        1,
        "the one planted match was not found, so the timing means nothing: {hits:?}"
    );
    println!(
        "matched {} KiB against {MANY_PATTERNS} patterns in {elapsed:?}",
        text.len() / 1024
    );
    assert!(
        elapsed < MATCH_TIME_LIMIT,
        "matching {} KiB against {MANY_PATTERNS} patterns took {elapsed:?}, over the {MATCH_TIME_LIMIT:?} limit",
        text.len() / 1024
    );
}
