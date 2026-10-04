//! Reading the scope activation log back over a whole session: why a scope is
//! on in a subagent, followed to the message in its parent that turned it on.
//!
//! What each cause records is `activation_log_test.rs`, and the JSON the tools
//! answer with is `mcp_test.rs`. What only a session shows is the chain: a
//! scope a subagent never matched itself, inherited at its start, traced back
//! through the parent's entry to the tool result the parent's trigger matched,
//! with every read made through the wrapped MCP calls the model makes.

mod common;
mod scenario;

use scenario::{Field, Store, World, bash};
use serde_json::{Value, json};

/// The machine the scenario runs on.
const ALPHA: &str = "alpha";

/// The home directory the session starts in, which no trigger fires on.
const HOME: &str = "/home/dev";

/// The directory name the `thermostat` scope's trigger recognises.
const FIRMWARE_DIRECTORY: &str = "thermostat-firmware";

/// What `ls ~` answers in the parent session: a home directory with the
/// firmware project among its directories.
const HOME_LISTING: &str = "Desktop\nDocuments\nDownloads\nnotes\nthermostat-firmware\nvideos\n";

/// The subagent's task, which names nothing any trigger recognises.
const TASK: &str = "check what the build needs";

/// Detects a subagent's inherited scope that cannot be followed back to why its
/// parent had it: an entry that names no parent entry, one that names an entry
/// of another scope or context, or a parent entry whose message is not the text
/// the trigger matched. That chain is what the log is for when a subagent works
/// under a scope nobody in it asked for.
///
/// The parent lists its home directory, and the listing names the firmware
/// directory, which is `thermostat`'s trigger on a tool result. The subagent's
/// task names nothing, so its `thermostat` can only have come from the parent.
#[test]
fn a_subagents_inherited_scope_leads_back_to_the_listing_that_turned_it_on() {
    let world = World::new()
        .store(Store::empty().scope("thermostat", |scope| {
            scope.trigger(Field::ToolResult, &format!(r"\b{FIRMWARE_DIRECTORY}\b"));
        }))
        .build();
    let session = world.claude(ALPHA).session();
    session.start_in(HOME);
    session
        .tool("Bash", bash("ls ~"))
        .result(json!({ "stdout": HOME_LISTING }));
    let child = session.subagent("general-purpose", TASK);
    assert!(
        child.active_scopes().contains("thermostat"),
        "the child starts in the scope its parent turned on, got {:?}",
        child.active_scopes()
    );

    let listed = child
        .mcp()
        .call(
            "scope_activations",
            json!({ "session": child.key(), "scope": "thermostat" }),
        )
        .expect_ok();
    let entries = listed.json()["entries"]
        .as_array()
        .expect("the answer lists entries")
        .clone();
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry["cause"].clone(), entry["parent_context"].clone()))
            .collect::<Vec<_>>(),
        vec![(json!("inherited"), json!(session.key()))],
        "the child's thermostat came from its parent, got {entries:?}"
    );
    assert_eq!(
        (entries[0]["agent_type"].clone(), entries[0]["task"].clone()),
        (json!("general-purpose"), json!(TASK)),
        "the child's entry names the kind of subagent it is and its task"
    );
    let parent_entry: Value = entries[0]["parent_entry"].clone();

    let followed = child
        .mcp()
        .call("scope_activation_get", json!({ "id": parent_entry }))
        .expect_ok();
    let parent = followed.json();
    assert_eq!(
        (
            parent["context"].clone(),
            parent["scope"].clone(),
            parent["cause"].clone(),
            parent["field"].clone(),
        ),
        (
            json!(session.key()),
            json!("thermostat"),
            json!("trigger"),
            json!("tool_result"),
        ),
        "the parent entry is the parent's own trigger match on a tool result, got {parent}"
    );
    assert_eq!(
        parent["message"],
        json!(HOME_LISTING),
        "the parent entry's message is the listing the trigger matched"
    );
    assert_eq!(
        parent["evidence"]["matched"],
        json!(FIRMWARE_DIRECTORY),
        "what the trigger found in the listing is the directory name"
    );
}
