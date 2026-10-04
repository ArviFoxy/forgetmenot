//! Server configuration, read once at start.
//!
//! Only the infrastructure is here: where the server listens, what it reads and
//! writes, and how long it keeps things. Everything that changes what an agent
//! experiences lives in the store's own `config.yml`, so that it is versioned,
//! reviewable and the same for every machine the server serves.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

/// The default delay between a change to the context registry and the snapshot
/// that records it.
pub const DEFAULT_SNAPSHOT_DEBOUNCE_MS: u64 = 1_000;

/// The port the server listens on unless `--listen` says otherwise.
pub const DEFAULT_PORT: u16 = 8731;

/// Settings read once at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Directory of the git repository holding scopes and memories.
    pub store_path: PathBuf,
    /// Address the HTTP server binds.
    pub listen: SocketAddr,
    /// File the context registry is snapshotted to.
    pub state_path: PathBuf,
    /// The sqlite database statistics are appended to.
    pub stats_path: PathBuf,
    /// Directory of the built frontend; nothing is served at `/` without it.
    pub web_dist: Option<PathBuf>,
    /// Host header values accepted on `/mcp`.
    pub allowed_hosts: Vec<String>,
    /// How long a change waits for further changes before the registry is
    /// written; zero writes on every change, which is what tests use.
    pub snapshot_debounce_ms: u64,
    /// Contexts not seen for this long are dropped; unset keeps them forever.
    pub context_retention_days: Option<u64>,
    /// Transaction branches not written to for this long are deleted; unset
    /// keeps them forever.
    pub branch_retention_days: Option<u64>,
    /// Whether every scope that comes on in a context is recorded in the
    /// activation log, with the full text of the message that turned it on.
    pub activation_log: bool,
    /// Activation log entries older than this are deleted, and the texts no
    /// remaining entry names; unset keeps them forever.
    pub activation_log_retention_days: Option<u64>,
}

impl Config {
    /// Configuration for a store at `store_path`, with the state file, the
    /// statistics database and the listen address at their defaults.
    ///
    /// The state file and the statistics database sit beside the store rather
    /// than inside it: neither belongs in the git repository the store is.
    pub fn new(store_path: impl AsRef<Path>) -> Self {
        let store_path = store_path.as_ref().to_path_buf();
        Self {
            state_path: store_path.with_file_name("forgetmenot-state.json"),
            stats_path: store_path.with_file_name("forgetmenot-stats.sqlite"),
            store_path,
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)),
            web_dist: None,
            allowed_hosts: Vec::new(),
            snapshot_debounce_ms: DEFAULT_SNAPSHOT_DEBOUNCE_MS,
            context_retention_days: None,
            branch_retention_days: None,
            activation_log: true,
            activation_log_retention_days: None,
        }
    }
}

impl Config {
    /// How long a branch may sit untouched before it is deleted.
    pub fn branch_retention(&self) -> Option<std::time::Duration> {
        self.branch_retention_days
            .map(|days| std::time::Duration::from_secs(days.saturating_mul(24 * 60 * 60)))
    }

    /// How long an activation log entry is kept. A number of days too large
    /// for a time span is longer than any entry can be old, so it keeps them
    /// all, as no retention does.
    pub fn activation_log_retention(&self) -> Option<chrono::Duration> {
        let days = i64::try_from(self.activation_log_retention_days?).ok()?;
        chrono::Duration::try_days(days)
    }
}
