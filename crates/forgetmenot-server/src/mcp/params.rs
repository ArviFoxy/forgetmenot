//! The parameters the MCP tools take.
//!
//! Each struct derives `Deserialize` and `JsonSchema`, so the doc comment on a
//! field is the description the model reads in the tool's input schema. The
//! store's own types are not used here: a parameter is text the model wrote, and
//! the schema has to say which words are accepted before any of it reaches the
//! operations layer.

use rmcp::ErrorData;
// Imported under its own name as well: the derive writes `schemars::` paths, and
// this crate does not depend on schemars other than through rmcp.
use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::context::ContextKey;
use crate::store::memory::{MemoryKind, MemorySource};

/// Which kind a memory is delivered as.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestedKind {
    /// Delivered in full whenever it becomes due.
    Critical,
    /// Delivered as one index line the model can fetch the body by.
    Knowledge,
}

impl From<RequestedKind> for MemoryKind {
    fn from(kind: RequestedKind) -> Self {
        match kind {
            RequestedKind::Critical => MemoryKind::Critical,
            RequestedKind::Knowledge => MemoryKind::Knowledge,
        }
    }
}

/// Who a memory came from.
///
/// Free text rather than a choice of two: the conventional values are `user`,
/// the person who did not ask for it to be rewritten, and `assistant`, the
/// model itself, and any other value a person or another tool keeps in the file
/// is stored and read back as it was written.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct RequestedSource(String);

impl From<RequestedSource> for MemorySource {
    fn from(source: RequestedSource) -> Self {
        MemorySource::new(source.0)
    }
}

/// The `metadata` keys forgetmenot does not interpret, as a write sends them.
pub type RequestedMetadata = serde_json::Map<String, serde_json::Value>;

/// Which memories to list.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryIndexParams {
    /// Only memories carrying at least one of these scope ids; every memory when
    /// this is absent.
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Only memories of this kind; both kinds when this is absent.
    #[serde(default)]
    pub kind: Option<RequestedKind>,
}

/// Which memory to read, and for whom.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryGetParams {
    /// The memory id, as the index reports it: `widget-naming`, or
    /// `sessions/<machine>/<session-id>/<name>` for a session's own note.
    pub id: String,
    /// The calling session's key. Given it, reading the body counts as the
    /// memory having been delivered to this session, so it is not repeated at
    /// the next hook event until it changes.
    #[serde(default)]
    pub session_key: Option<String>,
}

/// One memory to write.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryPutParams {
    /// The calling session's key, recorded as the commit's author.
    pub session_key: String,
    /// The memory id, which is the file the memory lives in. A new id creates a
    /// memory; an id that exists replaces the version named by `base_version`.
    pub id: String,
    /// The one-line description that is this memory's index entry.
    pub description: String,
    pub kind: RequestedKind,
    /// The scope ids this memory is delivered in. A memory under
    /// `sessions/<machine>/<session-id>/` takes exactly its own session scope.
    pub scopes: Vec<String>,
    /// Who the memory came from: `user` or `assistant` by convention, and any
    /// other value is kept as it was written.
    pub source: RequestedSource,
    /// The other `metadata` keys of the memory's file, Claude Code's own `type`
    /// and any other key a person or a tool keeps in the frontmatter, which
    /// replace the ones the memory carries. Leave it out to keep them.
    #[serde(default)]
    pub metadata: Option<RequestedMetadata>,
    /// The memory itself, as markdown. `[[name]]` links to another memory.
    pub body: String,
    /// The commit's title line: one line, at most 72 characters, saying what
    /// changed and why.
    pub message: String,
    /// The version this write replaces, as `memory_get` reported it. Absent
    /// creates a memory at an id that is free.
    #[serde(default)]
    pub base_version: Option<String>,
    /// A branch from `branch_create`. With it the write is committed to that
    /// branch instead of to main, so no session is delivered anything until the
    /// branch is landed.
    #[serde(default)]
    pub branch: Option<String>,
}

/// One memory to delete.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryDeleteParams {
    /// The calling session's key, recorded as the commit's author.
    pub session_key: String,
    /// The memory id to delete.
    pub id: String,
    /// The commit's title line: one line, at most 72 characters, saying why the
    /// memory is no longer in force.
    pub message: String,
    /// The version this write removes, as `memory_get` reported it. Absent
    /// deletes whatever version the store holds now.
    #[serde(default)]
    pub base_version: Option<String>,
    /// A branch from `branch_create`. With it the deletion is committed to that
    /// branch instead of to main, so the memory keeps being delivered until the
    /// branch is landed.
    #[serde(default)]
    pub branch: Option<String>,
}

/// One snippet of one memory's body to replace.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryReplaceTextParams {
    /// The calling session's key, recorded as the commit's author.
    pub session_key: String,
    /// The memory id whose body is edited.
    pub id: String,
    /// The exact text to replace, copied from the body as `memory_get` reported
    /// it. It is matched literally, not as a pattern.
    pub old_string: String,
    /// What to put in its place.
    pub new_string: String,
    /// Replace every occurrence. Without it, a snippet that appears more than
    /// once is refused rather than one of them being picked.
    #[serde(default)]
    pub replace_all: bool,
    /// The commit's title line: one line, at most 72 characters.
    pub message: String,
    /// The version this write replaces. Absent edits whatever version the store
    /// holds now.
    #[serde(default)]
    pub base_version: Option<String>,
    /// A branch from `branch_create`. With it the write is committed to that
    /// branch instead of to main.
    #[serde(default)]
    pub branch: Option<String>,
}

/// The fields of one memory to set, leaving its body alone.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemorySetFieldsParams {
    /// The calling session's key, recorded as the commit's author.
    pub session_key: String,
    /// The memory id whose fields are set.
    pub id: String,
    /// The one-line description that is this memory's index entry.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub kind: Option<RequestedKind>,
    /// The scope ids this memory is delivered in, replacing the ones it has.
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Who the memory came from: `user` or `assistant` by convention, and any
    /// other value is kept as it was written.
    #[serde(default)]
    pub source: Option<RequestedSource>,
    /// The other `metadata` keys of the memory's file, Claude Code's own `type`
    /// and any other key a person or a tool keeps in the frontmatter. A key
    /// given is added or replaced, a key given as null is removed, and a key
    /// not given keeps its value.
    #[serde(default)]
    pub metadata: Option<RequestedMetadata>,
    /// The commit's title line: one line, at most 72 characters.
    pub message: String,
    /// The version this write replaces. Absent edits whatever version the store
    /// holds now.
    #[serde(default)]
    pub base_version: Option<String>,
    /// A branch from `branch_create`. With it the write is committed to that
    /// branch instead of to main.
    #[serde(default)]
    pub branch: Option<String>,
}

/// One memory to move to another id.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryRenameParams {
    /// The calling session's key, recorded as the commit's author.
    pub session_key: String,
    /// The memory id as it is now.
    pub from: String,
    /// The memory id it takes. It has to be free.
    pub to: String,
    /// The commit's title line: one line, at most 72 characters.
    pub message: String,
    /// The version this write moves. Absent moves whatever version the store
    /// holds now.
    #[serde(default)]
    pub base_version: Option<String>,
    /// A branch from `branch_create`. With it the move is committed to that
    /// branch instead of to main.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Which session is opening a branch.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BranchCreateParams {
    /// The calling session's key, recorded as the branch's owner.
    pub session_key: String,
}

/// Which branch a call is about.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BranchParams {
    /// The branch name `branch_create` answered with, as `tx/<name>`.
    pub branch: String,
}

/// Which branch to land, and as what commit.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BranchLandParams {
    /// The calling session's key, recorded as the commit's author.
    pub session_key: String,
    /// The branch name `branch_create` answered with, as `tx/<name>`.
    pub branch: String,
    /// The title line of the one commit every write on the branch becomes: one
    /// line, at most 72 characters, saying what the whole change did.
    pub message: String,
}

/// Which setting to change, and to what.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SettingsSetParams {
    /// The calling session's key, recorded as the commit's author.
    pub session_key: String,
    /// The setting to change, one of the keys `settings_get` reports.
    pub key: String,
    /// The new value, of the type `settings_get` gives for this key: a number,
    /// a bool, null, or a list of tool names.
    pub value: serde_json::Value,
    /// The commit's title line: one line, at most 72 characters, saying what the
    /// new behaviour is for.
    pub message: String,
    /// A branch from `branch_create`. With it the change is committed to that
    /// branch instead of to main, so no session behaves differently until the
    /// branch is landed.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Which session is asking.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionParams {
    /// The calling session's key, printed in this session's first hook context
    /// as `machine/session-id`, or `machine/session-id/agent-id` in a subagent.
    pub session_key: String,
}

/// Which session, and which scopes of it.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionScopeParams {
    /// The calling session's key, printed in this session's first hook context.
    pub session_key: String,
    /// The scope ids to turn on or off, as `session_scopes` lists them, all in
    /// one call. One scope is a list of one.
    pub scopes: Vec<String>,
}

/// Which session is asking, and which session it takes its scopes from.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionInheritParams {
    /// The calling session's key, printed in this session's first hook context.
    pub session_key: String,
    /// The session whose scopes this one takes over, as `machine/session-id`.
    /// It has to be a session this server has already seen.
    pub from_session_key: String,
}

/// The context a `session_key` names.
///
/// MCP tool calls carry no session identity, so the key is a parameter and can
/// be anything the model wrote. A key that names no context is refused as an
/// invalid parameter rather than silently creating a context of its own, which
/// would collect deliveries no session ever sees.
pub fn parse_session_key(session_key: &str) -> Result<ContextKey, ErrorData> {
    let segments: Vec<&str> = session_key.split('/').collect();
    let complete = segments.iter().all(|segment| !segment.is_empty());
    match segments.as_slice() {
        [machine, session_id] if complete => Ok(ContextKey::main(*machine, *session_id)),
        [machine, session_id, agent] if complete => {
            Ok(ContextKey::subagent(*machine, *session_id, *agent))
        }
        _ => Err(ErrorData::invalid_params(
            format!(
                "session_key must be `machine/session-id`, or `machine/session-id/agent-id` \
                 inside a subagent, not `{session_key}`"
            ),
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a session key split that drops the agent, which would let a
    /// subagent's calls change its parent session's scopes.
    #[test]
    fn a_session_key_with_three_parts_names_a_subagent() {
        let key = parse_session_key("alpha/session-1/agent-7").expect("three parts are a key");
        assert_eq!(key, ContextKey::subagent("alpha", "session-1", "agent-7"));
        let main = parse_session_key("alpha/session-1").expect("two parts are a key");
        assert_eq!(main, ContextKey::main("alpha", "session-1"));
    }

    /// Detects a key accepted with a part missing: `alpha/` would name the
    /// session with the empty id, a context no session ever reads.
    #[test]
    fn a_session_key_with_a_missing_or_extra_part_is_refused() {
        for key in [
            "",
            "alpha",
            "alpha/",
            "/session-1",
            "alpha//agent",
            "a/b/c/d",
        ] {
            assert!(
                parse_session_key(key).is_err(),
                "`{key}` names no context and must be refused"
            );
        }
    }
}
