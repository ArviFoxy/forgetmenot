//! The scope activation log: one entry each time a scope goes from off to on in
//! a context, with what turned it on.
//!
//! The log answers how a scope came to be active in a session, and follows a
//! scope a subagent inherited back to the entry of the context it came from.
//! What a trigger found is kept with the full text it was found in, once per
//! text however many scopes that text turned on: the text is a row of
//! `activation_messages` and every entry it caused names that row.
//!
//! The columns a query filters on are the same for every cause: time, machine,
//! session, agent, scope and cause. What is particular to one kind of trigger is
//! JSON in two columns, `trigger` for the trigger as its scope file writes it
//! and `evidence` for what it found ([`crate::triggers::Evidence`]), so a new
//! kind of trigger
//! needs no new column.
//!
//! Entries are written by the statistics writer thread, in the order they were
//! queued. An entry that follows from another one names it by id, and the id is
//! looked up when the entry is written: the newest entry of the implying scope
//! in the same context, or of the inherited scope in the parent context. A
//! context's events are answered one after another, so the entry an event or a
//! subagent start follows from was queued before it.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Transaction, params};
use serde::Serialize;

use super::queries::{Window, parameters, stamp};
use super::{StatsError, StatsReader, StatsWriter};
use crate::context::{ContextKey, MAIN_AGENT};
use crate::store::ScopeId;
use crate::store::scope::TriggerField;
use crate::triggers::TriggerEvidence;

/// The most entries one page of the log holds, whatever the caller asks for.
pub const PAGE_LIMIT_MAX: usize = 200;

/// The entries one page holds when the caller does not say.
pub const PAGE_LIMIT_DEFAULT: usize = 50;

/// The most characters of a matched text a listed entry carries. One entry is
/// read whole with [`StatsReader::activation`].
pub const LISTED_MATCH_CHARS: i64 = 200;

/// Why a scope came on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A trigger of the scope matched a text of a hook event.
    Trigger {
        /// The text the trigger was matched against.
        field: TriggerField,
        /// The blob id of the scope's file in the catalog the trigger was
        /// matched from.
        scope_version: Option<String>,
        /// The trigger as written, and what it found.
        found: TriggerEvidence,
        /// The position of the matched text in [`Activations::messages`].
        message: usize,
    },
    /// A scope that came on in the same step implies this one, or a scope that
    /// was already on does and this one had been turned off.
    Implied { by: ScopeId },
    /// A subagent's context was created with the scopes of `parent`.
    Inherited { parent: ContextKey },
    /// `session_scope_on` named the scope.
    SessionScopeOn,
    /// `session_inherit` took the scope over from `source`.
    SessionInherit { source: ContextKey },
}

impl Cause {
    /// The name the log stores and reports the cause under.
    pub fn as_str(&self) -> &'static str {
        match self {
            Cause::Trigger { .. } => "trigger",
            Cause::Implied { .. } => "implied",
            Cause::Inherited { .. } => "inherited",
            Cause::SessionScopeOn => "session_scope_on",
            Cause::SessionInherit { .. } => "session_inherit",
        }
    }
}

/// One scope that came on, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub scope: ScopeId,
    pub cause: Cause,
}

/// What one step turned on in one context: the entries in the order they are
/// written, and the texts the trigger entries among them were found in.
///
/// An implied entry comes after the entry of the scope that implies it, so the
/// id it names is already written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activations {
    pub messages: Vec<String>,
    pub entries: Vec<Activation>,
}

impl Activations {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// What a step that is not a hook event turned on in one context.
#[derive(Clone, Debug)]
pub struct ActivationRecord {
    pub ts: DateTime<Utc>,
    pub key: ContextKey,
    pub activations: Activations,
}

/// Where the activation entries of a step go: the statistics writer when the
/// log is on, nowhere when the server was started with it off.
#[derive(Clone)]
pub struct ActivationLog {
    writer: Option<StatsWriter>,
}

impl ActivationLog {
    /// The log written through `writer`.
    pub fn on(writer: StatsWriter) -> Self {
        Self {
            writer: Some(writer),
        }
    }

    /// The log of a server that records nothing.
    pub fn off() -> Self {
        Self { writer: None }
    }

    pub fn is_on(&self) -> bool {
        self.writer.is_some()
    }

    /// Queue what one step turned on; nothing when the log is off or the step
    /// turned nothing on.
    pub async fn record(&self, record: ActivationRecord) {
        if record.activations.is_empty() {
            return;
        }
        if let Some(writer) = &self.writer {
            writer.record_activations(record).await;
        }
    }
}

/// The entries for the scopes of `after` that neither `before` nor `direct`
/// holds, each named after the scope that implies it.
///
/// The scope named is the first of `direct` that implies it, else the first
/// scope of `before` that does: a scope can come back on through a scope that
/// was on all along when it had been turned off by itself.
pub fn implied_activations(
    implies: impl Fn(&ScopeId, &ScopeId) -> bool,
    before: &BTreeSet<ScopeId>,
    direct: &[ScopeId],
    after: &BTreeSet<ScopeId>,
) -> Vec<Activation> {
    after
        .iter()
        .filter(|scope| !before.contains(*scope) && !direct.contains(*scope))
        .filter_map(|scope| {
            let by = direct
                .iter()
                .chain(before.iter())
                .find(|candidate| implies(candidate, scope))?;
            Some(Activation {
                scope: scope.clone(),
                cause: Cause::Implied { by: by.clone() },
            })
        })
        .collect()
}

/// Write what one step turned on in the context `key`, inside the writer's
/// transaction.
pub(super) fn insert(
    transaction: &Transaction<'_>,
    ts: &str,
    key: &ContextKey,
    activations: &Activations,
) -> Result<(), rusqlite::Error> {
    let mut message_ids = Vec::with_capacity(activations.messages.len());
    for text in &activations.messages {
        transaction.execute(
            "INSERT INTO activation_messages (text, chars) VALUES (?1, ?2)",
            params![text, text.chars().count() as i64],
        )?;
        message_ids.push(transaction.last_insert_rowid());
    }
    for entry in &activations.entries {
        let mut row = EntryRow {
            cause: entry.cause.as_str(),
            ..EntryRow::default()
        };
        match &entry.cause {
            Cause::Trigger {
                field,
                scope_version,
                found,
                message,
            } => {
                row.trigger_kind = Some(found.evidence.kind());
                row.trigger = Some(to_json(&found.trigger)?);
                row.evidence = Some(to_json(&found.evidence)?);
                row.scope_version = scope_version.clone();
                row.field = Some(field.as_str());
                row.message_id = Some(message_ids[*message]);
            }
            Cause::Implied { by } => {
                row.cause_scope = Some(by.to_string());
                row.cause_entry_id = newest_entry(transaction, key, by)?;
            }
            Cause::Inherited { parent } => {
                row.cause_context = Some(parent.to_string());
                row.cause_entry_id = newest_entry(transaction, parent, &entry.scope)?;
            }
            Cause::SessionScopeOn => {}
            Cause::SessionInherit { source } => {
                row.cause_context = Some(source.to_string());
            }
        }
        transaction.execute(
            "INSERT INTO scope_activations
                 (ts, machine, session_id, agent, scope_id, cause, trigger_kind, trigger,
                  scope_version, field, evidence, message_id, cause_scope, cause_entry_id,
                  cause_context)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                ts,
                key.machine,
                key.session_id,
                key.agent,
                entry.scope.as_str(),
                row.cause,
                row.trigger_kind,
                row.trigger,
                row.scope_version,
                row.field,
                row.evidence,
                row.message_id,
                row.cause_scope,
                row.cause_entry_id,
                row.cause_context,
            ],
        )?;
    }
    Ok(())
}

/// The columns of one entry that depend on its cause.
#[derive(Default)]
struct EntryRow {
    cause: &'static str,
    trigger_kind: Option<&'static str>,
    trigger: Option<String>,
    evidence: Option<String>,
    scope_version: Option<String>,
    field: Option<&'static str>,
    message_id: Option<i64>,
    cause_scope: Option<String>,
    cause_entry_id: Option<i64>,
    cause_context: Option<String>,
}

fn to_json(value: &impl Serialize) -> Result<String, rusqlite::Error> {
    serde_json::to_string(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

/// The newest entry of `scope` in the context `key`, absent when the log holds
/// none: the scope came on before the log recorded it, or by being one of the
/// scopes every context starts with.
fn newest_entry(
    transaction: &Transaction<'_>,
    key: &ContextKey,
    scope: &ScopeId,
) -> Result<Option<i64>, rusqlite::Error> {
    transaction.query_row(
        "SELECT max(id) FROM scope_activations
         WHERE session_id = ?1 AND scope_id = ?2 AND machine = ?3 AND agent = ?4",
        params![key.session_id, scope.as_str(), key.machine, key.agent],
        |row| row.get::<_, Option<i64>>(0),
    )
}

/// Delete every entry recorded before `cutoff`, then every text that only those
/// entries named.
pub(super) fn prune(
    connection: &mut rusqlite::Connection,
    cutoff: DateTime<Utc>,
) -> Result<(), rusqlite::Error> {
    let cutoff = stamp(cutoff);
    let transaction = connection.transaction()?;
    let named: Vec<i64> = {
        let mut statement = transaction.prepare(
            "SELECT DISTINCT message_id FROM scope_activations
             WHERE ts < ?1 AND message_id IS NOT NULL",
        )?;
        let rows = statement.query_map(params![cutoff], |row| row.get::<_, i64>(0))?;
        rows.collect::<Result<_, _>>()?
    };
    transaction.execute(
        "DELETE FROM scope_activations WHERE ts < ?1",
        params![cutoff],
    )?;
    for message_id in named {
        transaction.execute(
            "DELETE FROM activation_messages WHERE id = ?1
             AND NOT EXISTS (SELECT 1 FROM scope_activations WHERE message_id = ?1)",
            params![message_id],
        )?;
    }
    transaction.commit()
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Which end of the log a page starts from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    Oldest,
    #[default]
    Newest,
}

/// Which entries to list.
#[derive(Clone, Debug)]
pub struct ActivationFilter {
    /// One context. A main context's key reads the subagents of its session as
    /// well when `include_subagents` says so; a subagent's key reads that
    /// subagent alone.
    pub context: Option<ContextKey>,
    pub include_subagents: bool,
    pub scope: Option<String>,
    /// Both ends included, at the whole seconds the log records.
    pub window: Window,
    pub order: Order,
    /// At most [`PAGE_LIMIT_MAX`].
    pub limit: usize,
    /// Only entries with an id below this.
    pub before: Option<i64>,
    /// Only entries with an id above this.
    pub after: Option<i64>,
}

impl Default for ActivationFilter {
    fn default() -> Self {
        Self {
            context: None,
            include_subagents: true,
            scope: None,
            window: Window::default(),
            order: Order::default(),
            limit: PAGE_LIMIT_DEFAULT,
            before: None,
            after: None,
        }
    }
}

/// One entry of the log, without the text a trigger matched.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ActivationEntry {
    pub id: i64,
    /// When the scope came on, as the log wrote it.
    pub time: String,
    /// The context the scope came on in, as `machine/session-id` or
    /// `machine/session-id/agent-id`.
    pub context: String,
    /// The kind of subagent the context is, when the server still holds the
    /// context and was told.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// The task the subagent was given, under the same condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub scope: String,
    #[serde(flatten)]
    pub cause: CauseDetails,
}

/// What an entry records about its cause, tagged with the cause.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum CauseDetails {
    Trigger {
        /// `regex`, the kind of the trigger.
        trigger_kind: String,
        /// The trigger as its scope file writes it.
        trigger: serde_json::Value,
        /// The blob id of the scope's file when the trigger matched.
        scope_version: Option<String>,
        /// The text of the hook event the trigger matched.
        field: String,
        /// What the trigger found. Where it carries `start` and `end` it
        /// carries `matched` as well, the text between them.
        evidence: serde_json::Value,
        /// The id of the stored text, which [`StatsReader::activation`] returns
        /// whole.
        message_id: i64,
        /// The length of that text in characters.
        message_chars: u64,
    },
    Implied {
        /// The scope that implies this one.
        implied_by: String,
        /// That scope's newest entry in the same context when this one was
        /// written; absent when the log holds none.
        implied_by_entry: Option<i64>,
    },
    Inherited {
        /// The context the subagent was created from.
        parent_context: String,
        /// The parent's newest entry for this scope when the subagent was
        /// created; absent when the parent's activation predates the log.
        parent_entry: Option<i64>,
    },
    SessionScopeOn,
    SessionInherit {
        /// The session the scope was taken over from.
        source_context: String,
    },
}

/// One page of the log.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ActivationPage {
    pub entries: Vec<ActivationEntry>,
    /// The cursor of the next page when there is one: `before` for the newest
    /// first, `after` for the oldest first.
    pub next: Option<i64>,
}

/// One entry with the whole text its trigger matched.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ActivationDetail {
    #[serde(flatten)]
    pub entry: ActivationEntry,
    /// The text of the hook event, as it was matched; absent for an entry no
    /// trigger made.
    pub message: Option<String>,
}

/// The columns every read of an entry selects, in the order [`entry_of`] reads
/// them. The matched text is cut to the length bound as the first parameter.
const ENTRY_COLUMNS: &str = "
    scope_activations.id, scope_activations.ts, scope_activations.machine,
    scope_activations.session_id, scope_activations.agent, scope_activations.scope_id,
    scope_activations.cause, scope_activations.trigger_kind, scope_activations.trigger,
    scope_activations.scope_version, scope_activations.field, scope_activations.evidence,
    scope_activations.message_id, activation_messages.chars,
    scope_activations.cause_scope, scope_activations.cause_entry_id,
    scope_activations.cause_context,
    CASE WHEN json_extract(scope_activations.evidence, '$.start') IS NOT NULL
              AND json_extract(scope_activations.evidence, '$.end') IS NOT NULL
         THEN substr(activation_messages.text,
                     json_extract(scope_activations.evidence, '$.start') + 1,
                     min(json_extract(scope_activations.evidence, '$.end')
                         - json_extract(scope_activations.evidence, '$.start'), ?1))
    END";

/// The join every read of an entry reads from.
const ENTRY_SOURCE: &str = "scope_activations
    LEFT JOIN activation_messages ON activation_messages.id = scope_activations.message_id";

/// One entry read from the columns of [`ENTRY_COLUMNS`].
fn entry_of(row: &rusqlite::Row<'_>) -> Result<ActivationEntry, rusqlite::Error> {
    let json = |index: usize| -> Result<serde_json::Value, rusqlite::Error> {
        let text: String = row.get(index)?;
        serde_json::from_str(&text).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    };
    let cause: String = row.get(6)?;
    let details = match cause.as_str() {
        "trigger" => {
            let mut evidence = json(11)?;
            let matched: Option<String> = row.get(17)?;
            if let (Some(object), Some(matched)) = (evidence.as_object_mut(), matched) {
                object.insert("matched".to_string(), serde_json::Value::String(matched));
            }
            CauseDetails::Trigger {
                trigger_kind: row.get(7)?,
                trigger: json(8)?,
                scope_version: row.get(9)?,
                field: row.get(10)?,
                evidence,
                message_id: row.get(12)?,
                message_chars: row.get::<_, i64>(13)?.max(0) as u64,
            }
        }
        "implied" => CauseDetails::Implied {
            implied_by: row.get(14)?,
            implied_by_entry: row.get(15)?,
        },
        "inherited" => CauseDetails::Inherited {
            parent_context: row.get(16)?,
            parent_entry: row.get(15)?,
        },
        "session_scope_on" => CauseDetails::SessionScopeOn,
        "session_inherit" => CauseDetails::SessionInherit {
            source_context: row.get(16)?,
        },
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                format!("`{other}` is not a cause of activation").into(),
            ));
        }
    };
    Ok(ActivationEntry {
        id: row.get(0)?,
        time: row.get(1)?,
        context: ContextKey::subagent(
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        )
        .to_string(),
        agent_type: None,
        task: None,
        scope: row.get(5)?,
        cause: details,
    })
}

impl StatsReader {
    /// One page of the log, in the order the filter asks for, with the cursor
    /// of the page after it.
    pub fn activations(&self, filter: &ActivationFilter) -> Result<ActivationPage, StatsError> {
        let mut conditions = Vec::new();
        let mut values = vec![SqlValue::Integer(LISTED_MATCH_CHARS)];
        if let Some(context) = &filter.context {
            conditions.push("scope_activations.machine = ?");
            values.push(SqlValue::Text(context.machine.clone()));
            conditions.push("scope_activations.session_id = ?");
            values.push(SqlValue::Text(context.session_id.clone()));
            if context.is_subagent() || !filter.include_subagents {
                conditions.push("scope_activations.agent = ?");
                values.push(SqlValue::Text(context.agent.clone()));
            }
        } else if !filter.include_subagents {
            conditions.push("scope_activations.agent = ?");
            values.push(SqlValue::Text(MAIN_AGENT.to_string()));
        }
        if let Some(scope) = &filter.scope {
            conditions.push("scope_activations.scope_id = ?");
            values.push(SqlValue::Text(scope.clone()));
        }
        if let Some(from) = filter.window.from {
            conditions.push("scope_activations.ts >= ?");
            values.push(SqlValue::Text(stamp(from)));
        }
        if let Some(to) = filter.window.to {
            conditions.push("scope_activations.ts <= ?");
            values.push(SqlValue::Text(stamp(to)));
        }
        if let Some(before) = filter.before {
            conditions.push("scope_activations.id < ?");
            values.push(SqlValue::Integer(before));
        }
        if let Some(after) = filter.after {
            conditions.push("scope_activations.id > ?");
            values.push(SqlValue::Integer(after));
        }
        let clause = match conditions.is_empty() {
            true => String::new(),
            false => format!(" WHERE {}", conditions.join(" AND ")),
        };
        let direction = match filter.order {
            Order::Oldest => "ASC",
            Order::Newest => "DESC",
        };
        let limit = filter.limit.min(PAGE_LIMIT_MAX);
        // One more than the page, which is what says whether there is a next.
        values.push(SqlValue::Integer(limit as i64 + 1));
        let mut entries = self.rows(
            &format!(
                "SELECT {ENTRY_COLUMNS} FROM {ENTRY_SOURCE}{clause}
                 ORDER BY scope_activations.id {direction} LIMIT ?"
            ),
            &parameters(&values),
            entry_of,
        )?;
        let next = match entries.len() > limit {
            true => {
                entries.truncate(limit);
                entries.last().map(|entry| entry.id)
            }
            false => None,
        };
        Ok(ActivationPage { entries, next })
    }

    /// One entry with the whole text its trigger matched, absent when the log
    /// holds no entry with this id.
    pub fn activation(&self, id: i64) -> Result<Option<ActivationDetail>, StatsError> {
        let rows = self.rows(
            &format!(
                "SELECT {ENTRY_COLUMNS}, activation_messages.text FROM {ENTRY_SOURCE}
                 WHERE scope_activations.id = ?2"
            ),
            &parameters(&[SqlValue::Integer(i64::MAX), SqlValue::Integer(id)]),
            |row| {
                Ok(ActivationDetail {
                    entry: entry_of(row)?,
                    message: row.get(18)?,
                })
            },
        )?;
        Ok(rows.into_iter().next())
    }
}
