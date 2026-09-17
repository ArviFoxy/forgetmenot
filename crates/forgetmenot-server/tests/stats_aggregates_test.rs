//! Tests of the statistics read side: the aggregates `GET /api/stats/*` and
//! `forgetmenot stats` report.
//!
//! One scripted sequence of hook events runs against a real server on a copy of
//! the example store, and every expected count is worked out from that sequence
//! and the example store in the comment on [`run_sequence`]. Nothing here is a
//! captured number: a count that changes because the server delivered something
//! else is a failure, not an expectation to update.

mod common;

use std::process::Command;

use chrono::DateTime;
use forgetmenot_server::clock::{Clock, FixedClock};
use forgetmenot_server::stats::{MEMORY_GET_TOOL, StatsWriter, ToolCallRecord};
use serde_json::{Value, json};

use common::{
    TestServer, additional_context, binary_path, example_store_files, hook_fixture,
    permission_decision,
};

/// The machine every event of the sequence comes from.
const MACHINE: &str = "alpha";

/// The session every event of the sequence belongs to. It is the session the
/// example store has a session memory for.
const SESSION: &str = "session-1";

/// The context size reported with every event: the same one throughout, so that
/// nothing goes stale. Staleness is not what these counts are about.
const TOKENS: Option<u64> = Some(10_000);

/// `reading-list` as the example store holds it, with one change: it is a
/// critical memory, so it is delivered in full rather than as an index line.
const READING_LIST_AS_CRITICAL: &str = "\
---
name: reading-list
description: The workshop references are all on paper in the binder, nothing online
metadata:
  type: reference
  kind: critical
---
# Where the workshop references live

The bench notes, the parts catalogue and the test log all live in the workshop
binder; nothing in it is online.

Stage numbering is written up separately in [[rocket-stages]]. A link written as
`[[name]]` inside a code span, like this one, is not a link.
";

/// Run the scripted sequence and hand back the server it ran against.
///
/// The sequence, on machine `alpha` in session `session-1`, and what each step
/// implies against the example store:
///
/// 1. **SessionStart.** The context starts in `global`, `machine:alpha` and its
///    own session scope, and neither of its directories matches a trigger. Due, and
///    none of it delivered yet: `bench-power` in full (new), `reading-list` and
///    `sessions/alpha/session-1/notes` as index lines (new).
/// 2. **UserPromptSubmit naming a widget.** The `widgets` pattern on
///    `user_message` fires and turns the scope on for the first time, which
///    implies `rocketry`: `widget-naming` in full (new) and `rocket-stages` as an
///    index line (new).
/// 3. **A commit behind the server's back** makes `reading-list` critical, the
///    way a person with a shell would. No event, so nothing is delivered.
/// 4. **PreToolUse** whose input names widgets. The `widgets` pattern on
///    `tool_input` fires for a scope that is already on. `reading-list` is now
///    critical and its version differs from what was delivered, so the call is
///    stopped and it is delivered in full (changed).
/// 5. **The same PreToolUse again.** The pattern fires again; nothing is owed,
///    so the call is allowed and nothing is delivered.
/// 6. **SessionStart with source `compact`,** the start of the conversation a
///    compaction rebuilt. The session keeps its scopes and its record of what it
///    has been shown begins afresh, so everything due is new again:
///    `bench-power`, `reading-list` and `widget-naming` in full, `notes` and
///    `rocket-stages` as index lines. Neither of its directories matches a
///    trigger.
/// 7. **Stop.** Nothing is owed and nothing is delivered. The message matches no
///    trigger, because no pattern in the example store is on
///    `assistant_message`.
///
/// So six hook events, one of them stopped; eleven deliveries; three trigger
/// fires, one of them an activation.
fn run_sequence() -> TestServer {
    let server = TestServer::start(example_store_files(), |_| {});

    let (_, start) = server.hook(MACHINE, TOKENS, &hook_fixture("session_start"));
    assert!(
        additional_context(&start).is_some(),
        "step 1 must deliver the memories that are due at a session start, got {start}"
    );

    let (_, prompt) = server.hook(
        MACHINE,
        TOKENS,
        &json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": SESSION,
            "cwd": "/home/dev/widgets",
            "prompt": "check the widget numbering before we continue"
        }),
    );
    assert!(
        additional_context(&prompt).is_some(),
        "step 2 must deliver what turning the widgets scope on made due, got {prompt}"
    );

    server.commit(
        "reading-list becomes a critical memory",
        vec![(
            "memories/reading-list.md".to_string(),
            Some(READING_LIST_AS_CRITICAL.as_bytes().to_vec()),
        )],
    );

    let (_, stopped) = server.hook(MACHINE, TOKENS, &hook_fixture("pre_tool_use_bash"));
    assert_eq!(
        permission_decision(&stopped),
        Some("deny"),
        "step 4 must stop the call for the memory that changed, got {stopped}"
    );

    let (_, allowed) = server.hook(MACHINE, TOKENS, &hook_fixture("pre_tool_use_bash"));
    assert_eq!(
        permission_decision(&allowed),
        None,
        "step 5 must allow the re-issued call, got {allowed}"
    );

    let mut rebuilt = hook_fixture("session_start");
    rebuilt["source"] = json!("compact");
    let (_, after_compaction) = server.hook(MACHINE, TOKENS, &rebuilt);
    assert!(
        additional_context(&after_compaction).is_some(),
        "step 6 must deliver everything due again after the compaction, got {after_compaction}"
    );

    let (_, stop) = server.hook(MACHINE, TOKENS, &hook_fixture("stop"));
    assert!(
        additional_context(&stop).is_none(),
        "step 7 must deliver nothing, everything due arrived in step 6, got {stop}"
    );

    server
}

/// Detects index lines and full bodies counted as the same thing. `reading-list`
/// was named in one index line while it was a knowledge memory and delivered in
/// full once after it became critical, so it must report one of each and two of
/// nothing.
#[test]
fn a_memory_shown_as_an_index_line_and_later_in_full_reports_one_of_each() {
    let server = run_sequence();
    let rows = get(&server, "/api/stats/memories");
    let row = row_with(&rows, "memory", "reading-list");

    assert_eq!(
        row["shown_index"],
        json!(1),
        "one index line, in step 1: {row}"
    );
    assert_eq!(
        row["shown_full_changed"],
        json!(1),
        "one full delivery for the version that changed, in step 4: {row}"
    );
    assert_eq!(
        row["shown_full_new"],
        json!(1),
        "one full delivery as new after the compaction, in step 6: {row}"
    );
    assert_eq!(
        row["shown_full_stale"],
        json!(0),
        "the context never grew, so nothing went stale: {row}"
    );
    assert_eq!(
        row["retracted"],
        json!(0),
        "the memory stayed in scope throughout: {row}"
    );
    assert_eq!(
        row["fetched_full"],
        json!(0),
        "no tool fetched it, so no fetch may be counted: {row}"
    );
}

/// Detects a report that adds the two forms together, or crosses them over: a
/// critical memory is only ever delivered in full and a knowledge memory only
/// ever as an index line, so each must leave the other's columns at zero.
#[test]
fn full_deliveries_and_index_lines_are_never_added_into_one_count() {
    let server = run_sequence();
    let rows = get(&server, "/api/stats/memories");

    let critical = row_with(&rows, "memory", "bench-power");
    assert_eq!(
        critical["shown_full_new"],
        json!(2),
        "delivered in full in step 1 and again in step 6: {critical}"
    );
    assert_eq!(
        critical["shown_index"],
        json!(0),
        "a critical memory is never reduced to an index line: {critical}"
    );

    let knowledge = row_with(&rows, "memory", "rocket-stages");
    assert_eq!(
        knowledge["shown_index"],
        json!(2),
        "named in an index line in step 2 and again in step 6: {knowledge}"
    );
    for column in ["shown_full_new", "shown_full_changed", "shown_full_stale"] {
        assert_eq!(
            knowledge[column],
            json!(0),
            "a knowledge memory's body is never delivered: {column} of {knowledge}"
        );
    }
}

/// Detects a last-shown timestamp taken from the moment of the query rather than
/// from the log, which would make every memory look as if it had just been
/// delivered.
#[test]
fn the_last_shown_time_is_the_time_the_event_was_logged() {
    let server = run_sequence();
    let rows = get(&server, "/api/stats/memories");
    let row = row_with(&rows, "memory", "bench-power");
    let last_shown = row["last_shown"]
        .as_str()
        .unwrap_or_else(|| panic!("a delivered memory must report when: {row}"));

    assert_eq!(
        DateTime::parse_from_rfc3339(last_shown).map(|time| time.to_utc()),
        Ok(FixedClock::default().now()),
        "the server's clock stood still, so every event was logged at that instant"
    );
}

/// Detects a memory the model fetched itself counted as a delivery the server
/// made: what was pushed at the model and what the model asked for answer
/// different questions about a memory.
#[test]
fn a_memory_fetched_through_the_tool_is_counted_apart_from_what_was_shown() {
    let server = run_sequence();
    let writer = StatsWriter::open(server.stats_path()).expect("the statistics database opens");
    let runtime = tokio::runtime::Runtime::new().expect("a tokio runtime");
    runtime.block_on(async {
        writer
            .record_tool_call(ToolCallRecord {
                ts: FixedClock::default().now(),
                tool: MEMORY_GET_TOOL.to_string(),
                session_key: format!("{MACHINE}/{SESSION}"),
                memory: Some("reading-list".to_string()),
                scope: None,
                ok: true,
            })
            .await;
        writer.flush().await;
    });

    let row = row_with(
        &get(&server, "/api/stats/memories"),
        "memory",
        "reading-list",
    );
    assert_eq!(
        row["fetched_full"],
        json!(1),
        "the one fetch must be counted as a fetch: {row}"
    );
    assert_eq!(
        row["shown_index"],
        json!(1),
        "a fetch must not be counted as an index line: {row}"
    );
    assert_eq!(
        row["shown_full_changed"],
        json!(1),
        "a fetch must not be counted as a delivery the server made: {row}"
    );
}

/// Detects denies counted against the wrong day, or against only the events
/// that were stopped: the share of calls that are stopped is what says whether
/// the interrupt rule is worth its cost, so the day's whole event count has to
/// be there too.
#[test]
fn the_days_row_counts_the_one_stopped_call_among_every_event_of_that_day() {
    let server = run_sequence();
    let rows = get(&server, "/api/stats/denies");
    let day = FixedClock::default().now().date_naive().to_string();
    let row = row_with(&rows, "day", &day);

    assert_eq!(
        row["denies"],
        json!(1),
        "step 4 is the only stopped call of the sequence: {row}"
    );
    assert_eq!(
        row["events"],
        json!(6),
        "the sequence sent six hook events, all at the same fixed instant: {row}"
    );
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(1),
        "the clock stood still, so there is one day to report: {rows}"
    );
}

/// Detects a trigger report that cannot tell a pattern that turned a scope on
/// from one that matched again, or that spreads one event's deny over every
/// pattern: the two patterns of the `widgets` scope fired different numbers of
/// times, and only one fire led to a stopped call.
#[test]
fn each_trigger_pattern_reports_its_own_fires_activations_and_deny_share() {
    let server = run_sequence();
    let rows = get(&server, "/api/stats/triggers");

    let on_message = row_with_two(&rows, ("scope_id", "widgets"), ("field", "user_message"));
    assert_eq!(
        on_message["fires"],
        json!(1),
        "the prompt of step 2 is the only message naming a widget: {on_message}"
    );
    assert_eq!(
        on_message["new_activations"],
        json!(1),
        "that fire turned the scope on: {on_message}"
    );
    assert_eq!(
        on_message["deny_share"],
        json!(0.0),
        "a prompt cannot stop a tool call: {on_message}"
    );

    let on_input = row_with_two(&rows, ("scope_id", "widgets"), ("field", "tool_input"));
    assert_eq!(
        on_input["fires"],
        json!(2),
        "the tool input of steps 4 and 5 both name widgets: {on_input}"
    );
    assert_eq!(
        on_input["new_activations"],
        json!(0),
        "the scope was already on by then: {on_input}"
    );
    assert_eq!(
        on_input["deny_share"],
        json!(0.5),
        "one of those two fires was on the event that stopped the call: {on_input}"
    );
}

/// Detects a scope report that mixes up what the log says with what the server
/// holds: `widgets` was turned on once by a trigger, and one live context is
/// working in it.
#[test]
fn a_scope_reports_its_activations_and_the_contexts_live_in_it() {
    let server = run_sequence();
    let row = row_with(&get(&server, "/api/stats/scopes"), "scope_id", "widgets");

    assert_eq!(
        row["activations"],
        json!(1),
        "one trigger fire turned the scope on, in step 2: {row}"
    );
    assert_eq!(
        row["live_contexts"],
        json!(1),
        "the sequence ran in one context, and it is working in this scope: {row}"
    );
}

/// Detects a closure counted as an activation: `rocketry` was never matched by a
/// pattern, it is on because `widgets` implies it, so it is live without having
/// been activated.
#[test]
fn a_scope_turned_on_by_implication_is_live_without_an_activation() {
    let server = run_sequence();
    let row = row_with(&get(&server, "/api/stats/scopes"), "scope_id", "rocketry");

    assert_eq!(
        row["activations"],
        json!(0),
        "no message of the sequence names a rocket: {row}"
    );
    assert_eq!(
        row["live_contexts"],
        json!(1),
        "the implied scope is in force in the one live context: {row}"
    );
}

/// Detects delivered bytes attributed to a session key the MCP tools cannot
/// take, and the two forms' bytes summed into one column, which would leave the
/// other at zero.
#[test]
fn delivered_bytes_are_reported_per_session_with_the_two_forms_apart() {
    let server = run_sequence();
    let rows = get(&server, "/api/stats/sessions");

    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(1),
        "every event of the sequence belongs to one session: {rows}"
    );
    let row = row_with(&rows, "session_key", &format!("{MACHINE}/{SESSION}"));
    assert!(
        row["bytes_full"].as_u64().unwrap_or(0) > 0,
        "six full bodies were delivered: {row}"
    );
    assert!(
        row["bytes_index"].as_u64().unwrap_or(0) > 0,
        "five index lines were delivered: {row}"
    );
    assert_eq!(
        row["last_context_tokens"],
        json!(10_000),
        "every event of the sequence reported the same context size: {row}"
    );
    let chars = row["chars"]
        .as_u64()
        .unwrap_or_else(|| panic!("a session reports the characters delivered into it: {row}"));
    assert_eq!(
        row["tokens"].as_u64(),
        Some((chars as f64 / 3.5).ceil() as u64),
        "the tokens must be the characters over the store's divisor of 3.5, rounded up: {row}"
    );
}

/// Detects latency reported for the wrong number of measurements, or
/// percentiles that are not ordered, which would make a latency budget
/// unreadable. The two PreToolUse events of the sequence are one row of two.
#[test]
fn latency_covers_every_event_of_one_name_with_ordered_percentiles() {
    let server = run_sequence();
    let rows = get(&server, "/api/stats/latency");
    let row = row_with(&rows, "event", "PreToolUse");

    assert_eq!(
        row["count"],
        json!(2),
        "steps 4 and 5 are the two PreToolUse events: {row}"
    );
    let percentile = |name: &str| {
        row[name]
            .as_u64()
            .unwrap_or_else(|| panic!("{name} must be a number of microseconds: {row}"))
    };
    assert!(
        percentile("p50_us") <= percentile("p90_us")
            && percentile("p90_us") <= percentile("p99_us")
            && percentile("p99_us") <= percentile("max_us"),
        "the percentiles of one row must not decrease: {row}"
    );

    let counted: u64 = rows
        .as_array()
        .expect("the report is a list of rows")
        .iter()
        .map(|row| row["count"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(
        counted, 6,
        "each of the six events must be measured in exactly one row: {rows}"
    );
}

/// Detects a command that reports different numbers from the API for the same
/// log, which would make the two disagree about what happened.
///
/// The command prints the raw character counts the log holds and the API turns
/// them into tokens, so the API's rows carry a field the command's do not; every
/// field the command does report has to be the API's value for it.
#[test]
fn the_command_reports_the_numbers_the_api_reports_for_the_same_log() {
    let server = run_sequence();
    let from_api: Vec<(&str, Value)> = vec![
        ("memories", get(&server, "/api/stats/memories")),
        ("triggers", get(&server, "/api/stats/triggers")),
        ("denies", get(&server, "/api/stats/denies")),
        ("latency", get(&server, "/api/stats/latency")),
        ("sessions", get(&server, "/api/stats/sessions")),
    ];
    let from_command = run_command(&server, &["--json"]);

    for (aggregate, rows) in from_api {
        let printed = from_command[aggregate]
            .as_array()
            .unwrap_or_else(|| panic!("the command must report the {aggregate}: {from_command}"));
        let answered = rows
            .as_array()
            .unwrap_or_else(|| panic!("the API must report the {aggregate}: {rows}"));
        assert_eq!(
            printed.len(),
            answered.len(),
            "the command and the API must report the same {aggregate} rows"
        );
        for (printed, answered) in printed.iter().zip(answered) {
            for (field, value) in printed
                .as_object()
                .unwrap_or_else(|| panic!("a row is an object, got {printed}"))
            {
                assert_eq!(
                    &answered[field], value,
                    "the command and the API must agree on the {field} of this {aggregate} row: \
                     {printed} against {answered}"
                );
            }
        }
    }
}

/// Detects a command that prints a live-context count it cannot know: it reads a
/// file, and which contexts are working in a scope is state a running server
/// holds. The activations it does know must still match the API.
#[test]
fn the_command_reports_activations_without_claiming_to_know_live_contexts() {
    let server = run_sequence();
    let from_api = get(&server, "/api/stats/scopes");
    let from_command = run_command(&server, &["--json"]);
    let scopes = from_command["scopes"]
        .as_array()
        .unwrap_or_else(|| panic!("the report must list the scopes: {from_command}"));

    for row in scopes {
        assert!(
            row.get("live_contexts").is_none(),
            "the command must not report live contexts: {row}"
        );
        let scope_id = row["scope_id"].as_str().expect("a scope id is text");
        assert_eq!(
            row["activations"],
            row_with(&from_api, "scope_id", scope_id)["activations"],
            "the command and the API must agree on the activations of {scope_id}"
        );
    }
}

/// Detects a table report that drops rows, prints only headers, or fails on a
/// log that has something in it: every memory the sequence delivered has to be
/// in it.
#[test]
fn the_table_report_names_every_memory_that_was_delivered() {
    let server = run_sequence();
    let output = Command::new(binary_path())
        .args(["stats", "--stats-path"])
        .arg(server.stats_path())
        .output()
        .expect("the command runs");
    assert!(
        output.status.success(),
        "the command must succeed on a readable log, got {:?} and {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("the report is text");
    for memory in [
        "bench-power",
        "reading-list",
        "rocket-stages",
        "widget-naming",
        "sessions/alpha/session-1/notes",
    ] {
        assert!(
            text.contains(memory),
            "the report must name {memory}, got:\n{text}"
        );
    }
}

/// Read one JSON resource from the server, failing the test on any other answer.
fn get(server: &TestServer, path: &str) -> Value {
    let (status, answer) = server.api("GET", path, None);
    assert_eq!(
        status, 200,
        "GET {path} must be answered, got {status}: {answer}"
    );
    answer
}

/// Run `forgetmenot stats` on the server's log and read its JSON.
fn run_command(server: &TestServer, arguments: &[&str]) -> Value {
    // The reader flushes what the hot path queued, so the file holds every
    // record of the sequence before another process reads it.
    let _ = server.stats();
    let output = Command::new(binary_path())
        .args(["stats", "--stats-path"])
        .arg(server.stats_path())
        .args(arguments)
        .output()
        .expect("the command runs");
    assert!(
        output.status.success(),
        "the command must succeed on a readable log, got {:?} and {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "the report must be JSON: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// The one row whose `field` is `value`.
fn row_with(rows: &Value, field: &str, value: &str) -> Value {
    let matching: Vec<&Value> = rows
        .as_array()
        .unwrap_or_else(|| panic!("a report is a list of rows, got {rows}"))
        .iter()
        .filter(|row| row[field].as_str() == Some(value))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one row must have {field} {value}, got {rows}"
    );
    matching[0].clone()
}

/// The one row matching both fields.
fn row_with_two(rows: &Value, first: (&str, &str), second: (&str, &str)) -> Value {
    let matching: Vec<&Value> = rows
        .as_array()
        .unwrap_or_else(|| panic!("a report is a list of rows, got {rows}"))
        .iter()
        .filter(|row| {
            row[first.0].as_str() == Some(first.1) && row[second.0].as_str() == Some(second.1)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one row must have {} {} and {} {}, got {rows}",
        first.0,
        first.1,
        second.0,
        second.1
    );
    matching[0].clone()
}
