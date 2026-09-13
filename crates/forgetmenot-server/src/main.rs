//! The `forgetmenot` command.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use forgetmenot_server::clock::SystemClock;
use forgetmenot_server::config::{Config, DEFAULT_SNAPSHOT_DEBOUNCE_MS};
use forgetmenot_server::stats::{
    DenyDayRow, LatencyRow, MemoryStatsRow, SessionBytesRow, StatsReader, TriggerStatsRow,
};
use forgetmenot_server::store::catalog::Catalog;
use forgetmenot_server::store::git::GitRepo;
use forgetmenot_server::store::validate::validate;
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "forgetmenot",
    version,
    about = "Trigger-based, scoped memory for Claude Code"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server.
    Serve(ServeArgs),
    /// Report every problem in a store, and its counts when there are none.
    Check {
        /// Directory of the store's git repository.
        #[arg(long, value_name = "PATH")]
        store: PathBuf,
    },
    /// Report what the statistics log holds: what was shown for each memory,
    /// what each trigger and scope did, stopped calls per day, hook latency and
    /// delivered bytes per session.
    Stats(StatsArgs),
}

/// Where the server listens and what it reads and writes.
#[derive(Args)]
struct ServeArgs {
    /// Directory of the store's git repository; created if it does not exist.
    #[arg(long, value_name = "PATH")]
    store: PathBuf,
    /// Address to listen on.
    #[arg(long, value_name = "ADDR")]
    listen: Option<SocketAddr>,
    /// File the live contexts are snapshotted to.
    #[arg(long, value_name = "PATH")]
    state_path: Option<PathBuf>,
    /// The sqlite database statistics are appended to.
    #[arg(long, value_name = "PATH")]
    stats_path: Option<PathBuf>,
    /// Directory of the built frontend to serve at `/`.
    #[arg(long, value_name = "PATH")]
    web_dist: Option<PathBuf>,
    /// A Host header value accepted on `/mcp`; may be given more than once.
    #[arg(long = "allowed-host", value_name = "HOST")]
    allowed_hosts: Vec<String>,
    /// How long a context change waits for further changes before the state
    /// file is written.
    #[arg(long, value_name = "MS", default_value_t = DEFAULT_SNAPSHOT_DEBOUNCE_MS)]
    snapshot_debounce_ms: u64,
    /// Drop contexts not seen for this many days; they are kept forever without
    /// it.
    #[arg(long, value_name = "DAYS")]
    context_retention_days: Option<u64>,
    /// Delete transaction branches not written to for this many days; they are
    /// kept forever without it.
    #[arg(long, value_name = "DAYS")]
    branch_retention_days: Option<u64>,
}

impl ServeArgs {
    fn into_config(self) -> Config {
        let mut config = Config::new(&self.store);
        if let Some(listen) = self.listen {
            config.listen = listen;
        }
        if let Some(state_path) = self.state_path {
            config.state_path = state_path;
        }
        if let Some(stats_path) = self.stats_path {
            config.stats_path = stats_path;
        }
        config.web_dist = self.web_dist;
        config.allowed_hosts = self.allowed_hosts;
        config.snapshot_debounce_ms = self.snapshot_debounce_ms;
        config.context_retention_days = self.context_retention_days;
        config.branch_retention_days = self.branch_retention_days;
        config
    }
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Check { store } => check(&store),
        Command::Serve(arguments) => serve(arguments.into_config()),
        Command::Stats(arguments) => stats(&arguments),
    }
}

/// Run the server until the process is asked to stop.
fn serve(config: Config) -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("the async runtime could not be started: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        let server = match forgetmenot_server::app::start(config, Arc::new(SystemClock)).await {
            Ok(server) => server,
            Err(error) => {
                eprintln!("the server could not start: {error}");
                return ExitCode::FAILURE;
            }
        };
        tracing::info!("listening on {}", server.address);
        match server.run_until_signalled().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("the server stopped with an error: {error}");
                ExitCode::FAILURE
            }
        }
    })
}

/// Validate a store: one line per problem, else the counts.
///
/// Exit 1 on any problem, so that a commit hook or a deployment step can gate
/// on the store being valid.
fn check(store_path: &Path) -> ExitCode {
    if !store_path.exists() {
        eprintln!("{}: no such directory", store_path.display());
        return ExitCode::FAILURE;
    }
    // Opened read-only: `check` reports on a store, it does not create one.
    let repository = match GitRepo::open(store_path) {
        Ok(repository) => repository,
        Err(error) => {
            eprintln!("{}: {error}", store_path.display());
            return ExitCode::FAILURE;
        }
    };
    let catalog = match Catalog::load(&repository) {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("{}: {error}", store_path.display());
            return ExitCode::FAILURE;
        }
    };

    let report = validate(&catalog);
    // Warnings are printed whatever the outcome: a skipped file or a memory with
    // no index entry is worth seeing, but neither makes the store invalid.
    for warning in report.warnings() {
        println!("{}: warning: {warning}", warning.path());
    }
    if report.has_errors() {
        for error in report.errors() {
            println!("{}: {error}", error.path());
        }
        return ExitCode::FAILURE;
    }

    println!("scopes: {}", catalog.scopes().len());
    println!("memories: {}", catalog.memories().len());
    println!("triggers: {}", catalog.triggers().len());
    ExitCode::SUCCESS
}

/// What to report and in which form.
#[derive(Args)]
struct StatsArgs {
    /// The sqlite database the server appended statistics to.
    #[arg(long, value_name = "PATH")]
    stats_path: PathBuf,
    /// Print one JSON object holding every aggregate instead of tables.
    #[arg(long)]
    json: bool,
}

/// Every aggregate, as one JSON object.
///
/// There is no live-context count here: which contexts are working in a scope
/// right now is state a running server holds, and this command reads a file, so
/// the scope report is the activations the log recorded and nothing else.
#[derive(Serialize)]
struct StatsReport {
    memories: Vec<MemoryStatsRow>,
    triggers: Vec<TriggerStatsRow>,
    scopes: Vec<ScopeActivationsRow>,
    denies: Vec<DenyDayRow>,
    latency: Vec<LatencyRow>,
    sessions: Vec<SessionBytesRow>,
}

/// What one scope did, as a file can answer it.
#[derive(Serialize)]
struct ScopeActivationsRow {
    scope_id: String,
    activations: u64,
}

/// Report the statistics at `--stats-path`.
fn stats(arguments: &StatsArgs) -> ExitCode {
    let reader = match StatsReader::open(&arguments.stats_path) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    // No active scope sets: see StatsReport.
    let report = (|| {
        Ok::<_, forgetmenot_server::stats::StatsError>(StatsReport {
            memories: reader.memory_stats()?,
            triggers: reader.trigger_stats()?,
            scopes: reader
                .scope_stats(&[])?
                .into_iter()
                .map(|row| ScopeActivationsRow {
                    scope_id: row.scope_id,
                    activations: row.activations,
                })
                .collect(),
            denies: reader.deny_days()?,
            latency: reader.latency()?,
            sessions: reader.session_bytes()?,
        })
    })();
    let report = match report {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    if arguments.json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("the report could not be written as JSON: {error}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }
    print_report(&report);
    ExitCode::SUCCESS
}

/// The report as plain tables, one per aggregate.
fn print_report(report: &StatsReport) {
    print_table(
        "memories",
        &[
            "memory",
            "shown_index",
            "shown_full_new",
            "shown_full_changed",
            "shown_full_stale",
            "fetched_full",
            "retracted",
            "last_shown",
        ],
        report
            .memories
            .iter()
            .map(|row| {
                vec![
                    row.memory.clone(),
                    row.shown_index.to_string(),
                    row.shown_full_new.to_string(),
                    row.shown_full_changed.to_string(),
                    row.shown_full_stale.to_string(),
                    row.fetched_full.to_string(),
                    row.retracted.to_string(),
                    row.last_shown.clone().unwrap_or_default(),
                ]
            })
            .collect(),
    );
    print_table(
        "triggers",
        &[
            "scope_id",
            "field",
            "pattern",
            "fires",
            "new_activations",
            "deny_share",
        ],
        report
            .triggers
            .iter()
            .map(|row| {
                vec![
                    row.scope_id.clone(),
                    row.field.clone(),
                    row.pattern.clone(),
                    row.fires.to_string(),
                    row.new_activations.to_string(),
                    format!("{:.2}", row.deny_share),
                ]
            })
            .collect(),
    );
    print_table(
        "scopes",
        &["scope_id", "activations"],
        report
            .scopes
            .iter()
            .map(|row| vec![row.scope_id.clone(), row.activations.to_string()])
            .collect(),
    );
    print_table(
        "denies",
        &["day", "denies", "events"],
        report
            .denies
            .iter()
            .map(|row| {
                vec![
                    row.day.clone(),
                    row.denies.to_string(),
                    row.events.to_string(),
                ]
            })
            .collect(),
    );
    print_table(
        "latency",
        &["event", "count", "p50_us", "p90_us", "p99_us", "max_us"],
        report
            .latency
            .iter()
            .map(|row| {
                vec![
                    row.event.clone(),
                    row.count.to_string(),
                    row.p50_us.to_string(),
                    row.p90_us.to_string(),
                    row.p99_us.to_string(),
                    row.max_us.to_string(),
                ]
            })
            .collect(),
    );
    print_table(
        "sessions",
        &["session_key", "bytes_full", "bytes_index"],
        report
            .sessions
            .iter()
            .map(|row| {
                vec![
                    row.session_key.clone(),
                    row.bytes_full.to_string(),
                    row.bytes_index.to_string(),
                ]
            })
            .collect(),
    );
}

/// One table: the name of the aggregate, the column names, then the rows, each
/// column as wide as its widest cell.
fn print_table(name: &str, headers: &[&str], rows: Vec<Vec<String>>) {
    println!("{name}");
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in &rows {
        for (column, cell) in row.iter().enumerate() {
            let width = cell.chars().count();
            if width > widths[column] {
                widths[column] = width;
            }
        }
    }
    println!("  {}", pad(headers.iter().copied(), &widths));
    for row in &rows {
        println!("  {}", pad(row.iter().map(String::as_str), &widths));
    }
    println!();
}

/// One line of a table: each cell padded to its column's width.
fn pad<'a>(cells: impl Iterator<Item = &'a str>, widths: &[usize]) -> String {
    cells
        .enumerate()
        .map(|(column, cell)| format!("{cell:<width$}", width = widths[column]))
        .collect::<Vec<_>>()
        .join("  ")
        .trim_end()
        .to_string()
}
