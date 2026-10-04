//! Coming back up on a statistics database an older build wrote.
//!
//! The server opens the database the build before it left behind, so every
//! version of the schema that database has ever been at has to open, keep its
//! rows and reach the version this build writes. The fixture is one such
//! database, and [`regenerate_the_version_1_stats_fixture`] is what produced it.

mod common;

use std::path::{Path, PathBuf};

use forgetmenot_server::stats::{SCHEMA_VERSION, Table};
use serde_json::{Value, json};

use common::{TestServer, example_store_files};

/// The tables of version 1, every one of which the fixture holds rows of.
const VERSION_1_TABLES: [&str; 5] = [
    "hook_events",
    "trigger_fires",
    "scope_forgettings",
    "deliveries",
    "tool_calls",
];

/// The tables version 2 adds.
const VERSION_2_TABLES: [&str; 2] = ["scope_activations", "activation_messages"];

/// The machine the fixture was played on.
const ALPHA: &str = "alpha";

/// The session the fixture was played on.
const SESSION: &str = "session-1";

/// The file these tests read, as committed.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stats/stats-version-1.sqlite")
}

/// One event of the played session, with the fields every event of it carries.
fn event(fields: Value) -> Value {
    let mut payload = json!({
        "session_id": SESSION,
        "transcript_path": "/home/dev/widgets/.claude/session-1.jsonl",
        "cwd": "/home/dev/widgets",
        "permission_mode": "default",
    });
    let object = payload.as_object_mut().expect("a payload is an object");
    for (key, value) in fields.as_object().expect("the fields are an object") {
        object.insert(key.clone(), value.clone());
    }
    payload
}

/// The schema version the database at `path` records.
fn user_version(path: &Path) -> i64 {
    rusqlite::Connection::open(path)
        .expect("the database opens")
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .expect("a sqlite database reports a user version")
}

/// Whether the database at `path` has a table named `table`.
fn has_table(path: &Path, table: &str) -> bool {
    rusqlite::Connection::open(path)
        .expect("the database opens")
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get::<_, i64>(0),
        )
        .expect("the schema is readable")
        == 1
}

/// Every row of `table` in the database at `path`, in the order they were
/// written, each as the debug text of its values.
fn rows_of(path: &Path, table: &str) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).expect("the database opens");
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .expect("the table is readable");
    let width = statement.column_count();
    let rows = statement
        .query_map([], |row| {
            (0..width)
                .map(|column| row.get::<_, rusqlite::types::Value>(column))
                .collect::<Result<Vec<_>, _>>()
                .map(|values| format!("{values:?}"))
        })
        .expect("the rows are readable");
    rows.map(|row| row.expect("a row")).collect()
}

/// Detects a fixture that is no longer a version-1 database: one at a later
/// version, or one that already holds the activation log, would let the test
/// below pass with the migration doing nothing.
#[test]
fn the_fixture_is_at_version_1_without_the_activation_log() {
    let path = fixture_path();

    assert_eq!(user_version(&path), 1, "the fixture records version 1");
    for table in VERSION_2_TABLES {
        assert!(
            !has_table(&path, table),
            "the fixture must not hold {table}, which version 2 adds"
        );
    }
}

/// Detects a statistics database of the build before the activation log that
/// does not reach the current version, that is built over instead of migrated,
/// or that loses or changes a row on the way: the statistics are the history of
/// what was delivered and are never thrown away. Detects too a migration that
/// leaves the new tables unable to take an entry.
///
/// Expectation source: the fixture's own rows, read before the server opens a
/// copy of it, and the example store's `widgets` trigger, which the prompt sent
/// afterwards matches.
#[test]
fn a_version_1_database_gains_the_activation_log_and_keeps_every_row() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("stats.sqlite");
    std::fs::copy(fixture_path(), &path).expect("the fixture is copied where a server reads");
    let before: Vec<Vec<String>> = VERSION_1_TABLES
        .iter()
        .map(|table| rows_of(&path, table))
        .collect();

    let server = TestServer::start(example_store_files(), {
        let path = path.clone();
        move |config| config.stats_path = path
    });

    assert_eq!(
        user_version(&path),
        SCHEMA_VERSION as i64,
        "the database is brought to the current version"
    );
    let after: Vec<Vec<String>> = VERSION_1_TABLES
        .iter()
        .map(|table| rows_of(&path, table))
        .collect();
    assert_eq!(after, before, "every row of version 1 is there unchanged");
    let (status, answer) = server.hook(
        ALPHA,
        Some(10_000),
        &event(json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "session-2",
            "prompt": "rename the widget brackets",
        })),
    );
    assert_eq!(status, 200, "the prompt must be answered, got {answer}");
    assert_eq!(
        server.stats().count_rows(Table::ActivationMessages),
        1,
        "the migrated database takes the message of the prompt that turned widgets on"
    );
}

/// What the fixture was made from. Ignored, because it writes the fixture
/// rather than checking anything; run it to make the fixture again:
///
/// ```text
/// cargo test -p forgetmenot-server --test stats_migration_test -- \
///     --ignored regenerate_the_version_1_stats_fixture
/// ```
///
/// A server on the example store is played a session start, a prompt that names
/// a widget, a subagent start, a call inside the subagent and one MCP tool call,
/// and is then shut down, which writes everything it queued. The database is
/// copied out with `VACUUM INTO`, which is one file holding every committed row
/// whatever the write-ahead log still held. Dropping the tables version 2 added
/// and recording version 1 is what turns a database this build writes into one
/// the build before it wrote: the first fixture was written by that build
/// itself, and the shape of version 1 is the same either way.
#[test]
#[ignore = "writes the committed fixture rather than checking anything"]
fn regenerate_the_version_1_stats_fixture() {
    let mut server = TestServer::start(example_store_files(), |_| {});

    let (status, _) = server.hook(
        ALPHA,
        Some(10_000),
        &event(json!({ "hook_event_name": "SessionStart", "source": "startup" })),
    );
    assert_eq!(status, 200, "the session must start");
    let (status, _) = server.hook(
        ALPHA,
        Some(10_400),
        &event(json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": "rename the widget brackets",
        })),
    );
    assert_eq!(status, 200, "the prompt must be answered");
    let (status, _) = server.hook_tasked(
        ALPHA,
        Some(12_000),
        Some("list the widget files"),
        &event(json!({
            "hook_event_name": "SubagentStart",
            "agent_id": "agent-1",
            "agent_type": "general-purpose",
        })),
    );
    assert_eq!(status, 200, "the subagent must start");
    let (status, _) = server.hook(
        ALPHA,
        Some(10_200),
        &event(json!({
            "hook_event_name": "PreToolUse",
            "agent_id": "agent-1",
            "tool_name": "Grep",
            "tool_use_id": "toolu_01AAAAAAAAAAAAAAAAAAAAAA",
            "tool_input": { "pattern": "widget" },
        })),
    );
    assert_eq!(status, 200, "the subagent's call must be answered");
    {
        let mcp = server.mcp();
        let answer = mcp.call(
            "session_scope_on",
            json!({ "session_key": format!("{ALPHA}/{SESSION}"), "scopes": ["workshop"] }),
        );
        assert_ne!(answer.is_error, Some(true), "the tool call must succeed");
    }

    // A restart shuts the first server down, which flushes what it queued.
    server.restart();
    let path = fixture_path();
    std::fs::create_dir_all(path.parent().expect("the fixture is in a directory"))
        .expect("the fixture directory is creatable");
    if path.exists() {
        std::fs::remove_file(&path).expect("the old fixture is removable");
    }
    rusqlite::Connection::open(server.stats_path())
        .expect("the database the server wrote opens")
        .execute("VACUUM INTO ?1", [path.to_string_lossy()])
        .expect("the database is copied into the fixture");
    let fixture = rusqlite::Connection::open(&path).expect("the fixture opens");
    for table in VERSION_2_TABLES {
        fixture
            .execute_batch(&format!("DROP TABLE {table};"))
            .unwrap_or_else(|error| panic!("dropping {table} failed: {error}"));
    }
    fixture
        .execute_batch("PRAGMA user_version = 1; VACUUM;")
        .expect("the fixture records version 1");
}
