//! Tests of the statistics written on the hot path.
//!
//! Only what the write path guarantees is asserted: one row per hook event, one
//! row per delivery with the form and the reason it was delivered for, and one
//! row per trigger that fired. The aggregate queries come with the statistics
//! API.

mod common;

use forgetmenot_server::stats::Table;
use serde_json::json;

use common::{TestServer, example_store_files, hook_fixture, permission_decision};

/// The context size reported with both events of the sequence.
const SOME_TOKENS: Option<u64> = Some(10_000);

/// Detects hook events that are not logged at all, or logged more than once:
/// every later count of denies, latencies and deliveries is derived from these
/// rows.
#[test]
fn every_answered_hook_event_is_logged_once() {
    let server = TestServer::start(example_store_files(), |_| {});

    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_bash"));

    let stats = server.stats();
    assert_eq!(
        stats.count_rows(Table::HookEvents),
        2,
        "the two events answered must be the two rows logged"
    );
    assert_eq!(
        stats.decisions(),
        vec!["context".to_string(), "deny".to_string()],
        "the session start attached context and the tool call was stopped"
    );
}

/// Detects deliveries logged without the distinction the statistics exist to
/// report: a memory shown in full and a memory named in an index line must not
/// be counted as the same event.
///
/// Expectation source: the example store. A session start on `alpha/session-1`
/// makes three memories due (`bench-power` in full, `reading-list` and that
/// session's `notes` as index lines) and the widget tool call makes two more
/// (`widget-naming` in full, `rocket-stages` as an index line through the
/// implied scope).
#[test]
fn each_delivery_is_logged_with_the_form_and_the_reason_it_was_delivered_for() {
    let server = TestServer::start(example_store_files(), |_| {});

    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    let (_, answer) = server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_bash"));
    assert_eq!(
        permission_decision(&answer),
        Some("deny"),
        "the sequence the counts are derived from must be the one that ran, got {answer}"
    );

    let stats = server.stats();
    assert_eq!(
        stats.count_rows(Table::Deliveries),
        5,
        "the five memories delivered must be five rows"
    );
    assert_eq!(
        stats.count_deliveries("full", "new"),
        2,
        "the two critical memories must be logged as shown in full because they were new"
    );
    assert_eq!(
        stats.count_deliveries("index", "new"),
        3,
        "the three knowledge memories must be logged as index lines because they were new"
    );
}

/// Detects a trigger fire that is not logged, and an activation counted as new
/// when the scope was already on: the report of which trigger earns its keep
/// depends on telling those apart.
#[test]
fn a_trigger_that_turns_a_scope_on_is_logged_once_as_a_new_activation() {
    let server = TestServer::start(example_store_files(), |_| {});

    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));
    server.hook("alpha", SOME_TOKENS, &hook_fixture("pre_tool_use_bash"));

    let stats = server.stats();
    assert_eq!(
        stats.count_rows(Table::TriggerFires),
        1,
        "the one pattern that matched must be the one row logged"
    );
    assert_eq!(
        stats.count_trigger_fires(true),
        1,
        "the fire turned the widgets scope on, so it must be logged as a new activation"
    );
}

/// Detects statistics that are only in memory: a restart must not lose the log,
/// because it is the history of what was delivered.
#[test]
fn the_log_survives_a_restart() {
    let mut server = TestServer::start(example_store_files(), |_| {});
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    server.restart();
    server.hook(
        "alpha",
        SOME_TOKENS,
        &json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "session-2",
            "prompt": "start a second session"
        }),
    );

    assert_eq!(
        server.stats().count_rows(Table::HookEvents),
        2,
        "the event logged before the restart and the one after it must both be there"
    );
}
