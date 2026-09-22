//! Tests of the statistics database: the schema it is opened at, and what the
//! write path guarantees.
//!
//! Only what the write path guarantees is asserted: one row per hook event, one
//! row per delivery with the form and the reason it was delivered for, and one
//! row per trigger that fired. The aggregate queries come with the statistics
//! API.

mod common;

use std::path::Path;

use forgetmenot_server::stats::{SCHEMA_VERSION, StatsWriter, Table};
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

/// The tables of the log and the columns the reader's queries name in each.
///
/// Expectation source: the columns `stats` and `stats::queries` select and
/// bind. A database missing any of them cannot answer the reports.
const TABLE_COLUMNS: [(&str, &[&str]); 5] = [
    (
        "hook_events",
        &[
            "id",
            "ts",
            "machine",
            "session_id",
            "agent",
            "event",
            "context_tokens",
            "latency_us",
            "decision",
            "answer_chars",
        ],
    ),
    (
        "trigger_fires",
        &["event_id", "scope_id", "field", "pattern", "activated_new"],
    ),
    (
        "scope_forgettings",
        &["event_id", "scope_id", "tokens_since_trigger", "tokens_at"],
    ),
    (
        "deliveries",
        &[
            "event_id", "memory", "kind", "form", "reason", "bytes", "scope", "chars",
        ],
    ),
    (
        "tool_calls",
        &["id", "ts", "tool", "session_key", "memory", "scope", "ok"],
    ),
];

/// The schema version the database at `path` records.
fn user_version(path: &Path) -> i64 {
    rusqlite::Connection::open(path)
        .expect("the database opens")
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .expect("a sqlite database reports a user version")
}

/// The columns of one table, as sqlite reports them.
fn columns_of(path: &Path, table: &str) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).expect("the database opens");
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("the table can be asked what it holds");
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("the answer is a list of columns");
    columns.map(|name| name.expect("a column name")).collect()
}

/// Detects a database created without its migrations run, which would leave
/// every report to fail on a missing table, and one created at a version that
/// does not say what its shape is, which would make the next schema change run
/// the wrong step over it.
///
/// Expectation source: [`TABLE_COLUMNS`] for the shape, and the rule that a
/// database the server created is at the version of the schema it was created
/// from.
#[test]
fn a_database_created_from_nothing_ends_at_the_current_version_with_every_table_and_column() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let path = directory.path().join("stats.sqlite");

    StatsWriter::open(&path).expect("a database is created where there was none");

    assert_ne!(
        user_version(&path),
        0,
        "a database the server created records its version, and zero is what a database \
         under no version control reports"
    );
    assert_eq!(
        user_version(&path),
        SCHEMA_VERSION as i64,
        "a database created from nothing is at the version of the whole schema"
    );
    for (table, expected) in TABLE_COLUMNS {
        let columns = columns_of(&path, table);
        for column in expected {
            assert!(
                columns.iter().any(|name| name == column),
                "{table} must carry {column}, got {columns:?}"
            );
        }
    }
}

/// The batch the server ran to bootstrap the log before the version was
/// recorded, with two rows of the kind the deployed database holds.
///
/// Expectation source: the bootstrap that created the deployed database, kept
/// here verbatim, so that the test opens a database the running server really
/// wrote rather than one derived from today's schema. It records no version,
/// which is the case the adoption exists for.
const BOOTSTRAP_SQL: &str = "\
CREATE TABLE IF NOT EXISTS hook_events (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    machine TEXT NOT NULL,
    session_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    event TEXT NOT NULL,
    context_tokens INTEGER,
    latency_us INTEGER NOT NULL,
    decision TEXT NOT NULL,
    answer_chars INTEGER
);
CREATE TABLE IF NOT EXISTS trigger_fires (
    event_id INTEGER NOT NULL,
    scope_id TEXT NOT NULL,
    field TEXT NOT NULL,
    pattern TEXT NOT NULL,
    activated_new INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS scope_forgettings (
    event_id INTEGER NOT NULL,
    scope_id TEXT NOT NULL,
    tokens_since_trigger INTEGER NOT NULL,
    tokens_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS deliveries (
    event_id INTEGER NOT NULL,
    memory TEXT NOT NULL,
    kind TEXT NOT NULL,
    form TEXT NOT NULL,
    reason TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    scope TEXT,
    chars INTEGER
);
CREATE TABLE IF NOT EXISTS tool_calls (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    tool TEXT NOT NULL,
    session_key TEXT NOT NULL,
    memory TEXT,
    scope TEXT,
    ok INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS trigger_fires_event ON trigger_fires (event_id);
CREATE INDEX IF NOT EXISTS scope_forgettings_event ON scope_forgettings (event_id);
CREATE INDEX IF NOT EXISTS deliveries_event ON deliveries (event_id);
CREATE INDEX IF NOT EXISTS deliveries_memory ON deliveries (memory);
CREATE INDEX IF NOT EXISTS tool_calls_tool ON tool_calls (tool, memory);
INSERT INTO hook_events
    (ts, machine, session_id, agent, event, context_tokens, answer_chars, latency_us, decision)
VALUES ('2026-01-01T00:00:00Z', 'alpha', 'old-session', 'main', 'SessionStart', 9000, 400, 5,
        'context');
INSERT INTO deliveries (event_id, memory, kind, form, reason, bytes, scope, chars)
VALUES (1, 'bench-power', 'critical', 'full', 'new', 400, 'global', 400);
";

/// Detects a log that was bootstrapped before the version was recorded being
/// built over instead of adopted: the log is the history of what was delivered
/// and is never thrown away, so the running server's database has to open,
/// keep every row and take new ones.
///
/// Expectation source: [`BOOTSTRAP_SQL`] for what the database holds, and the
/// rule that a database already carrying the schema is stamped at the version
/// whose shape it has, which is the version a database created from nothing
/// ends at.
#[test]
fn a_log_bootstrapped_before_the_version_was_recorded_is_adopted_and_keeps_its_rows() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let path = directory.path().join("stats.sqlite");
    rusqlite::Connection::open(&path)
        .expect("the bootstrapped database is creatable")
        .execute_batch(BOOTSTRAP_SQL)
        .expect("the bootstrap is valid sqlite");
    assert_eq!(
        user_version(&path),
        0,
        "the bootstrap recorded no version, which is the database this test is about"
    );

    let server = TestServer::start(example_store_files(), {
        let path = path.clone();
        move |config| config.stats_path = path
    });
    server.hook("alpha", SOME_TOKENS, &hook_fixture("session_start"));

    assert_eq!(
        user_version(&path),
        SCHEMA_VERSION as i64,
        "the database carried the schema already, so it must be stamped at the version \
         whose shape it has"
    );
    let stats = server.stats();
    assert_eq!(
        stats.count_rows(Table::HookEvents),
        2,
        "the event of the bootstrapped log and the one just answered must both be there, so \
         the tables were adopted and not created again"
    );
    let deliveries = stats
        .deliveries_of("bench-power")
        .expect("the adopted log is readable");
    assert_eq!(
        deliveries
            .first()
            .map(|row| (row.scope.clone(), row.chars, row.bytes)),
        Some((Some("global".to_string()), 400, 400)),
        "the row the bootstrapped log held must be there unchanged, got {deliveries:?}"
    );
    let latest = deliveries
        .last()
        .expect("the session start delivered the memory");
    assert!(
        latest.chars > 0 && latest.scope.as_deref() == Some("global"),
        "the row written after the adoption must carry the section and the length of the text \
         sent, got {latest:?}"
    );
}

/// Detects a schema brought up a second time on a database already at the
/// current version, which would fail on the tables that are there or empty
/// them: the server opens the same file on every start, and a restart must
/// change nothing.
///
/// Expectation source: the rule that a migration runs once, so the version and
/// the rows are the same after the second open as after the first.
#[test]
fn opening_a_database_that_is_already_at_the_current_version_leaves_it_as_it_was() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let path = directory.path().join("stats.sqlite");
    StatsWriter::open(&path).expect("a database is created where there was none");
    let created = user_version(&path);
    rusqlite::Connection::open(&path)
        .expect("the created database opens")
        .execute_batch(
            "INSERT INTO hook_events
                 (ts, machine, session_id, agent, event, context_tokens, answer_chars,
                  latency_us, decision)
             VALUES ('2026-01-01T00:00:00Z', 'alpha', 'session-1', 'main', 'SessionStart',
                     9000, 400, 5, 'context');",
        )
        .expect("the created database takes a row");

    StatsWriter::open(&path).expect("a database at the current version opens again");

    assert_eq!(
        user_version(&path),
        created,
        "nothing was applied, so the version is the one the first open left"
    );
    assert_eq!(
        rusqlite::Connection::open(&path)
            .expect("the database opens")
            .query_row("SELECT count(*) FROM hook_events", [], |row| row
                .get::<_, i64>(0))
            .expect("the table is readable"),
        1,
        "the row written between the two opens must still be there"
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
