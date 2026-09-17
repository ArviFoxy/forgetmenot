//! Reading the log over a span of time: what was delivered, when, and to whom.
//!
//! Every query here reads `hook_events` and the rows that hang off it, narrowed
//! by a [`Filter`]: a span of time, one session, one scope. A delivery's cost is
//! the `chars` its row recorded, which is the length of the text the renderer
//! printed for it; nothing is derived from the catalog or from a set of scopes.
//!
//! Characters become tokens in exactly one place, [`tokens_of`], so the number a
//! page shows and the number the command prints come from the same arithmetic.

use chrono::{DateTime, Duration, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{ToSql, params_from_iter};
use serde::Serialize;

use super::{StatsError, StatsReader};
use crate::context::ContextKey;

/// Tokens for a length of delivered text: `chars / characters_per_token`,
/// rounded up.
///
/// The only conversion in the server. A divisor that is not a positive number
/// would make every figure infinite or negative, so it is read as one character
/// per token, which is the most a text can cost.
pub fn tokens_of(chars: u64, characters_per_token: f64) -> u64 {
    if !characters_per_token.is_finite() || characters_per_token <= 0.0 {
        return chars;
    }
    (chars as f64 / characters_per_token).ceil() as u64
}

/// The span of time a query covers. An absent bound is open at that end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

impl Window {
    /// The window from `from` to `to`, both ends included.
    pub fn between(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        Self {
            from: Some(from),
            to: Some(to),
        }
    }

    /// The window that reaches `length` back from `now`.
    pub fn last(length: Duration, now: DateTime<Utc>) -> Self {
        Self::between(now - length, now)
    }
}

/// Which rows a query reads: a span of time, one session, one scope.
///
/// The default reads the whole log, which is what a request that names no
/// filter asks for.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub window: Window,
    /// One context, by the key the MCP tools take.
    pub session: Option<ContextKey>,
    /// One scope, matched against the section a memory was printed under.
    pub scope: Option<String>,
}

impl Filter {
    /// The filter that reads the whole log.
    pub fn all() -> Self {
        Self::default()
    }

    /// The same filter over one window.
    pub fn over(window: Window) -> Self {
        Self {
            window,
            ..Self::default()
        }
    }

    /// The `WHERE` clause and its values for a query over `hook_events`, with
    /// the scope filter applied to the delivery rows when `deliveries` says the
    /// query joins them.
    ///
    /// A query that does not read delivery rows cannot honour a scope filter,
    /// so it is given none: `deliveries` is what says which of the two a query
    /// is.
    pub(crate) fn clause(&self, deliveries: bool) -> (String, Vec<SqlValue>) {
        let mut conditions = Vec::new();
        let mut values = Vec::new();
        self.push_window(&mut conditions, &mut values, "hook_events.ts");
        self.push_session(&mut conditions, &mut values);
        if deliveries && let Some(scope) = &self.scope {
            conditions.push("deliveries.scope = ?".to_string());
            values.push(SqlValue::Text(scope.clone()));
        }
        (where_of(&conditions), values)
    }

    /// The same for a query over `tool_calls`, which names its context by the
    /// session key rather than by the three columns and knows no scope.
    pub(crate) fn tool_call_clause(&self) -> (String, Vec<SqlValue>) {
        let mut conditions = Vec::new();
        let mut values = Vec::new();
        self.push_window(&mut conditions, &mut values, "tool_calls.ts");
        if let Some(session) = &self.session {
            conditions.push("tool_calls.session_key = ?".to_string());
            values.push(SqlValue::Text(session.to_string()));
        }
        (where_of(&conditions), values)
    }

    fn push_window(&self, conditions: &mut Vec<String>, values: &mut Vec<SqlValue>, column: &str) {
        if let Some(from) = self.window.from {
            conditions.push(format!("{column} >= ?"));
            values.push(SqlValue::Text(stamp(from)));
        }
        if let Some(to) = self.window.to {
            conditions.push(format!("{column} <= ?"));
            values.push(SqlValue::Text(stamp(to)));
        }
    }

    fn push_session(&self, conditions: &mut Vec<String>, values: &mut Vec<SqlValue>) {
        let Some(session) = &self.session else {
            return;
        };
        conditions.push(
            "hook_events.machine = ? AND hook_events.session_id = ? AND hook_events.agent = ?"
                .to_string(),
        );
        values.push(SqlValue::Text(session.machine.clone()));
        values.push(SqlValue::Text(session.session_id.clone()));
        values.push(SqlValue::Text(session.agent.clone()));
    }
}

/// A timestamp in the form the log writes, so that a bound compares against a
/// recorded timestamp as text.
///
/// The log records whole seconds, so a bound is compared at that resolution: a
/// sub-second part is dropped rather than rounded, at both ends.
fn stamp(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The conditions as a `WHERE` clause, empty when there are none.
fn where_of(conditions: &[String]) -> String {
    match conditions.is_empty() {
        true => String::new(),
        false => format!(" WHERE {}", conditions.join(" AND ")),
    }
}

/// The values of a clause as query parameters.
pub(crate) fn parameters(values: &[SqlValue]) -> Vec<&dyn ToSql> {
    values.iter().map(|value| value as &dyn ToSql).collect()
}

/// How long the windows of the summary reach back, with the name each is
/// reported under.
pub const SUMMARY_WINDOWS: [(&str, i64); 4] =
    [("5m", 300), ("1h", 3_600), ("1d", 86_400), ("7d", 604_800)];

/// What happened in one window of the summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SummaryRow {
    /// The window's name, one of [`SUMMARY_WINDOWS`].
    pub name: String,
    /// Characters of delivered text, which [`tokens_of`] turns into tokens.
    pub chars: u64,
    pub events: u64,
    /// Tool calls the server stopped so a critical memory could be read first.
    pub held: u64,
    /// Scopes that turned themselves off because their `forget` rule was
    /// reached.
    pub forgettings: u64,
}

/// One bucket of the delivered-tokens series.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SeriesPoint {
    /// The bucket's start, as RFC 3339 in UTC.
    pub t: String,
    /// Characters of delivered text in this bucket.
    pub chars: u64,
    /// Deliveries that put text in an answer; a withdrawal and a memory that
    /// only shrank are rows of the log but carry no text, so neither is one.
    pub deliveries: u64,
}

/// One hook event of one context, as the per-session chart reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionEventRow {
    /// When the event was answered, as the log wrote it.
    pub t: String,
    pub event: String,
    /// The context size Claude Code reported at this event, absent when the
    /// event carried none. Claude Code's own measurement, never fitted to the
    /// figure beside it.
    pub context_tokens: Option<u64>,
    /// Characters the answer to this event carried, which [`tokens_of`] turns
    /// into tokens.
    pub answer_chars: u64,
}

/// How long a bucket of the series is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bucket {
    Minute,
    Hour,
    Day,
}

/// The fewest points a bucket may give and still be read as a series rather
/// than as a handful of bars.
const FEWEST_POINTS: i64 = 20;

/// The most points a bucket may give: past this a line is denser than a chart
/// can show and the answer is larger than the page needs.
const MOST_POINTS: i64 = 200;

impl Bucket {
    /// The bucket of this name, or `None` when the name is not one of them.
    /// `auto` is not a bucket; it is a request for [`Bucket::automatic`].
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "minute" => Some(Bucket::Minute),
            "hour" => Some(Bucket::Hour),
            "day" => Some(Bucket::Day),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Minute => "minute",
            Bucket::Hour => "hour",
            Bucket::Day => "day",
        }
    }

    pub fn seconds(self) -> i64 {
        match self {
            Bucket::Minute => 60,
            Bucket::Hour => 3_600,
            Bucket::Day => 86_400,
        }
    }

    /// The next bucket up, `None` for the coarsest.
    fn coarser(self) -> Option<Self> {
        match self {
            Bucket::Minute => Some(Bucket::Hour),
            Bucket::Hour => Some(Bucket::Day),
            Bucket::Day => None,
        }
    }

    /// The bucket for a window of `span`: the coarsest that gives at least
    /// [`FEWEST_POINTS`] points, stepped up again while it would give more than
    /// [`MOST_POINTS`].
    ///
    /// A window too short for any bucket to reach the floor is bucketed by the
    /// minute, which is the finest there is, and a window so long that even days
    /// pass the ceiling is bucketed by the day, because nothing is coarser.
    pub fn automatic(span: Duration) -> Self {
        let seconds = span.num_seconds().max(0);
        let mut chosen = Bucket::Minute;
        for bucket in [Bucket::Day, Bucket::Hour, Bucket::Minute] {
            if seconds / bucket.seconds() >= FEWEST_POINTS {
                chosen = bucket;
                break;
            }
        }
        while seconds / chosen.seconds() > MOST_POINTS {
            match chosen.coarser() {
                Some(coarser) => chosen = coarser,
                None => break,
            }
        }
        chosen
    }

    /// The SQL that names a row's bucket by its start, as RFC 3339 in UTC.
    ///
    /// Timestamps are written as RFC 3339 in UTC, so the leading characters of
    /// one are the day, the hour and the minute it falls in.
    fn start_expression(self) -> &'static str {
        match self {
            Bucket::Minute => "substr(hook_events.ts, 1, 16) || ':00Z'",
            Bucket::Hour => "substr(hook_events.ts, 1, 13) || ':00:00Z'",
            Bucket::Day => "substr(hook_events.ts, 1, 10) || 'T00:00:00Z'",
        }
    }
}

impl StatsReader {
    /// What was delivered, answered, held and forgotten in each of the
    /// [`SUMMARY_WINDOWS`], counted back from `now`.
    pub fn summary(&self, now: DateTime<Utc>) -> Result<Vec<SummaryRow>, StatsError> {
        let mut rows = Vec::new();
        for (name, seconds) in SUMMARY_WINDOWS {
            let filter = Filter::over(Window::last(Duration::seconds(seconds), now));
            let (events_where, events_values) = filter.clause(false);
            let (delivery_where, delivery_values) = filter.clause(true);
            let chars = self.value(
                &format!(
                    "SELECT coalesce(sum(deliveries.chars), 0) FROM deliveries
                     JOIN hook_events ON hook_events.id = deliveries.event_id{delivery_where}"
                ),
                &parameters(&delivery_values),
            )?;
            let events = self.value(
                &format!("SELECT count(*) FROM hook_events{events_where}"),
                &parameters(&events_values),
            )?;
            let held = self.value(
                &format!(
                    "SELECT sum(CASE WHEN hook_events.decision = 'deny' THEN 1 ELSE 0 END)
                     FROM hook_events{events_where}"
                ),
                &parameters(&events_values),
            )?;
            let forgettings = self.value(
                &format!(
                    "SELECT count(*) FROM scope_forgettings
                     JOIN hook_events ON hook_events.id = scope_forgettings.event_id{events_where}"
                ),
                &parameters(&events_values),
            )?;
            rows.push(SummaryRow {
                name: name.to_string(),
                chars,
                events,
                held,
                forgettings,
            });
        }
        Ok(rows)
    }

    /// Delivered characters and deliveries per bucket, oldest first.
    ///
    /// A bucket nothing was delivered in is left out: the series is what was
    /// sent, and a page draws the gaps.
    pub fn series(&self, filter: &Filter, bucket: Bucket) -> Result<Vec<SeriesPoint>, StatsError> {
        let (clause, values) = filter.clause(true);
        let start = bucket.start_expression();
        self.rows(
            &format!(
                "SELECT {start} AS bucket_start,
                        coalesce(sum(deliveries.chars), 0),
                        sum(CASE WHEN deliveries.chars > 0 THEN 1 ELSE 0 END)
                 FROM deliveries JOIN hook_events ON hook_events.id = deliveries.event_id{clause}
                 GROUP BY bucket_start
                 ORDER BY bucket_start"
            ),
            &parameters(&values),
            |row| {
                Ok(SeriesPoint {
                    t: row.get(0)?,
                    chars: row.get::<_, i64>(1)?.max(0) as u64,
                    deliveries: row.get::<_, i64>(2)?.max(0) as u64,
                })
            },
        )
    }

    /// The span the events this filter selects cover, `None` when it selects
    /// none.
    ///
    /// What `auto` chooses its bucket from when the request names no window: the
    /// series covers the log, so the bucket has to fit the log.
    pub fn span(&self, filter: &Filter) -> Result<Option<(String, String)>, StatsError> {
        let (clause, values) = filter.clause(false);
        let bounds = self.rows(
            &format!("SELECT min(hook_events.ts), max(hook_events.ts) FROM hook_events{clause}"),
            &parameters(&values),
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )?;
        Ok(match bounds.into_iter().next() {
            Some((Some(first), Some(last))) => Some((first, last)),
            _ => None,
        })
    }

    /// Every hook event of one context, oldest first, with the context size it
    /// reported and the characters its answer carried.
    ///
    /// Empty for a context the log has never seen, which is how a caller tells
    /// an unknown key from a quiet one.
    pub fn session_series(
        &self,
        key: &ContextKey,
        window: Window,
    ) -> Result<Vec<SessionEventRow>, StatsError> {
        let filter = Filter {
            window,
            session: Some(key.clone()),
            scope: None,
        };
        let (clause, values) = filter.clause(false);
        self.rows(
            &format!(
                "SELECT hook_events.ts, hook_events.event, hook_events.context_tokens,
                        coalesce(hook_events.answer_chars, 0)
                 FROM hook_events{clause}
                 ORDER BY hook_events.id"
            ),
            &parameters(&values),
            |row| {
                Ok(SessionEventRow {
                    t: row.get(0)?,
                    event: row.get(1)?,
                    context_tokens: row
                        .get::<_, Option<i64>>(2)?
                        .map(|tokens| tokens.max(0) as u64),
                    answer_chars: row.get::<_, i64>(3)?.max(0) as u64,
                })
            },
        )
    }

    /// One number out of `query`, zero when the query has no rows to read.
    fn value(&self, query: &str, parameters: &[&dyn ToSql]) -> Result<u64, StatsError> {
        let counted = self.rows(query, parameters, |row| row.get::<_, Option<i64>>(0))?;
        Ok(counted.into_iter().next().flatten().unwrap_or(0).max(0) as u64)
    }

    /// Every row of `query`, read with `read`, with the parameters bound in
    /// order.
    pub(super) fn rows<T>(
        &self,
        query: &str,
        parameters: &[&dyn ToSql],
        read: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, StatsError> {
        let mut statement = self
            .connection()
            .prepare(query)
            .map_err(|source| self.failed(source))?;
        let mapped = statement
            .query_map(params_from_iter(parameters.iter()), |row| read(row))
            .map_err(|source| self.failed(source))?;
        let mut collected = Vec::new();
        for row in mapped {
            collected.push(row.map_err(|source| self.failed(source))?);
        }
        Ok(collected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a conversion that truncates instead of rounding up, which would
    /// report a text that costs a token as costing none, and one that does not
    /// divide by the store's figure at all.
    ///
    /// Expectation source: the definition, `ceil(chars / characters_per_token)`,
    /// with the default divisor of 3.5, Anthropic's published figure for Claude.
    #[test]
    fn tokens_are_the_characters_over_the_divisor_rounded_up() {
        assert_eq!(tokens_of(0, 3.5), 0, "nothing delivered costs nothing");
        assert_eq!(tokens_of(1, 3.5), 1, "a character is already a token");
        assert_eq!(tokens_of(7, 3.5), 2, "two tokens exactly");
        assert_eq!(tokens_of(8, 3.5), 3, "a part of a token is a token");
        assert_eq!(tokens_of(35_000, 3.5), 10_000);
        assert_eq!(tokens_of(12, 4.0), 3, "another divisor is another figure");
        assert_eq!(tokens_of(12, 1.0), 12);
    }

    /// Detects a divisor of zero or below turning every figure into infinity, a
    /// negative count, or a panic: the setting is checked on the way in, and a
    /// file edited by hand can still carry one.
    #[test]
    fn a_divisor_that_is_not_a_positive_number_costs_a_token_per_character() {
        assert_eq!(tokens_of(10, 0.0), 10);
        assert_eq!(tokens_of(10, -3.5), 10);
        assert_eq!(tokens_of(10, f64::NAN), 10);
    }

    /// Detects a bucket that gives a line of two points for a week, or one point
    /// per minute for a month, either of which makes the chart unreadable.
    ///
    /// Expectation source: the rule that `auto` takes the coarsest bucket giving
    /// at least 20 points, stepping up again past 200: a week is 168 hours, a
    /// month is 30 days, an hour is 60 minutes.
    #[test]
    fn the_automatic_bucket_is_the_coarsest_one_that_gives_a_readable_number_of_points() {
        assert_eq!(Bucket::automatic(Duration::days(7)), Bucket::Hour);
        assert_eq!(Bucket::automatic(Duration::days(30)), Bucket::Day);
        assert_eq!(Bucket::automatic(Duration::hours(24)), Bucket::Hour);
        assert_eq!(Bucket::automatic(Duration::hours(1)), Bucket::Minute);
        assert_eq!(Bucket::automatic(Duration::hours(3)), Bucket::Minute);
    }

    /// Detects a window too short for any bucket to reach twenty points
    /// answered with nothing or with days, and one so long that days pass two
    /// hundred points answered with hours: the finest and the coarsest bucket
    /// are the ends of what there is.
    #[test]
    fn a_window_no_bucket_fits_is_given_the_finest_or_the_coarsest_bucket_there_is() {
        assert_eq!(Bucket::automatic(Duration::minutes(10)), Bucket::Minute);
        assert_eq!(Bucket::automatic(Duration::seconds(0)), Bucket::Minute);
        assert_eq!(Bucket::automatic(Duration::days(365 * 3)), Bucket::Day);
        assert_eq!(
            Bucket::automatic(Duration::hours(5)),
            Bucket::Hour,
            "300 minutes is past the ceiling, so the next bucket up is taken"
        );
    }

    /// Detects a bucket name the API accepts that the query has no notion of,
    /// and `auto` read as a bucket of its own rather than as a request to
    /// choose one.
    #[test]
    fn only_the_three_bucket_names_are_buckets_and_auto_is_not_one_of_them() {
        assert_eq!(Bucket::parse("minute"), Some(Bucket::Minute));
        assert_eq!(Bucket::parse("hour"), Some(Bucket::Hour));
        assert_eq!(Bucket::parse("day"), Some(Bucket::Day));
        assert_eq!(Bucket::parse("auto"), None);
        assert_eq!(Bucket::parse("week"), None);
        assert_eq!(Bucket::parse(""), None);
    }
}
