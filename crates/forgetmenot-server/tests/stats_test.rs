//! Tests of the statistics written on the hot path.
//!
//! Only what the write path guarantees is asserted: one row per hook event, one
//! row per delivery with the form and the reason it was delivered for, and one
//! row per trigger that fired. The aggregate queries come with the statistics
//! API.

mod common;

use forgetmenot_server::stats::{StatsWriter, Table};
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

/// The `CREATE TABLE` text of the two tables as the server wrote them before
/// the scope, character and answer-length columns existed.
///
/// Expectation source: the schema this crate shipped with, kept here verbatim
/// so that the test opens a database the old server really could have written
/// rather than one derived from today's schema.
const OLD_SCHEMA: &str = "\
CREATE TABLE hook_events (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    machine TEXT NOT NULL,
    session_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    event TEXT NOT NULL,
    context_tokens INTEGER,
    latency_us INTEGER NOT NULL,
    decision TEXT NOT NULL
);
CREATE TABLE deliveries (
    event_id INTEGER NOT NULL,
    memory TEXT NOT NULL,
    kind TEXT NOT NULL,
    form TEXT NOT NULL,
    reason TEXT NOT NULL,
    bytes INTEGER NOT NULL
);
INSERT INTO hook_events
    (ts, machine, session_id, agent, event, context_tokens, latency_us, decision)
VALUES ('2026-01-01T00:00:00Z', 'alpha', 'old-session', '', 'SessionStart', 9000, 5, 'context');
INSERT INTO deliveries (event_id, memory, kind, form, reason, bytes)
VALUES (1, 'bench-power', 'critical', 'full', 'new', 400);
";

/// Detects a server that refuses to open a log written before the scope, the
/// character count and the answer length were recorded, or that opens it and
/// then cannot write to it: the log is the history of what was delivered and is
/// never thrown away, so an upgrade has to keep reading and writing the same
/// file.
///
/// Expectation source: the rule that the columns are added when absent and that
/// a row written before they existed carries null rather than a made-up figure.
#[test]
fn a_log_written_before_the_new_columns_opens_keeps_its_rows_and_takes_new_ones() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let path = directory.path().join("stats.sqlite");
    rusqlite::Connection::open(&path)
        .expect("the old database is creatable")
        .execute_batch(OLD_SCHEMA)
        .expect("the old schema is valid sqlite");

    let server = TestServer::start(example_store_files(), {
        let path = path.clone();
        move |config| config.stats_path = path
    });
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    let stats = server.stats();
    assert_eq!(
        stats.count_rows(Table::HookEvents),
        2,
        "the event of the old log and the one just answered must both be there"
    );
    let old = stats
        .deliveries_of("bench-power")
        .expect("the old log is readable");
    assert_eq!(
        old.first().map(|row| (row.scope.clone(), row.chars)),
        Some((None, 0)),
        "the row written before the columns existed must carry no scope and no length, got {old:?}"
    );
    let latest = old.last().expect("the session start delivered the memory");
    assert_eq!(
        latest.scope.as_deref(),
        Some("global"),
        "the row written now must name the section it was printed in, got {latest:?}"
    );
    assert!(
        latest.chars > 0,
        "the row written now must carry the length of the text sent, got {latest:?}"
    );
    // The writer is opened again on the same file, which is what a second
    // upgrade of an already migrated log does.
    StatsWriter::open(&path).expect("the migrated database opens again");
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
