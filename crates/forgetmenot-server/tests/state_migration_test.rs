//! Coming back up on a state file an older build wrote.
//!
//! The server starts from the file the build before it left behind, so every
//! shape that file has ever had has to load and turn into the shape this build
//! reads. The fixture is one such file, and
//! [`regenerate_the_version_0_state_fixture`] is what produced it.

mod common;

use std::path::{Path, PathBuf};

use common::{TestServer, example_store_files};
use serde_json::{Value, json};

use forgetmenot_server::context::migrate::CURRENT_VERSION;

/// The machine the fixture was played on, which is what its keys name.
const ALPHA: &str = "alpha";

/// The session the fixture was played on.
const SESSION: &str = "session-1";

/// What the fixture says about itself, so that whoever opens the file knows
/// what it is without reading this one.
const FIXTURE_COMMENT: &str = "A state file in the shape written before the version field \
    existed. Made by the ignored test regenerate_the_version_0_state_fixture in \
    crates/forgetmenot-server/tests/state_migration_test.rs: a server on the example store under \
    examples/store was played a session start, a prompt, a subagent start and a call inside the \
    subagent through POST /hook and was then restarted, which is what made it write its state; \
    the version field and the subagents' parent fields were stripped from what it wrote. Nothing \
    here comes from any real store or any real session.";

/// The file these tests read, as committed.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/state/contexts-version-0.json")
}

/// The fixture as JSON.
fn fixture_json() -> Value {
    let path = fixture_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {} failed: {error}", path.display()));
    serde_json::from_str(&text).expect("the fixture is JSON")
}

/// The records of a state file that name an agent rather than a session.
fn subagent_records(state: &Value) -> Vec<&Value> {
    state["contexts"]
        .as_array()
        .expect("the contexts of a state file are an array")
        .iter()
        .filter(|record| record["agent"] != json!("main"))
        .collect()
}

/// Detects a fixture that is no longer the version-0 file the test below needs:
/// a file that already carries a version, or whose subagents already name a
/// parent, would let that test pass with the migration doing nothing at all.
#[test]
fn the_fixture_carries_no_version_and_no_parent_on_its_subagents() {
    let fixture = fixture_json();

    assert!(
        fixture.get("version").is_none(),
        "a version-0 file was written before the field existed, so it carries none"
    );
    let subagents = subagent_records(&fixture);
    assert!(
        !subagents.is_empty(),
        "the fixture must hold a subagent, which is what the migration repairs"
    );
    for record in subagents {
        assert!(
            record.get("parent").is_none(),
            "the fixture's subagents must name no parent, got {record}"
        );
    }
}

/// Detects a state file from before a subagent recorded which context it
/// inherited from being read as it stands: every subagent restored from it
/// would belong to nothing, so the contexts page would show a session's
/// subagents detached from it and the next `session_inherit` or prompt reading
/// that context would work from a parent that is not there. Detects too a
/// snapshot written back without the version it is now in, which would leave
/// every later start repairing a file that has already been repaired.
///
/// Expectation source: what version 0 meant. It recorded every subagent under
/// the session it ran in, so a record left without a parent has that session
/// for its answer, and the record's own key names it.
#[test]
fn a_server_starting_on_a_version_0_state_file_puts_every_subagent_under_its_session() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let state_path = directory.path().join("contexts.json");
    std::fs::copy(fixture_path(), &state_path).expect("the fixture is copied where a server reads");
    let mut server = TestServer::start(example_store_files(), |config| {
        config.state_path = state_path.clone();
    });

    let (status, contexts) = server.api("GET", "/api/contexts", None);

    assert_eq!(status, 200, "the contexts must be readable, got {contexts}");
    let rows = contexts.as_array().expect("the contexts are an array");
    let subagents: Vec<&Value> = rows
        .iter()
        .filter(|row| {
            let key = row["key"].as_str().expect("a context row has a key");
            key.matches('/').count() == 2
        })
        .collect();
    assert!(
        !subagents.is_empty(),
        "the fixture's subagents must be among the contexts the server came up with, got \
         {contexts}"
    );
    for row in subagents {
        assert_eq!(
            row["parent"],
            json!(format!("{ALPHA}/{SESSION}")),
            "a subagent from version 0 inherited from the session it runs in, got {row}"
        );
    }

    // Shutting down is what writes the state, and the server is still alive
    // afterwards, so the file it wrote is still there to read.
    server.restart();
    let written: Value =
        serde_json::from_slice(&std::fs::read(&state_path).expect("the state file was written"))
            .expect("the written state file is JSON");
    assert_eq!(
        written["version"],
        json!(CURRENT_VERSION),
        "a state file this build writes carries the version it is in"
    );
}

/// What the fixture was made from. Ignored, because it writes the fixture
/// rather than checking anything; run it to make the fixture again:
///
/// ```text
/// cargo test -p forgetmenot-server --test state_migration_test -- \
///     --ignored regenerate_the_version_0_state_fixture
/// ```
///
/// A server on the example store is played a session and a subagent through
/// `POST /hook` and then restarted, which is what makes it write its state
/// file. Stripping the version field and the subagents' parents from what it
/// wrote is what turns a file this build writes into one the build before it
/// wrote.
#[test]
#[ignore = "writes the committed fixture rather than checking anything"]
fn regenerate_the_version_0_state_fixture() {
    let mut server = TestServer::start(example_store_files(), |_| {});
    let transcript = "/home/dev/widgets/.claude/session-1.jsonl";
    let common = json!({
        "session_id": SESSION,
        "transcript_path": transcript,
        "cwd": "/home/dev/widgets",
        "permission_mode": "default",
    });
    let event = |fields: Value| {
        let mut payload = common.clone();
        let object = payload.as_object_mut().expect("a payload is an object");
        for (key, value) in fields.as_object().expect("the fields are an object") {
            object.insert(key.clone(), value.clone());
        }
        payload
    };

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

    server.restart();
    let mut state: Value =
        serde_json::from_slice(&std::fs::read(server.state_path()).expect("the state was written"))
            .expect("the state file is JSON");
    for record in state["contexts"]
        .as_array_mut()
        .expect("the contexts are an array")
    {
        if record["agent"] != json!("main") {
            record
                .as_object_mut()
                .expect("a context record is an object")
                .remove("parent");
        }
    }
    // The version is dropped by leaving it out of what is written, and the
    // comment comes first so that it is the first thing read.
    let fixture = json!({
        "comment": FIXTURE_COMMENT,
        "contexts": state["contexts"].take(),
    });

    let mut text = serde_json::to_string_pretty(&fixture).expect("the fixture renders");
    text.push('\n');
    let path = fixture_path();
    std::fs::create_dir_all(path.parent().expect("the fixture is in a directory"))
        .expect("the fixture directory is creatable");
    std::fs::write(&path, text).expect("the fixture is writable");
}
