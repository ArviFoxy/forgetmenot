//! The statistics log.
//!
//! Append-only sqlite, written off the hot path: the hook handler hands a
//! record to a bounded channel and one writer thread inserts it. The channel
//! applies backpressure when it is full rather than dropping records, because a
//! missing row would be read later as "this memory was never delivered".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use crate::context::ContextKey;
use crate::store::ScopeId;

/// How many records may be queued before senders wait.
pub const QUEUE_CAPACITY: usize = 4096;

/// What went wrong opening or reading the statistics database.
#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error("statistics database {path}: {source}")]
    Sqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
}

/// The decision an event was answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// A tool call was stopped so a critical memory could be read first.
    Deny,
    /// Context was attached to the answer.
    Context,
    /// Nothing was owed.
    None,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Deny => "deny",
            Decision::Context => "context",
            Decision::None => "none",
        }
    }
}

/// One trigger that matched during an event.
#[derive(Clone, Debug)]
pub struct TriggerFire {
    pub scope_id: String,
    pub field: String,
    pub pattern: String,
    /// Whether this fire turned a scope on that was off before the event.
    pub activated_new: bool,
}

/// One scope turned off during an event because its `forget` rule was reached.
#[derive(Clone, Debug)]
pub struct ForgottenScope {
    pub scope_id: String,
    /// The rule that was reached: the context tokens since the last activation
    /// the scope's file declares.
    pub tokens_since_trigger: u64,
    /// The context size the event reported, at which the count was reached.
    pub tokens_at: u64,
}

/// One memory delivered during an event.
#[derive(Clone, Debug)]
pub struct Delivery {
    pub memory: String,
    pub kind: String,
    /// `full` or `index`; for a retraction, the form it had been delivered in is
    /// not what matters, so `index` and `full` are both written as `none`.
    pub form: String,
    /// `new`, `changed`, `stale`, `shrunk` or `retracted`.
    pub reason: String,
    pub bytes: u64,
}

/// One hook event and everything it produced.
#[derive(Clone, Debug)]
pub struct HookEventRecord {
    pub ts: DateTime<Utc>,
    pub machine: String,
    pub session_id: String,
    pub agent: String,
    pub event: String,
    pub context_tokens: Option<u64>,
    pub latency_us: u64,
    pub decision: Decision,
    pub triggers: Vec<TriggerFire>,
    pub forgotten: Vec<ForgottenScope>,
    pub deliveries: Vec<Delivery>,
}

/// One MCP tool call.
#[derive(Clone, Debug)]
pub struct ToolCallRecord {
    pub ts: DateTime<Utc>,
    pub tool: String,
    pub session_key: String,
    pub memory: Option<String>,
    pub scope: Option<String>,
    pub ok: bool,
}

/// What the writer thread receives.
enum Record {
    Hook(Box<HookEventRecord>),
    ToolCall(Box<ToolCallRecord>),
    /// Answered once everything queued before it has been inserted.
    Flush(oneshot::Sender<()>),
}

/// The handle every writer of statistics holds.
#[derive(Clone)]
pub struct StatsWriter {
    sender: mpsc::Sender<Record>,
}

impl StatsWriter {
    /// Open the database at `path`, create its schema if needed and start the
    /// writer thread.
    ///
    /// The thread owns the connection, so no sqlite handle is ever held across
    /// an await; it ends when the last `StatsWriter` is dropped.
    pub fn open(path: &Path) -> Result<Self, StatsError> {
        let connection = open_connection(path)?;
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("forgetmenot-stats".to_string())
            .spawn(move || write_records(connection, receiver))
            .map_err(|error| StatsError::Sqlite {
                path: path.to_path_buf(),
                source: rusqlite::Error::ToSqlConversionFailure(Box::new(error)),
            })?;
        Ok(Self { sender })
    }

    /// Queue one hook event. Waits when the queue is full; records are never
    /// dropped.
    pub async fn record_hook_event(&self, record: HookEventRecord) {
        let _ = self.sender.send(Record::Hook(Box::new(record))).await;
    }

    /// Queue one tool call.
    pub async fn record_tool_call(&self, record: ToolCallRecord) {
        let _ = self.sender.send(Record::ToolCall(Box::new(record))).await;
    }

    /// Wait until everything queued so far has been written.
    pub async fn flush(&self) {
        let (sender, receiver) = oneshot::channel();
        if self.sender.send(Record::Flush(sender)).await.is_ok() {
            let _ = receiver.await;
        }
    }
}

/// Run `query` against the log at `path`, with everything `writer` has queued
/// written first.
///
/// The flush is what makes an answer cover the requests answered before it: the
/// statistics are written off the hot path, so without it a reader can miss the
/// event it was opened to look at. sqlite is a blocking API, so the query itself
/// does not run on an async worker.
pub async fn read<Answer: Send + 'static>(
    writer: &StatsWriter,
    path: &Path,
    query: impl FnOnce(&StatsReader) -> Result<Answer, StatsError> + Send + 'static,
) -> Result<Answer, StatsError> {
    writer.flush().await;
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking({
        let path = path.clone();
        move || query(&StatsReader::open(&path)?)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(StatsError::Sqlite {
            path,
            source: rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error))),
        }),
    }
}

/// Reads the statistics database: the aggregates the JSON API and
/// `forgetmenot stats` report, and the counts the tests of the write path need.
pub struct StatsReader {
    connection: Connection,
    /// Named in every error, so a failure says which database it came from.
    path: PathBuf,
}

impl StatsReader {
    pub fn open(path: &Path) -> Result<Self, StatsError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| StatsError::Sqlite {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
        })
    }

    /// The number of rows in one of the statistics tables.
    pub fn count_rows(&self, table: Table) -> u64 {
        self.count(
            &format!("SELECT count(*) FROM {}", table.as_str()),
            params![],
        )
    }

    /// The number of deliveries recorded with this form and reason.
    pub fn count_deliveries(&self, form: &str, reason: &str) -> u64 {
        self.count(
            "SELECT count(*) FROM deliveries WHERE form = ?1 AND reason = ?2",
            params![form, reason],
        )
    }

    /// The number of trigger fires recorded as turning a scope on, or as firing
    /// for a scope that was already on.
    pub fn count_trigger_fires(&self, activated_new: bool) -> u64 {
        self.count(
            "SELECT count(*) FROM trigger_fires WHERE activated_new = ?1",
            params![i64::from(activated_new)],
        )
    }

    /// The decisions recorded, oldest first.
    pub fn decisions(&self) -> Vec<String> {
        let mut statement = self
            .connection
            .prepare("SELECT decision FROM hook_events ORDER BY id")
            .expect("the query compiles");
        let rows = statement
            .query_map(params![], |row| row.get::<_, String>(0))
            .expect("the query runs");
        rows.map(|row| row.expect("a decision is text")).collect()
    }

    fn count(&self, query: &str, parameters: &[&dyn rusqlite::ToSql]) -> u64 {
        self.connection
            .query_row(query, parameters, |row| row.get::<_, i64>(0))
            .map(|count| count as u64)
            .unwrap_or_else(|error| panic!("{query}: {error}"))
    }
}

// ---------------------------------------------------------------------------
// Aggregates: what the statistics API and `forgetmenot stats` report.
// ---------------------------------------------------------------------------

/// Prefix of the `reason` of a delivery row that withdrew a memory instead of
/// showing it; what follows is why it was withdrawn.
pub const RETRACTED_PREFIX: &str = "retracted:";

/// The `reason` of a delivery row for a memory whose new version only removed
/// lines from what the context had already been given, so nothing was sent.
pub const SHRUNK_REASON: &str = "shrunk";

/// The tool whose calls count as a memory fetched in full by the model.
pub const MEMORY_GET_TOOL: &str = "memory_get";

/// What was delivered for one memory.
///
/// An index line and a full body are counted in separate columns and never
/// added together: they cost different amounts of context and answer different
/// questions, so one number covering both would say nothing about either.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryStatsRow {
    pub memory: String,
    /// Times this memory was named in an index line.
    pub shown_index: u64,
    /// Times the whole body was delivered because the memory was new here.
    pub shown_full_new: u64,
    /// Times the whole body was delivered because the store held a new version.
    pub shown_full_changed: u64,
    /// Times the whole body was delivered again because the context had grown
    /// past the stale threshold.
    pub shown_full_stale: u64,
    /// Times the model fetched the whole body itself, through the tool.
    pub fetched_full: u64,
    /// Times a new version was not delivered because it only removed lines from
    /// the text the context already held. Not a delivery: nothing was sent.
    pub shrunk: u64,
    /// Times this memory was withdrawn from a context.
    pub retracted: u64,
    /// When this memory was last delivered in any form, as the log wrote it.
    pub last_shown: Option<String>,
}

impl MemoryStatsRow {
    fn empty(memory: String) -> Self {
        Self {
            memory,
            shown_index: 0,
            shown_full_new: 0,
            shown_full_changed: 0,
            shown_full_stale: 0,
            fetched_full: 0,
            shrunk: 0,
            retracted: 0,
            last_shown: None,
        }
    }
}

/// What one trigger pattern did.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TriggerStatsRow {
    pub scope_id: String,
    pub field: String,
    pub pattern: String,
    /// Times this pattern matched.
    pub fires: u64,
    /// Times matching turned the scope on rather than finding it already on.
    pub new_activations: u64,
    /// The fraction of the fires whose event stopped a tool call.
    pub deny_share: f64,
}

/// What one scope did, and where it is in force now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScopeStatsRow {
    pub scope_id: String,
    /// Times a trigger turned this scope on in a context it was off in.
    pub activations: u64,
    /// Times this scope turned itself off because its `forget` rule was reached.
    pub forgettings: u64,
    /// Live contexts working in this scope; this is the registry's answer, not
    /// the log's.
    pub live_contexts: u64,
}

/// Stopped tool calls on one day, against every event of that day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DenyDayRow {
    /// The day in UTC, as `YYYY-MM-DD`.
    pub day: String,
    pub denies: u64,
    pub events: u64,
}

/// How long the server took to answer the events of one name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LatencyRow {
    pub event: String,
    pub count: u64,
    pub p50_us: u64,
    pub p90_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

/// What one session was given, in bytes, with the two forms kept apart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionBytesRow {
    /// `<machine>/<session-id>[/<agent-id>]`, the key the MCP tools take.
    pub session_key: String,
    pub bytes_full: u64,
    pub bytes_index: u64,
}

impl StatsReader {
    /// What was delivered for each memory, by form and reason, one row per
    /// memory that was ever delivered or fetched, sorted by memory.
    pub fn memory_stats(&self) -> Result<Vec<MemoryStatsRow>, StatsError> {
        let mut rows: BTreeMap<String, MemoryStatsRow> = BTreeMap::new();
        let deliveries = self.rows(
            "SELECT deliveries.memory, deliveries.form, deliveries.reason, hook_events.ts
             FROM deliveries JOIN hook_events ON hook_events.id = deliveries.event_id",
            params![],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        for (memory, form, reason, ts) in deliveries {
            let row = rows
                .entry(memory.clone())
                .or_insert_with(|| MemoryStatsRow::empty(memory));
            // A shrunk row is counted before the form is read, because nothing
            // was shown in that form: counting it as one would inflate the
            // deliveries this memory is reported to have cost.
            if reason == SHRUNK_REASON {
                row.shrunk += 1;
                continue;
            }
            if reason.starts_with(RETRACTED_PREFIX) {
                row.retracted += 1;
            } else {
                match (form.as_str(), reason.as_str()) {
                    ("index", _) => row.shown_index += 1,
                    ("full", "new") => row.shown_full_new += 1,
                    ("full", "changed") => row.shown_full_changed += 1,
                    ("full", "stale") => row.shown_full_stale += 1,
                    _ => {}
                }
            }
            keep_newest(&mut row.last_shown, ts);
        }

        let fetches = self.rows(
            "SELECT memory, count(*) FROM tool_calls
             WHERE tool = ?1 AND memory IS NOT NULL
             GROUP BY memory",
            params![MEMORY_GET_TOOL],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;
        // A memory the model only ever fetched itself was never delivered, so it
        // has no delivery row; it still belongs in the report.
        for (memory, count) in fetches {
            rows.entry(memory.clone())
                .or_insert_with(|| MemoryStatsRow::empty(memory))
                .fetched_full = count.max(0) as u64;
        }
        Ok(rows.into_values().collect())
    }

    /// What each trigger pattern did, sorted by scope, field and pattern.
    pub fn trigger_stats(&self) -> Result<Vec<TriggerStatsRow>, StatsError> {
        self.rows(
            "SELECT trigger_fires.scope_id, trigger_fires.field, trigger_fires.pattern,
                    count(*),
                    sum(trigger_fires.activated_new),
                    sum(CASE WHEN hook_events.decision = 'deny' THEN 1 ELSE 0 END)
             FROM trigger_fires JOIN hook_events ON hook_events.id = trigger_fires.event_id
             GROUP BY trigger_fires.scope_id, trigger_fires.field, trigger_fires.pattern
             ORDER BY trigger_fires.scope_id, trigger_fires.field, trigger_fires.pattern",
            params![],
            |row| {
                let fires = row.get::<_, i64>(3)?.max(0) as u64;
                let denies = row.get::<_, i64>(5)?.max(0) as u64;
                Ok(TriggerStatsRow {
                    scope_id: row.get(0)?,
                    field: row.get(1)?,
                    pattern: row.get(2)?,
                    fires,
                    new_activations: row.get::<_, i64>(4)?.max(0) as u64,
                    // A pattern that never fired has no share of anything.
                    deny_share: if fires == 0 {
                        0.0
                    } else {
                        denies as f64 / fires as f64
                    },
                })
            },
        )
    }

    /// What each scope did, and how many live contexts work in it.
    ///
    /// `active_sets` is the active scope set of every live context, which only a
    /// running server knows: it is the registry's state, not the log's.
    pub fn scope_stats(
        &self,
        active_sets: &[BTreeSet<ScopeId>],
    ) -> Result<Vec<ScopeStatsRow>, StatsError> {
        /// The row of one scope, added with nothing counted yet if absent.
        fn row_for<'a>(
            rows: &'a mut BTreeMap<String, ScopeStatsRow>,
            scope_id: &str,
        ) -> &'a mut ScopeStatsRow {
            rows.entry(scope_id.to_string())
                .or_insert_with(|| ScopeStatsRow {
                    scope_id: scope_id.to_string(),
                    activations: 0,
                    forgettings: 0,
                    live_contexts: 0,
                })
        }

        let mut rows: BTreeMap<String, ScopeStatsRow> = BTreeMap::new();
        let activations = self.rows(
            "SELECT scope_id, count(*) FROM trigger_fires
             WHERE activated_new = 1
             GROUP BY scope_id",
            params![],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;
        for (scope_id, count) in activations {
            row_for(&mut rows, &scope_id).activations = count.max(0) as u64;
        }
        let forgettings = self.rows(
            "SELECT scope_id, count(*) FROM scope_forgettings GROUP BY scope_id",
            params![],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;
        for (scope_id, count) in forgettings {
            row_for(&mut rows, &scope_id).forgettings = count.max(0) as u64;
        }
        // A scope can be live without ever having been activated by a trigger:
        // the implicit scopes are, and so is one a scope implies.
        for active in active_sets {
            for scope in active {
                row_for(&mut rows, scope.as_str()).live_contexts += 1;
            }
        }
        Ok(rows.into_values().collect())
    }

    /// Stopped tool calls per day, against the events of that day, oldest first.
    pub fn deny_days(&self) -> Result<Vec<DenyDayRow>, StatsError> {
        // Timestamps are written as RFC 3339 in UTC, so the first ten characters
        // are the UTC date.
        self.rows(
            "SELECT substr(ts, 1, 10) AS day,
                    sum(CASE WHEN decision = 'deny' THEN 1 ELSE 0 END),
                    count(*)
             FROM hook_events
             GROUP BY day
             ORDER BY day",
            params![],
            |row| {
                Ok(DenyDayRow {
                    day: row.get(0)?,
                    denies: row.get::<_, i64>(1)?.max(0) as u64,
                    events: row.get::<_, i64>(2)?.max(0) as u64,
                })
            },
        )
    }

    /// How long events of each name took, sorted by event name.
    ///
    /// The percentiles are nearest-rank over the recorded latencies, computed
    /// here rather than in sqlite: the log is small enough to sort, and
    /// nearest-rank is then the same arithmetic wherever it is reported.
    pub fn latency(&self) -> Result<Vec<LatencyRow>, StatsError> {
        let measurements = self.rows(
            "SELECT event, latency_us FROM hook_events",
            params![],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;
        let mut by_event: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for (event, latency) in measurements {
            by_event
                .entry(event)
                .or_default()
                .push(latency.max(0) as u64);
        }
        Ok(by_event
            .into_iter()
            .map(|(event, mut latencies)| {
                latencies.sort_unstable();
                LatencyRow {
                    event,
                    count: latencies.len() as u64,
                    p50_us: nearest_rank(&latencies, 50),
                    p90_us: nearest_rank(&latencies, 90),
                    p99_us: nearest_rank(&latencies, 99),
                    max_us: latencies.last().copied().unwrap_or(0),
                }
            })
            .collect())
    }

    /// Every machine that sent an event, sorted.
    ///
    /// The log is the only record of a machine whose sessions the running server
    /// no longer holds, which is why the machine list is not read from the
    /// context registry alone.
    pub fn machines(&self) -> Result<Vec<String>, StatsError> {
        self.rows(
            "SELECT DISTINCT machine FROM hook_events ORDER BY machine",
            params![],
            |row| row.get::<_, String>(0),
        )
    }

    /// Delivered bytes per session, with full bodies and index lines apart,
    /// sorted by session key.
    pub fn session_bytes(&self) -> Result<Vec<SessionBytesRow>, StatsError> {
        let groups = self.rows(
            "SELECT hook_events.machine, hook_events.session_id, hook_events.agent,
                    deliveries.form, sum(deliveries.bytes)
             FROM deliveries JOIN hook_events ON hook_events.id = deliveries.event_id
             GROUP BY hook_events.machine, hook_events.session_id, hook_events.agent,
                      deliveries.form",
            params![],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )?;
        let mut rows: BTreeMap<String, SessionBytesRow> = BTreeMap::new();
        for (machine, session_id, agent, form, bytes) in groups {
            // The constructor prints the agent only when there is a subagent to
            // name, which is the form the MCP tools take.
            let session_key = ContextKey::subagent(machine, session_id, agent).to_string();
            let row = rows.entry(session_key.clone()).or_insert(SessionBytesRow {
                session_key,
                bytes_full: 0,
                bytes_index: 0,
            });
            let bytes = bytes.max(0) as u64;
            match form.as_str() {
                "full" => row.bytes_full += bytes,
                "index" => row.bytes_index += bytes,
                // A withdrawal carries no content, so it carries no bytes.
                _ => {}
            }
        }
        Ok(rows.into_values().collect())
    }

    /// Every row of `query`, read with `read`.
    fn rows<T>(
        &self,
        query: &str,
        parameters: &[&dyn rusqlite::ToSql],
        read: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, StatsError> {
        let mut statement = self
            .connection
            .prepare(query)
            .map_err(|source| self.failed(source))?;
        let mapped = statement
            .query_map(parameters, |row| read(row))
            .map_err(|source| self.failed(source))?;
        let mut collected = Vec::new();
        for row in mapped {
            collected.push(row.map_err(|source| self.failed(source))?);
        }
        Ok(collected)
    }

    fn failed(&self, source: rusqlite::Error) -> StatsError {
        StatsError::Sqlite {
            path: self.path.clone(),
            source,
        }
    }
}

/// Keep whichever of the two timestamps is the later one.
fn keep_newest(current: &mut Option<String>, candidate: String) {
    let replace = match current.as_deref() {
        None => true,
        Some(kept) => instant_of(&candidate) > instant_of(kept),
    };
    if replace {
        *current = Some(candidate);
    }
}

/// A recorded timestamp as an instant. A timestamp that cannot be read sorts
/// before every readable one, so it is never reported as the newest.
fn instant_of(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|instant| instant.with_timezone(&Utc))
}

/// The nearest-rank percentile of a sorted slice: the value at rank
/// `ceil(percentile * n / 100)`, counting from one.
///
/// Zero for an empty slice, which has no percentile to report.
fn nearest_rank(sorted: &[u64], percentile: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (percentile * sorted.len() as u64)
        .div_ceil(100)
        .clamp(1, sorted.len() as u64);
    sorted[rank as usize - 1]
}

/// The units above bytes, 1024 apart, the way a file manager counts them.
const UNITS_ABOVE_BYTES: [&str; 3] = ["KB", "MB", "GB"];

/// A byte count in the unit that fits it: a plain count below 1024, then one
/// decimal in KB, MB or GB (`24.6 KB`, `1.2 MB`). A count that would print as
/// `1024.0` of a unit moves up to the next one, so far as GB.
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut scaled = bytes as f64 / 1024.0;
    let mut unit = 0;
    let mut tenths = (scaled * 10.0).round() as u64;
    while tenths >= 10_240 && unit + 1 < UNITS_ABOVE_BYTES.len() {
        scaled /= 1024.0;
        unit += 1;
        tenths = (scaled * 10.0).round() as u64;
    }
    format!(
        "{}.{} {}",
        tenths / 10,
        tenths % 10,
        UNITS_ABOVE_BYTES[unit]
    )
}

/// The tables of the statistics database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    HookEvents,
    TriggerFires,
    ScopeForgettings,
    Deliveries,
    ToolCalls,
}

impl Table {
    pub fn as_str(self) -> &'static str {
        match self {
            Table::HookEvents => "hook_events",
            Table::TriggerFires => "trigger_fires",
            Table::ScopeForgettings => "scope_forgettings",
            Table::Deliveries => "deliveries",
            Table::ToolCalls => "tool_calls",
        }
    }
}

/// Open the database and make sure its schema is there.
fn open_connection(path: &Path) -> Result<Connection, StatsError> {
    let to_error = |source: rusqlite::Error| StatsError::Sqlite {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| StatsError::Sqlite {
            path: path.to_path_buf(),
            source: rusqlite::Error::ToSqlConversionFailure(Box::new(error)),
        })?;
    }
    let connection = Connection::open(path).map_err(to_error)?;
    // Write-ahead logging so that a reader (the stats API, a test) never blocks
    // the writer and sees every committed row.
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(to_error)?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS hook_events (
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
                 bytes INTEGER NOT NULL
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
             CREATE INDEX IF NOT EXISTS trigger_fires_event
                 ON trigger_fires (event_id);
             CREATE INDEX IF NOT EXISTS scope_forgettings_event
                 ON scope_forgettings (event_id);
             CREATE INDEX IF NOT EXISTS deliveries_event
                 ON deliveries (event_id);
             CREATE INDEX IF NOT EXISTS deliveries_memory
                 ON deliveries (memory);
             CREATE INDEX IF NOT EXISTS tool_calls_tool
                 ON tool_calls (tool, memory);",
        )
        .map_err(to_error)?;
    Ok(connection)
}

/// Insert records in the order they were queued, each group in one transaction
/// so that an event and its trigger fires and deliveries are never half written.
fn write_records(mut connection: Connection, mut receiver: mpsc::Receiver<Record>) {
    while let Some(record) = receiver.blocking_recv() {
        match record {
            Record::Hook(record) => {
                if let Err(error) = insert_hook_event(&mut connection, &record) {
                    tracing::warn!("statistics write failed: {error}");
                }
            }
            Record::ToolCall(record) => {
                if let Err(error) = insert_tool_call(&connection, &record) {
                    tracing::warn!("statistics write failed: {error}");
                }
            }
            Record::Flush(answer) => {
                let _ = answer.send(());
            }
        }
    }
}

fn insert_hook_event(
    connection: &mut Connection,
    record: &HookEventRecord,
) -> Result<(), rusqlite::Error> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT INTO hook_events
             (ts, machine, session_id, agent, event, context_tokens, latency_us, decision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            record.ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            record.machine,
            record.session_id,
            record.agent,
            record.event,
            record.context_tokens.map(|tokens| tokens as i64),
            record.latency_us as i64,
            record.decision.as_str(),
        ],
    )?;
    let event_id = transaction.last_insert_rowid();
    for fire in &record.triggers {
        transaction.execute(
            "INSERT INTO trigger_fires (event_id, scope_id, field, pattern, activated_new)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event_id,
                fire.scope_id,
                fire.field,
                fire.pattern,
                i64::from(fire.activated_new)
            ],
        )?;
    }
    for forgotten in &record.forgotten {
        transaction.execute(
            "INSERT INTO scope_forgettings
                 (event_id, scope_id, tokens_since_trigger, tokens_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                event_id,
                forgotten.scope_id,
                forgotten.tokens_since_trigger as i64,
                forgotten.tokens_at as i64
            ],
        )?;
    }
    for delivery in &record.deliveries {
        transaction.execute(
            "INSERT INTO deliveries (event_id, memory, kind, form, reason, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event_id,
                delivery.memory,
                delivery.kind,
                delivery.form,
                delivery.reason,
                delivery.bytes as i64
            ],
        )?;
    }
    transaction.commit()
}

fn insert_tool_call(
    connection: &Connection,
    record: &ToolCallRecord,
) -> Result<(), rusqlite::Error> {
    connection
        .execute(
            "INSERT INTO tool_calls (ts, tool, session_key, memory, scope, ok)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                record.tool,
                record.session_key,
                record.memory,
                record.scope,
                i64::from(record.ok)
            ],
        )
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a percentile off by one rank, which is invisible in a report but
    /// turns a latency budget into the wrong answer: with a hundred
    /// measurements of 1 to 100 µs, nearest rank puts p50 at 50 and p99 at 99.
    ///
    /// Expectation source: the nearest-rank definition, the value at rank
    /// `ceil(p * n / 100)` of the sorted measurements, counting from one.
    #[test]
    fn nearest_rank_percentiles_are_the_ranked_measurements() {
        let measurements: Vec<u64> = (1..=100).collect();
        assert_eq!(nearest_rank(&measurements, 50), 50);
        assert_eq!(nearest_rank(&measurements, 90), 90);
        assert_eq!(nearest_rank(&measurements, 99), 99);
    }

    /// Detects percentiles that read past the end of a short series, or report
    /// the same rank for every percentile: two measurements have a p50 of the
    /// lower and a p99 of the higher, and one measurement is every percentile.
    #[test]
    fn a_series_too_short_for_a_percentile_reports_a_measurement_it_has() {
        assert_eq!(nearest_rank(&[10, 20], 50), 10);
        assert_eq!(nearest_rank(&[10, 20], 99), 20);
        assert_eq!(nearest_rank(&[7], 99), 7);
        assert_eq!(
            nearest_rank(&[], 50),
            0,
            "nothing measured has no percentile"
        );
    }

    /// Detects a count printed in the wrong unit, divided by 1000 instead of
    /// 1024, or shown without its one decimal.
    ///
    /// Expectation source: the unit table decided in issue #12, which the
    /// frontend's `formatBytes` is pinned to as well.
    #[test]
    fn a_byte_count_is_shown_in_the_unit_that_fits_it() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(25_205), "24.6 KB");
        assert_eq!(format_bytes(1_258_291), "1.2 MB");
        assert_eq!(format_bytes(2_684_354_560), "2.5 GB");
    }

    /// Detects a count whose decimal rounds up to 1024.0 of a unit keeping that
    /// unit, which is not the unit that fits it.
    #[test]
    fn a_count_that_rounds_up_to_a_full_unit_moves_up_a_unit() {
        assert_eq!(format_bytes(1_048_530), "1.0 MB");
        assert_eq!(format_bytes(1_073_689_396), "1.0 GB");
    }

    /// Detects a newest timestamp chosen by string order across a boundary
    /// where string order and time order differ: the later instant wins, and a
    /// timestamp that cannot be read never beats one that can.
    #[test]
    fn the_newest_delivery_timestamp_is_the_latest_instant() {
        let mut newest = None;
        keep_newest(&mut newest, "2026-01-02T03:04:05+00:00".to_string());
        keep_newest(&mut newest, "2026-01-02T02:04:05-02:00".to_string());
        assert_eq!(
            instant_of(newest.as_deref().expect("a timestamp was kept")),
            instant_of("2026-01-02T04:04:05+00:00"),
            "the later instant must win however it is spelled"
        );
        keep_newest(&mut newest, "not a timestamp".to_string());
        assert_eq!(
            instant_of(newest.as_deref().expect("a timestamp was kept")),
            instant_of("2026-01-02T04:04:05+00:00"),
            "an unreadable timestamp must not be reported as the newest"
        );
    }
}
