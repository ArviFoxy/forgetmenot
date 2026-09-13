//! The MCP tools, mounted at `/mcp`.
//!
//! Each tool is a thin adapter over one function in [`crate::operations`], the
//! same functions the JSON API calls, so a rule about writes or scopes is
//! enforced in one place for both.
//!
//! The tools come in two families and the naming and the descriptions keep them
//! apart, because the two do very different things:
//!
//! - `memory_*` changes the store. Every write is a git commit and affects every
//!   context the memory's scopes cover.
//! - `session_*` changes the calling context alone and never touches the store.
//!
//! An MCP tool call carries no session identity, so the calling context is a
//! parameter: `session_key`, printed in the session's first hook context.

pub mod params;

use std::collections::BTreeSet;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde::Serialize;

use crate::app::AppState;
use crate::operations::{
    self, CurrentDocument, MemoryDeleteRequest, MemoryFilter, MemoryWriteRequest, OperationError,
};
use crate::stats::{MEMORY_GET_TOOL, ToolCallRecord};
use crate::store::validate::WriteMode;
use crate::store::{MemoryId, ScopeId};
use params::{
    MemoryDeleteParams, MemoryGetParams, MemoryIndexParams, MemoryPutParams, SessionInheritParams,
    SessionParams, SessionScopeParams, parse_session_key,
};

/// What the model is told about this server when it connects.
///
/// Two sentences, one per family, because a model that mixes them up either
/// commits to the store when it meant to change its own scopes, or expects a
/// scope change to reach everyone.
const INSTRUCTIONS: &str = "\
The memory management tools (memory_index, memory_get, memory_put, memory_delete) read and \
change the shared store of memories, where every write is one git commit and takes effect in \
every session the memory's scopes cover. The session management tools (session_scopes, \
session_scope_on, session_scope_off, session_inherit) change only the calling session's own \
scopes and never touch the store; every tool takes session_key, which is printed in this \
session's first hook context as machine/session-id, or machine/session-id/agent-id inside a \
subagent.";

/// The tool handler: one per connection, over the server's shared state.
#[derive(Clone)]
pub struct ToolServer {
    state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
}

impl ToolServer {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl ToolServer {
    #[tool(
        description = "Memory management family: the shared store of memories, where every write \
                       is one git commit and takes effect in every session the memory's scopes \
                       cover. This call only reads. Lists each memory's id, description, kind, \
                       scopes and version as JSON, which is what memory_get takes."
    )]
    async fn memory_index(
        &self,
        Parameters(params): Parameters<MemoryIndexParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let catalog = match self.state.store.snapshot().await {
            Ok(catalog) => catalog,
            Err(error) => return Ok(tool_failure(&OperationError::from(error))),
        };
        let filter = MemoryFilter {
            scope: None,
            kind: params.kind.map(Into::into),
        };
        let mut summaries = operations::memory_index(&catalog, &filter);
        if let Some(scopes) = params.scopes {
            let wanted: BTreeSet<ScopeId> = scopes.into_iter().map(ScopeId::new).collect();
            summaries.retain(|summary| summary.scopes.iter().any(|scope| wanted.contains(scope)));
        }
        json_text(&summaries)
    }

    #[tool(
        description = "Memory management family: the shared store of memories, where every write \
                       is one git commit and takes effect in every session the memory's scopes \
                       cover. This call only reads. Returns the whole memory as JSON: body, \
                       description, kind, scopes, version, links and backlinks. With session_key \
                       the body counts as delivered to that session, so the next hook event does \
                       not repeat it until it changes."
    )]
    async fn memory_get(
        &self,
        Parameters(params): Parameters<MemoryGetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = MemoryId::new(params.id);
        let context = match params.session_key.as_deref() {
            Some(key) => Some(parse_session_key(key)?),
            None => None,
        };
        let outcome = operations::memory_get(&self.state, &id, context.as_ref()).await;
        // Recorded only for a call that names its session: the statistics count
        // fetches per session, and a fetch by nobody belongs in no row.
        if let Some(key) = &context {
            self.state
                .stats
                .record_tool_call(ToolCallRecord {
                    ts: self.state.clock.now(),
                    tool: MEMORY_GET_TOOL.to_string(),
                    session_key: key.to_string(),
                    memory: Some(id.to_string()),
                    scope: None,
                    ok: outcome.is_ok(),
                })
                .await;
        }
        match outcome {
            Ok(document) => json_text(&document),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Memory management family: write one memory into the shared store. The \
                       write is one git commit, authored by session_key, and the memory is then \
                       delivered to every session its scopes cover. Leave base_version out to \
                       create a memory at an id that is free; to change one that exists, send \
                       the version memory_get reported, and if it is no longer current the write \
                       is refused and the current version is named."
    )]
    async fn memory_put(
        &self,
        Parameters(params): Parameters<MemoryPutParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let author = parse_session_key(&params.session_key)?;
        let id = MemoryId::new(params.id);
        let catalog = match self.state.store.snapshot().await {
            Ok(catalog) => catalog,
            Err(error) => return Ok(tool_failure(&OperationError::from(error))),
        };
        // A call without a version is a creation only where there is nothing to
        // overwrite; at an id that exists it is an update missing its version,
        // which the operation refuses rather than overwriting blind.
        let mode = if params.base_version.is_none() && catalog.memory(&id).is_none() {
            WriteMode::Create
        } else {
            WriteMode::Update
        };
        let request = MemoryWriteRequest {
            description: params.description,
            kind: params.kind.into(),
            scopes: params.scopes.into_iter().map(ScopeId::new).collect(),
            source: params.source.into(),
            body: params.body,
            base_version: params.base_version,
            author: author.to_string(),
            message: params.message,
        };
        match operations::memory_put(&self.state, &id, &request, mode).await {
            Ok(outcome) => json_text(&outcome),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Memory management family: remove one memory's file from the shared store, \
                       as one git commit authored by session_key. It stops being delivered \
                       anywhere, to every session, and the history keeps it, so every version it \
                       ever had is still readable. This is not how this session stops working in \
                       a scope: for that use session_scope_off, which leaves every memory as it \
                       is."
    )]
    async fn memory_delete(
        &self,
        Parameters(params): Parameters<MemoryDeleteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let author = parse_session_key(&params.session_key)?;
        let id = MemoryId::new(params.id);
        let base_version = match params.base_version {
            Some(version) => version,
            None => {
                let catalog = match self.state.store.snapshot().await {
                    Ok(catalog) => catalog,
                    Err(error) => return Ok(tool_failure(&OperationError::from(error))),
                };
                match catalog.memory(&id) {
                    Some(entry) => entry.version.to_string(),
                    None => {
                        return Ok(tool_failure(&OperationError::NotFound(format!(
                            "the memory `{id}`"
                        ))));
                    }
                }
            }
        };
        let request = MemoryDeleteRequest {
            base_version,
            author: author.to_string(),
            message: params.message,
        };
        match operations::memory_delete(&self.state, &id, &request).await {
            Ok(outcome) => json_text(&outcome),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Session management family: changes only the calling session and never \
                       touches the store. Reports the scope ids this session works in, and the \
                       ones it could turn on."
    )]
    async fn session_scopes(
        &self,
        Parameters(params): Parameters<SessionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let key = parse_session_key(&params.session_key)?;
        match operations::session_scopes(&self.state, &key).await {
            Ok(scopes) => json_text(&scopes),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Session management family: changes only the calling session and never \
                       touches the store. This session starts working in the scope, and the \
                       scope's memories, together with everything it implies, are delivered at \
                       the next hook event."
    )]
    async fn session_scope_on(
        &self,
        Parameters(params): Parameters<SessionScopeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let key = parse_session_key(&params.session_key)?;
        match operations::session_scope_on(&self.state, &key, &ScopeId::new(params.scope)).await {
            Ok(scopes) => json_text(&scopes),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Session management family: changes only the calling session and never \
                       touches the store. This session stops working in that scope; nothing \
                       happens to any memory, and every other session keeps getting them. The \
                       scope's memories are reported as withdrawn at the next hook event."
    )]
    async fn session_scope_off(
        &self,
        Parameters(params): Parameters<SessionScopeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let key = parse_session_key(&params.session_key)?;
        match operations::session_scope_off(&self.state, &key, &ScopeId::new(params.scope)).await {
            Ok(scopes) => json_text(&scopes),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Session management family: changes only the calling session and never \
                       touches the store. This session takes over another session's scopes, \
                       including that session's own scope, so its session memories become due \
                       here too. The other session has to be one this server has seen."
    )]
    async fn session_inherit(
        &self,
        Parameters(params): Parameters<SessionInheritParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let key = parse_session_key(&params.session_key)?;
        let source = parse_session_key(&params.from_session_key)?;
        match operations::session_inherit(&self.state, &key, &source).await {
            Ok(scopes) => json_text(&scopes),
            Err(error) => Ok(tool_failure(&error)),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ToolServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}

/// The `/mcp` service: the tools over `state`, with the transport's sessions
/// kept in memory, since a client that loses one reconnects and loses nothing.
///
/// The `Host` values the transport accepts are the loopback names it defaults to
/// plus the ones the configuration adds, so a server reached over the LAN by
/// name answers without the loopback client having to be configured for it.
pub fn service(state: Arc<AppState>) -> StreamableHttpService<ToolServer, LocalSessionManager> {
    let defaults = StreamableHttpServerConfig::default();
    let mut allowed_hosts = defaults.allowed_hosts.clone();
    allowed_hosts.extend(state.config.allowed_hosts.iter().cloned());
    StreamableHttpService::new(
        move || Ok(ToolServer::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        defaults.with_allowed_hosts(allowed_hosts),
    )
}

/// A resource as the JSON text of a successful call.
fn json_text<Body: Serialize>(body: &Body) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(body).map_err(|error| {
        // The caller can do nothing about this, so it is a protocol error rather
        // than an answer: the tool produced no result at all.
        ErrorData::internal_error(format!("the answer could not be serialised: {error}"), None)
    })?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// An operation's failure as a tool error, which is what the model reads.
///
/// A conflict names the version the store holds now, so the model can read that
/// version and write again instead of guessing; a refused document carries one
/// line per problem, so it knows which part to fix.
fn tool_failure(error: &OperationError) -> CallToolResult {
    let mut text = error.to_string();
    match error {
        OperationError::Conflict { current, .. } => {
            text.push_str(&format!(
                "; the current version is {}",
                current_version(current)
            ));
        }
        OperationError::Invalid { errors } => {
            for problem in errors {
                text.push_str(&format!("\n{}: {}", problem.path, problem.message));
            }
        }
        _ => {}
    }
    CallToolResult::error(vec![ContentBlock::text(text)])
}

/// The version of the document a conflict carries.
fn current_version(current: &CurrentDocument) -> &str {
    match current {
        CurrentDocument::Memory(document) => &document.version,
        CurrentDocument::Scope(document) => &document.version,
    }
}
