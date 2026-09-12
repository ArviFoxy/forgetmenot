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
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestedSource {
    /// The person, who did not ask for it to be rewritten.
    User,
    /// The model itself.
    Assistant,
}

impl From<RequestedSource> for MemorySource {
    fn from(source: RequestedSource) -> Self {
        match source {
            RequestedSource::User => MemorySource::User,
            RequestedSource::Assistant => MemorySource::Assistant,
        }
    }
}

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
    /// List archived memories as well. They are left out by default because an
    /// archived memory is delivered nowhere.
    #[serde(default)]
    pub include_archived: Option<bool>,
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
    pub source: RequestedSource,
    /// The memory itself, as markdown. `[[name]]` links to another memory.
    pub body: String,
    /// The commit's title line: one line, at most 72 characters, saying what
    /// changed and why.
    pub message: String,
    /// The version this write replaces, as `memory_get` reported it. Absent
    /// creates a memory at an id that is free.
    #[serde(default)]
    pub base_version: Option<String>,
}

/// One memory to archive.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryArchiveParams {
    /// The calling session's key, recorded as the commit's author.
    pub session_key: String,
    /// The memory id to archive.
    pub id: String,
    /// The commit's title line: one line, at most 72 characters, saying why the
    /// memory is no longer in force.
    pub message: String,
    /// The version this write replaces, as `memory_get` reported it. Absent
    /// archives whatever version the store holds now.
    #[serde(default)]
    pub base_version: Option<String>,
}

/// Which session is asking.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionParams {
    /// The calling session's key, printed in this session's first hook context
    /// as `machine/session-id`, or `machine/session-id/agent-id` in a subagent.
    pub session_key: String,
}

/// Which session, and which scope of it.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SessionScopeParams {
    /// The calling session's key, printed in this session's first hook context.
    pub session_key: String,
    /// The scope id, as `session_scopes` lists it.
    pub scope: String,
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
