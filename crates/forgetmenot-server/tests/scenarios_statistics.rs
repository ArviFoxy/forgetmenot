//! What a session cost, read back through the statistics.
//!
//! The rule the whole report rests on is that the cost of anything is the length
//! of the text the model was sent for it, recorded when the answer was sent.
//! Only a whole session shows that: the answers are what the model was given,
//! the log is what the server wrote down about them, and the two have to be the
//! same text measured the same way. What one query does with one window is
//! tested beside the queries and behind the routes.

mod common;
mod scenario;

use std::collections::BTreeMap;

use forgetmenot_server::stats::tokens_of;
use scenario::{Answer, Store, World};
use serde_json::Value;

/// The machine the session runs on, which is the machine the example store's
/// session memories are written for.
const ALPHA: &str = "alpha";

/// A directory that matches no trigger of the example store, so every scope the
/// session works in was turned on by the session itself.
const NEUTRAL_DIRECTORY: &str = "/home/dev/notes";

/// A prompt naming a widget, which turns the example store's `widgets` scope on
/// and delivers what that makes due.
const NAMES_A_WIDGET: &str = "check the widget numbering before we continue";

/// Detects statistics that report a cost the model was never sent: a scope
/// charged for text printed under another scope, a memory's cost taken from the
/// catalog rather than from the answer, a summary or a series that sums
/// something other than the delivery rows, or a token figure that does not go
/// through the store's own divisor.
///
/// Expectation source: the answers themselves. Every figure below is checked
/// against the text the session was actually given, measured the way Claude Code
/// measures a hook answer, and against the store's `characters_per_token`.
#[test]
fn the_tokens_a_session_costs_are_the_ones_the_statistics_report() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    let start = session.start_in(NEUTRAL_DIRECTORY);
    let prompt = session.prompt(NAMES_A_WIDGET);
    let sent: Vec<u64> = [&start, &prompt]
        .iter()
        .map(|answer| answer.chars() as u64)
        .collect();
    assert!(
        sent.iter().all(|chars| *chars > 0),
        "both events must have carried text, else there is nothing to account for: {sent:?}"
    );

    let divisor = characters_per_token(&world);
    let rows = delivery_rows(&world);
    let charged: u64 = rows.iter().map(|(_, chars)| chars).sum();
    let whole: u64 = sent.iter().sum();
    assert!(
        charged > 0 && charged <= whole,
        "what the scopes were charged is part of what the model was sent and never more than \
         it, got {charged} against {whole}"
    );

    // The summary is what the page's first row of numbers reads.
    let summary = world.stats_summary();
    let five_minutes = summary["windows"]
        .as_array()
        .unwrap_or_else(|| panic!("the summary carries its windows: {summary}"))
        .iter()
        .find(|row| row["name"] == "5m")
        .unwrap_or_else(|| panic!("the summary reports the five-minute window: {summary}"))
        .clone();
    assert_eq!(
        five_minutes["tokens"].as_u64(),
        Some(tokens_of(charged, divisor)),
        "the summary must report the delivered text of these two answers, got {five_minutes}"
    );

    // The series is the same text bucketed by time, so over a log that fits one
    // bucket it is the same number.
    let series = world.stats_series("");
    let points = series["points"]
        .as_array()
        .unwrap_or_else(|| panic!("the series carries its points: {series}"));
    assert_eq!(
        points
            .iter()
            .map(|point| point["tokens"].as_u64().unwrap_or(0))
            .sum::<u64>(),
        tokens_of(charged, divisor),
        "the series must carry the same text the summary does, got {series}"
    );

    // Every scope is charged its own sections, and the sections together are
    // everything that was charged.
    let mut per_scope: BTreeMap<String, u64> = BTreeMap::new();
    for (scope, chars) in &rows {
        if !scope.is_empty() {
            *per_scope.entry(scope.clone()).or_default() += chars;
        }
    }
    assert_eq!(
        per_scope.values().sum::<u64>(),
        charged,
        "every delivered character must belong to exactly one scope, got {per_scope:?}"
    );
    let reported = world.stats_scopes("");
    for (scope, chars) in &per_scope {
        let row = reported
            .iter()
            .find(|row| row["scope_id"] == scope.as_str())
            .unwrap_or_else(|| panic!("the scope {scope} must have a row: {reported:?}"));
        assert_eq!(
            row["tokens"].as_u64(),
            Some(tokens_of(*chars, divisor)),
            "the scope {scope} must be charged the text printed under it, got {row}"
        );
    }
    let scope_tokens: u64 = reported
        .iter()
        .map(|row| row["tokens"].as_u64().unwrap_or(0))
        .sum();
    assert!(
        scope_tokens <= tokens_of(whole, divisor) + reported.len() as u64,
        "the scopes together must not be charged more than the model was sent, got \
         {scope_tokens} against {whole} characters"
    );

    // The per-session series is the other half: what each answer as a whole
    // cost, overhead and all, beside the size Claude Code reported.
    let events = session_series(&world, &session.key());
    assert_eq!(
        events
            .iter()
            .map(|row| row["tokens"].as_u64().unwrap_or(0))
            .collect::<Vec<_>>(),
        sent.iter()
            .map(|chars| tokens_of(*chars, divisor))
            .collect::<Vec<_>>(),
        "each event must report the cost of the answer the model was given at it, got {events:?}"
    );
}

/// Detects a session's own answers charged to another session, which would make
/// a per-session figure meaningless: the two sessions here are given the same
/// memories and only one of them is asked about.
///
/// Expectation source: the answers of the second session alone.
#[test]
fn a_sessions_figures_cover_its_own_answers_and_no_other_sessions() {
    let world = World::new().store(Store::example()).build();
    let claude = world.claude(ALPHA);
    let first = claude.session_named("session-1");
    first.start_in(NEUTRAL_DIRECTORY);

    let second = claude.session_named("session-2");
    let start = second.start_in(NEUTRAL_DIRECTORY);
    let prompt = second.prompt(NAMES_A_WIDGET);

    let divisor = characters_per_token(&world);
    let events = session_series(&world, &second.key());
    assert_eq!(
        events
            .iter()
            .map(|row| row["tokens"].as_u64().unwrap_or(0))
            .collect::<Vec<_>>(),
        [&start, &prompt]
            .iter()
            .map(|answer: &&Answer<'_>| tokens_of(answer.chars() as u64, divisor))
            .collect::<Vec<_>>(),
        "the series must be this session's own answers and nothing of the other's, got {events:?}"
    );

    let narrowed = world.stats_series(&format!("session={}", second.key()));
    let whole = world.stats_series("");
    let tokens = |answer: &Value| {
        answer["points"]
            .as_array()
            .map(|points| {
                points
                    .iter()
                    .map(|point| point["tokens"].as_u64().unwrap_or(0))
                    .sum::<u64>()
            })
            .unwrap_or(0)
    };
    assert!(
        tokens(&narrowed) > 0 && tokens(&narrowed) < tokens(&whole),
        "one session's text must be part of the log and not all of it, got {narrowed} against \
         {whole}"
    );
}

/// The store's divisor, read through the settings the way anything outside the
/// server reads it.
fn characters_per_token(world: &World) -> f64 {
    let (status, body) = world.api("GET", "/api/settings", None);
    assert_eq!(status, 200, "the settings must be readable, got {body}");
    body["settings"]["characters_per_token"]
        .as_f64()
        .unwrap_or_else(|| panic!("the store sets a divisor: {body}"))
}

/// Every delivery the log recorded, as the scope it was printed under and the
/// characters printed for it.
///
/// Read memory by memory through the delivery log, over the memories the
/// statistics report as delivered, so the rows are the ones the server wrote
/// rather than a list this test keeps.
fn delivery_rows(world: &World) -> Vec<(String, u64)> {
    let (status, memories) = world.api("GET", "/api/stats/memories", None);
    assert_eq!(status, 200, "the memories must be readable, got {memories}");
    memories
        .as_array()
        .unwrap_or_else(|| panic!("the report is a list of rows: {memories}"))
        .iter()
        .filter_map(|row| row["memory"].as_str())
        .flat_map(|memory| world.deliveries_of(memory))
        .map(|row| {
            (
                row["scope"].as_str().unwrap_or_default().to_string(),
                row["chars"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

/// The per-event series of one context, as the page reads it.
fn session_series(world: &World, key: &str) -> Vec<Value> {
    let (status, body) = world.api("GET", &format!("/api/stats/session/{key}/series"), None);
    assert_eq!(
        status, 200,
        "the series of {key} must be readable, got {body}"
    );
    body.as_array()
        .unwrap_or_else(|| panic!("the series is a list of events: {body}"))
        .clone()
}
