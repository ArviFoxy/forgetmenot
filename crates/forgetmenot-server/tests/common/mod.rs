//! Shared test setup: a store that is a real git repository in a temporary
//! directory, built through the same `commit_files` the server uses.
//!
//! Building fixtures through the public write path rather than by copying files
//! into place means these helpers exercise the interface under test: a bug that
//! made `commit_files` write nothing would fail every test that reads a fixture
//! back, not silently pass.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use forgetmenot_server::store::catalog::Catalog;
use forgetmenot_server::store::git::GitRepo;
use tempfile::TempDir;

/// A store in a temporary directory, removed when this value is dropped.
pub struct TempStore {
    directory: TempDir,
    repository: GitRepo,
}

impl TempStore {
    /// The store's directory, which is what `--store` takes.
    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn repository(&self) -> &GitRepo {
        &self.repository
    }

    /// The catalog of the store's current head.
    pub fn catalog(&self) -> Catalog {
        Catalog::load(&self.repository).expect("the store's catalog loads")
    }

    /// Commit one more set of changes, failing the test on error.
    pub fn commit(&self, title: &str, files: Vec<(String, Option<Vec<u8>>)>) {
        let head = self.repository.head_oid().expect("the store has a head");
        self.repository
            .commit_files("test", title, "", files, Some(head))
            .unwrap_or_else(|error| panic!("committing {title:?} failed: {error}"));
    }
}

/// A store holding exactly `files`, committed as one commit.
pub fn store_with(files: &[(&str, &[u8])]) -> TempStore {
    let owned = files
        .iter()
        .map(|(path, bytes)| ((*path).to_string(), Some(bytes.to_vec())))
        .collect();
    store_from(owned)
}

/// A store holding exactly the given files, committed as one commit.
pub fn store_from(files: Vec<(String, Option<Vec<u8>>)>) -> TempStore {
    let directory = TempDir::new().expect("a temporary directory");
    let repository = GitRepo::open_or_init(directory.path()).expect("a new store initialises");
    if !files.is_empty() {
        let head = repository.head_oid().expect("the new store has a head");
        repository
            .commit_files("test", "build the fixture store", "", files, Some(head))
            .expect("the fixture store commits");
    }
    TempStore {
        directory,
        repository,
    }
}

/// A temporary store holding a copy of `examples/store`.
pub fn example_store() -> TempStore {
    store_from(read_directory(&example_store_path()))
}

/// The path of the example store committed in this repository.
pub fn example_store_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/store")
        .canonicalize()
        .expect("the example store directory exists")
}

/// Every file under `root`, as repository paths with `/` separators.
pub fn read_directory(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn collect(root: &Path, directory: &Path, files: &mut Vec<(String, Option<Vec<u8>>)>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("reading {} failed: {error}", directory.display()));
    for entry in entries {
        let entry = entry.expect("a readable directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("the entry is under the root")
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("reading {} failed: {error}", path.display()));
        files.push((relative, Some(bytes)));
    }
}

/// The path of the compiled `forgetmenot` binary under test.
pub fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_forgetmenot")
}

// ---------------------------------------------------------------------------
// A real server on a real store, which is what the hook and API tests drive.
// ---------------------------------------------------------------------------

use std::net::SocketAddr;
use std::sync::Arc;

use forgetmenot_server::app::{self, RunningServer};
use forgetmenot_server::clock::FixedClock;
use forgetmenot_server::config::Config;
use forgetmenot_server::stats::StatsReader;
use serde_json::{Value, json};

/// A server bound to an ephemeral loopback port, with its own temporary store,
/// state file and statistics database.
///
/// The clock is fixed, so nothing a test asserts depends on when it ran.
pub struct TestServer {
    runtime: tokio::runtime::Runtime,
    server: Option<RunningServer>,
    address: SocketAddr,
    store: TempStore,
    /// Holds the state file and the statistics database; removed on drop.
    paths: TempDir,
    config: Config,
}

impl TestServer {
    /// Start a server on a store holding `files`, with `overrides` applied to
    /// the configuration after the test defaults.
    pub fn start(
        files: Vec<(String, Option<Vec<u8>>)>,
        overrides: impl FnOnce(&mut Config),
    ) -> Self {
        let store = store_from(files);
        let paths = TempDir::new().expect("a temporary directory");
        let mut config = Config::new(store.path());
        config.listen = "127.0.0.1:0".parse().expect("a valid loopback address");
        config.state_path = paths.path().join("contexts.json");
        config.stats_path = paths.path().join("stats.sqlite");
        // Every change is written straight away, so a restart in a test sees
        // exactly what the last event left behind.
        config.snapshot_debounce_ms = 0;
        overrides(&mut config);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("a tokio runtime");
        let server = runtime
            .block_on(app::start(config.clone(), Arc::new(FixedClock::default())))
            .expect("the test server starts");
        let address = server.address;
        Self {
            runtime,
            server: Some(server),
            address,
            store,
            paths,
            config,
        }
    }

    /// The base URL, without a trailing slash.
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The store the server reads, for commits made behind its back.
    pub fn store(&self) -> &TempStore {
        &self.store
    }

    /// Commit to the store the way a person with a shell would.
    pub fn commit(&self, title: &str, files: Vec<(String, Option<Vec<u8>>)>) {
        self.store.commit(title, files);
    }

    /// Send one hook event and read the answer.
    pub fn hook(&self, machine: &str, context_tokens: Option<u64>, event: &Value) -> (u16, Value) {
        post_hook(&self.url(), machine, context_tokens, event)
    }

    /// Send one API request and read the status and the answer.
    ///
    /// `body` is sent as JSON for the methods that take one; the answer is
    /// parsed as JSON, which every API answer is.
    pub fn api(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        api_request(&self.url(), method, path, body)
    }

    /// The state the running server's handlers share, so that a test can call
    /// the operations layer at the same interface the MCP tools will use.
    pub fn state(&self) -> Arc<forgetmenot_server::app::AppState> {
        self.server
            .as_ref()
            .expect("the server is running")
            .state
            .clone()
    }

    /// Run one async operation on the server's runtime, for the operations that
    /// have no HTTP route of their own.
    pub fn run<Work: std::future::Future>(&self, work: Work) -> Work::Output {
        self.runtime.block_on(work)
    }

    /// Send one request body verbatim, for bodies that are not valid requests.
    pub fn post_raw(&self, path: &str, body: &str) -> (u16, String) {
        let agent = test_agent();
        let mut response = agent
            .post(format!("{}{path}", self.url()))
            .header("content-type", "application/json")
            .send(body)
            .expect("the request reaches the server");
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .expect("the answer is text");
        (status, text)
    }

    /// The statistics database the server writes, which is what `forgetmenot
    /// stats --stats-path` takes.
    pub fn stats_path(&self) -> &Path {
        &self.config.stats_path
    }

    /// Read the statistics the server has written. Flushes first, so every
    /// record queued by an answered request is in the database.
    pub fn stats(&self) -> StatsReader {
        let server = self.server.as_ref().expect("the server is running");
        self.runtime.block_on(server.state.stats.flush());
        StatsReader::open(&self.config.stats_path).expect("the statistics database opens")
    }

    /// Stop the server and start it again on the same store, state file and
    /// statistics database.
    pub fn restart(&mut self) {
        let server = self.server.take().expect("the server is running");
        self.runtime
            .block_on(server.shutdown())
            .expect("the server shuts down");
        let server = self
            .runtime
            .block_on(app::start(
                self.config.clone(),
                Arc::new(FixedClock::default()),
            ))
            .expect("the server starts again");
        self.address = server.address;
        self.server = Some(server);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = self.runtime.block_on(server.shutdown());
        }
        // Named so that the store and the temporary paths are seen to outlive
        // the server that was reading them.
        let _ = (&self.store, &self.paths);
    }
}

/// An HTTP client that reports a status instead of turning it into an error, so
/// that a test can assert on a 400.
pub fn test_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

/// Send one API request to a running server and read its status and answer.
///
/// A free function as well as a method, because the concurrency tests drive one
/// server from several threads at once and a thread only needs the base URL.
pub fn api_request(base_url: &str, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let agent = test_agent();
    let url = format!("{base_url}{path}");
    let mut response = match (method, body) {
        ("GET", _) => agent.get(&url).call(),
        ("PUT", Some(body)) => agent
            .put(&url)
            .header("content-type", "application/json")
            .send(serde_json::to_string(body).expect("the body serialises")),
        ("POST", Some(body)) => agent
            .post(&url)
            .header("content-type", "application/json")
            .send(serde_json::to_string(body).expect("the body serialises")),
        ("POST", None) => agent.post(&url).send_empty(),
        // A delete carries a body: it says which version it removes, so it is
        // refused when the store has moved on, like every other write. The
        // client's builder has no body for a delete, so the request is built as
        // an `http::Request` and run as it is.
        ("DELETE", Some(body)) => agent.run(
            ureq::http::Request::builder()
                .method("DELETE")
                .uri(&url)
                .header("content-type", "application/json")
                .body(serde_json::to_string(body).expect("the body serialises"))
                .expect("the delete request builds"),
        ),
        ("DELETE", None) => agent.delete(&url).call(),
        (other, _) => panic!("{other} is not a method these tests send"),
    }
    .unwrap_or_else(|error| panic!("{method} {url} did not reach the server: {error}"));

    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .expect("the answer is text");
    let answer = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, answer)
}

/// POST one hook event to a running server, as the hook client does.
pub fn post_hook(
    base_url: &str,
    machine: &str,
    context_tokens: Option<u64>,
    event: &Value,
) -> (u16, Value) {
    let body = json!({
        "machine": machine,
        "context_tokens": context_tokens,
        "hook": event,
    });
    let agent = test_agent();
    let mut response = agent
        .post(format!("{base_url}/hook"))
        .header("content-type", "application/json")
        // serialised here rather than with a json feature: the client crate
        // pins ureq without default features, and the tests use the same build.
        .send(serde_json::to_string(&body).expect("the body serialises"))
        .expect("the request reaches the server");
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .expect("the answer is text");
    let answer = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, answer)
}

/// The example store as a list of files, read at test time.
pub fn example_store_files() -> Vec<(String, Option<Vec<u8>>)> {
    read_directory(&example_store_path())
}

/// The path of the store's behaviour settings, which the example store carries
/// like any other file.
pub const SETTINGS_PATH: &str = "config.yml";

/// The example store with its `config.yml` replaced by `text`, for the tests
/// that run under settings other than the example's own.
pub fn example_store_with_settings(text: &str) -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = example_store_without_settings();
    files.push((SETTINGS_PATH.to_string(), Some(text.as_bytes().to_vec())));
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

/// The example store with no settings file at all, which is a store nobody has
/// ever configured.
pub fn example_store_without_settings() -> Vec<(String, Option<Vec<u8>>)> {
    let mut files = example_store_files();
    files.retain(|(path, _)| path != SETTINGS_PATH);
    files
}

/// One of the recorded hook payloads, with the transcript placeholder replaced.
///
/// The server never reads the transcript, so the path only has to be a path.
pub fn hook_fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hooks")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {} failed: {error}", path.display()));
    let text = text.replace("TRANSCRIPT_PATH", "/nonexistent/transcript.jsonl");
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not valid JSON: {error}", path.display()))
}

/// The text the answer asks Claude Code to inject, if any.
pub fn additional_context(answer: &Value) -> Option<&str> {
    answer
        .get("hookSpecificOutput")?
        .get("additionalContext")?
        .as_str()
}

/// The permission decision the answer carries, if any.
pub fn permission_decision(answer: &Value) -> Option<&str> {
    answer
        .get("hookSpecificOutput")?
        .get("permissionDecision")?
        .as_str()
}

/// The reason the answer gives for its decision, if any.
pub fn permission_decision_reason(answer: &Value) -> Option<&str> {
    answer
        .get("hookSpecificOutput")?
        .get("permissionDecisionReason")?
        .as_str()
}

// ---------------------------------------------------------------------------
// The compiled hook client, for the whole-chain tests.
//
// Claude Code runs the client as a process, so the whole-chain tests run it as
// one too: the things they exist to catch (junk on stdout, a wire mismatch, a
// context size that never leaves the client) are only visible outside it.
// ---------------------------------------------------------------------------

use std::io::Write;
use std::process::{Command, Stdio};

/// The directory of the test executable's profile, which is where cargo puts
/// every binary of the workspace.
///
/// `env!("CARGO_BIN_EXE_...")` only names binaries of the crate the test
/// belongs to, and the client is another crate, so the path is derived from the
/// running test executable instead: `target/<profile>/deps/<test>` has the
/// binaries in its parent's parent.
fn target_profile_directory() -> PathBuf {
    let test_executable = std::env::current_exe().expect("the test executable has a path");
    test_executable
        .parent()
        .and_then(Path::parent)
        .expect("the test executable is inside target/<profile>/deps")
        .to_path_buf()
}

/// The compiled `forgetmenot-hook` binary the whole-chain tests run.
///
/// Panics with what to do about it when the binary is not there: cargo does not
/// build another crate's binary for this crate's tests, so the workspace has to
/// have been built.
pub fn hook_client_binary() -> PathBuf {
    let binary = target_profile_directory()
        .join(format!("forgetmenot-hook{}", std::env::consts::EXE_SUFFIX));
    assert!(
        binary.is_file(),
        "the hook client binary is not at {}; run `cargo build -p forgetmenot-hook` \
         (or `cargo build --workspace`) before this test",
        binary.display()
    );
    binary
}

/// What one run of the client left behind.
pub struct ClientRun {
    /// `None` when the process was killed by a signal rather than exiting.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ClientRun {
    /// The bytes the client wrote to stdout, which is all Claude Code reads.
    pub fn stdout_text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    pub fn stderr_text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }

    /// The single JSON object the client wrote, and nothing else.
    ///
    /// Claude Code parses stdout as one JSON document, so two objects, an
    /// object with anything after it, or a log line in front of it are all
    /// failures of the same contract.
    pub fn single_json_object(&self) -> Result<Value, String> {
        let mut documents = serde_json::Deserializer::from_slice(&self.stdout).into_iter::<Value>();
        let first = match documents.next() {
            Some(Ok(value)) => value,
            Some(Err(error)) => return Err(format!("stdout is not JSON: {error}")),
            None => return Err("stdout is empty".to_string()),
        };
        if let Some(extra) = documents.next() {
            return Err(format!("stdout carries more than one document: {extra:?}"));
        }
        if !first.is_object() {
            return Err(format!("stdout is not a JSON object but {first}"));
        }
        Ok(first)
    }
}

/// Run the compiled client once with `payload` on stdin, as Claude Code does.
pub fn run_hook_client(server_url: &str, machine: &str, payload: &[u8]) -> ClientRun {
    run_hook_client_binary(&hook_client_binary(), server_url, machine, payload)
}

/// The same, with the client binary named: the limits test times another build
/// of the client when one is given.
pub fn run_hook_client_binary(
    binary: &Path,
    server_url: &str,
    machine: &str,
    payload: &[u8],
) -> ClientRun {
    let mut child = Command::new(binary)
        .args(["--server", server_url, "--machine", machine])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the client binary starts");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(payload)
        .expect("the payload is writable to the client");
    let output = child.wait_with_output().expect("the client finishes");
    ClientRun {
        code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

/// The names of every recorded hook payload, so that a payload added to the
/// fixture directory is covered without a change to any test.
pub fn hook_fixture_names() -> Vec<String> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hooks");
    let mut names: Vec<String> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("reading {} failed: {error}", directory.display()))
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| {
            path.file_stem()
                .expect("a file with an extension has a stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert!(
        !names.is_empty(),
        "no hook payloads in {}",
        directory.display()
    );
    names
}

/// One recorded hook payload as the bytes Claude Code writes to a hook's
/// stdin: the file's text with the transcript placeholder replaced, ending in
/// the newline Claude Code sends.
pub fn hook_fixture_payload(name: &str, transcript: &Path) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hooks")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {} failed: {error}", path.display()));
    let text = text.replace(
        "TRANSCRIPT_PATH",
        transcript.to_str().expect("a utf-8 transcript path"),
    );
    format!("{}\n", text.trim_end()).into_bytes()
}

/// Write a transcript whose last assistant message reports `context_tokens`,
/// padded to at least `minimum_bytes`.
///
/// The sum is known by construction rather than read back: the last assistant
/// line is the only one carrying `context_tokens`, split over the three
/// counters that make up the context size, and the lines after it are not
/// assistant lines, so a reader that takes the first assistant line, the last
/// line, or one counter of three gets a different answer.
pub fn write_transcript(path: &Path, context_tokens: u64, minimum_bytes: u64) {
    assert!(
        context_tokens >= 2,
        "the last line splits the sum over three counters, so it needs at least 2 tokens"
    );
    let padding = "x".repeat(400);
    let mut file = std::io::BufWriter::new(
        std::fs::File::create(path)
            .unwrap_or_else(|error| panic!("creating {} failed: {error}", path.display())),
    );
    let mut written = 0u64;
    let mut turn = 0u64;
    while written < minimum_bytes {
        let line = format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"turn {turn} {padding}\"}}}}\n\
             {{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input_tokens\":{},\"cache_read_input_tokens\":1,\"cache_creation_input_tokens\":1,\"output_tokens\":7}},\"content\":\"reply {turn} {padding}\"}}}}\n",
            turn + 1
        );
        file.write_all(line.as_bytes())
            .expect("the transcript writes");
        written += line.len() as u64;
        turn += 1;
    }
    let last = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input_tokens\":{},\"cache_read_input_tokens\":1,\"cache_creation_input_tokens\":1,\"output_tokens\":7}},\"content\":\"the last reply\"}}}}\n\
         {{\"type\":\"system\",\"subtype\":\"post_tool\",\"text\":\"after the last assistant line\"}}\n",
        context_tokens - 2
    );
    file.write_all(last.as_bytes())
        .expect("the transcript writes");
    file.flush().expect("the transcript flushes");
}

// ---------------------------------------------------------------------------
// A real MCP client against the running server's `/mcp`, which is what the MCP
// tests drive. Nothing here knows the tools: a test names the tool and sends
// JSON, so a tool renamed or a parameter renamed fails the test.
// ---------------------------------------------------------------------------

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientInfo, Tool};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

impl TestServer {
    /// Connect an MCP client to `/mcp` and complete the handshake.
    pub fn mcp(&self) -> McpSession<'_> {
        let url = format!("{}/mcp", self.url());
        // The transport is built inside the runtime: it starts a task of its own,
        // which needs a runtime to start in.
        let client = self
            .run(async move {
                let transport = StreamableHttpClientTransport::from_config(
                    StreamableHttpClientTransportConfig::with_uri(url),
                );
                ClientInfo::default().serve(transport).await
            })
            .expect("the MCP client initialises against /mcp");
        McpSession {
            server: self,
            client: Some(client),
        }
    }
}

/// One initialised MCP session, closed when this value is dropped.
///
/// The session is closed before the server is asked to stop, because a server
/// shutting down waits for the connections it still holds.
pub struct McpSession<'server> {
    server: &'server TestServer,
    client: Option<RunningService<RoleClient, ClientInfo>>,
}

impl McpSession<'_> {
    fn client(&self) -> &RunningService<RoleClient, ClientInfo> {
        self.client.as_ref().expect("the MCP session is open")
    }

    /// Every tool the server offers.
    pub fn tools(&self) -> Vec<Tool> {
        self.server
            .run(self.client().list_all_tools())
            .expect("the server answers tools/list")
    }

    /// The instructions the server sent at initialisation, if any.
    pub fn instructions(&self) -> Option<String> {
        self.client()
            .peer_info()
            .and_then(|info| info.instructions.clone())
    }

    /// Call one tool with `arguments` as its parameters, expecting the call to be
    /// answered: an operation that failed is an answer with `is_error` set.
    pub fn call(&self, tool: &str, arguments: Value) -> CallToolResult {
        self.try_call(tool, arguments)
            .unwrap_or_else(|error| panic!("calling {tool} failed at the protocol level: {error}"))
    }

    /// Call one tool and report the protocol error as well, for the calls that
    /// are refused before any operation runs.
    pub fn try_call(&self, tool: &str, arguments: Value) -> Result<CallToolResult, ServiceError> {
        let arguments = match arguments {
            Value::Null => None,
            Value::Object(map) => Some(map),
            other => panic!("tool arguments are a JSON object, not {other}"),
        };
        let mut request = CallToolRequestParams::new(tool.to_string());
        if let Some(arguments) = arguments {
            request = request.with_arguments(arguments);
        }
        self.server.run(self.client().call_tool(request))
    }
}

impl Drop for McpSession<'_> {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            let _ = self.server.run(client.cancel());
        }
    }
}

/// The text an MCP tool answered with: its text blocks, joined by newlines.
pub fn tool_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text())
        .map(|text| text.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool's answer parsed as the JSON it reports, which every tool here answers
/// with.
pub fn tool_json(result: &CallToolResult) -> Value {
    let text = tool_text(result);
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("the answer is not JSON: {error}; got {text:?}"))
}
