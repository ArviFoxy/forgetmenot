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
//! - `settings_*` reads and changes the store's behaviour settings, which is the
//!   same kind of change: one git commit, in force for every context.
//! - `branch_*` opens, inspects and lands a transaction, which is a git branch:
//!   writes made on one become a single commit on `main` when it lands.
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
use crate::context::ContextKey;
use crate::operations::branches::{self, LandRequest};
use crate::operations::settings::{self as settings_operations, SettingsWriteRequest};
use crate::operations::{
    self, CurrentDocument, DeleteRequest, DocumentKind, MemoryFilter, MemoryWriteRequest,
    OperationError, RenameRequest, ReplaceTextRequest, SetFieldsRequest,
};
use crate::stats::{MEMORY_GET_TOOL, ToolCallRecord};
use crate::store::branch::BranchName;
use crate::store::validate::WriteMode;
use crate::store::{MemoryId, ScopeId};
use params::{
    BranchCreateParams, BranchLandParams, BranchParams, MemoryDeleteParams, MemoryGetParams,
    MemoryIdParams, MemoryIndexParams, MemoryPutParams, MemoryRenameParams,
    MemoryReplaceTextParams, MemorySetFieldsParams, SessionInheritParams, SessionParams,
    SessionScopeParams, SettingsSetParams, parse_session_key,
};

/// What the model is told about this server when it connects.
///
/// Two sentences, one per family, because a model that mixes them up either
/// commits to the store when it meant to change its own scopes, or expects a
/// scope change to reach everyone.
const INSTRUCTIONS: &str = "\
The memory management tools (memory_index, memory_get, memory_history, memory_blame, \
memory_put, memory_replace_text, memory_set_fields, memory_rename, memory_delete) read and \
change the shared store of memories, where every write is one git commit and takes effect in \
every session the memory's scopes cover. memory_history and memory_blame are where the age of a \
memory and the author of one of its lines come from, rather than any date written into its text. \
A branch is how several changes land as one commit: branch_create opens one, every write \
tool takes its name in branch and then changes nothing any session sees, and branch_land \
squashes the whole branch onto main as a single commit. The settings management tools \
(settings_get, settings_set) read and change the shared store's behaviour settings, which say \
when memories are repeated, when a tool call is held and what a subagent starts with; a change \
is one git commit and takes effect in every session. The session management tools \
(session_scopes, session_scope_on, session_scope_off, session_inherit) change only the calling \
session's own scopes and never touch the store; every tool takes session_key, which is printed \
in this session's first hook context as machine/session-id, or machine/session-id/agent-id \
inside a subagent.";

/// The name of every tool this server registers, as the tool itself is named.
///
/// A client prefixes these with `mcp__` and the name the user registered this
/// server under, which is how the hook recognises the memory system's own
/// traffic and keeps it away from the triggers: these calls carry scope ids,
/// memory ids, session keys and whole memory bodies, none of which says anything
/// about the work the session is doing. The unit test below holds this list to
/// what the router registers.
pub const FORGETMENOT_TOOL_NAMES: &[&str] = &[
    "memory_index",
    "memory_get",
    "memory_history",
    "memory_blame",
    "memory_put",
    "memory_delete",
    "memory_replace_text",
    "memory_set_fields",
    "memory_rename",
    "branch_create",
    "branch_list",
    "branch_diff",
    "branch_land",
    "branch_abandon",
    "settings_get",
    "settings_set",
    "session_scopes",
    "session_scope_on",
    "session_scope_off",
    "session_inherit",
];

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

    /// Record a write in the writing session's own context, so that what it just
    /// wrote is not delivered back to it at its next event.
    ///
    /// A write on a branch records nothing: nothing on a branch is delivered to
    /// any context, and the land is where the session is recorded as holding
    /// what it wrote.
    async fn note_writes(
        &self,
        author: &ContextKey,
        branch: Option<&BranchName>,
        ids: &[MemoryId],
    ) {
        if branch.is_none() {
            operations::note_own_writes(&self.state, author, ids).await;
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
        description = "Memory management family: the shared store of memories, where every write \
                       is one git commit and takes effect in every session the memory's scopes \
                       cover. This call only reads. Returns the commits that changed one memory \
                       as JSON, newest first, each with its oid, time, author and title. Read \
                       how old a memory is, when it last changed and who wrote it from here, \
                       never from a date written into its text."
    )]
    async fn memory_history(
        &self,
        Parameters(params): Parameters<MemoryIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match operations::history(&self.state, DocumentKind::Memory, &params.id).await {
            Ok(commits) => json_text(&commits),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Memory management family: the shared store of memories, where every write \
                       is one git commit and takes effect in every session the memory's scopes \
                       cover. This call only reads. Returns the memory's file line by line as \
                       JSON, each line with the commit that last changed it: oid, time and \
                       author, the way git blame does. The whole file as it is stored, \
                       frontmatter included, so a line number is the line number in the file. \
                       Read when one line appeared and who wrote it from here, never from a date \
                       written into the text."
    )]
    async fn memory_blame(
        &self,
        Parameters(params): Parameters<MemoryIdParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let id = MemoryId::new(params.id);
        match operations::memory_blame(&self.state, &id).await {
            Ok(blame) => json_text(&blame),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Memory management family: write one memory into the shared store. The \
                       write is one git commit, authored by session_key, and the memory is then \
                       delivered to every session its scopes cover. Leave base_version out to \
                       create a memory at an id that is free; to change one that exists, send \
                       the version memory_get reported, and if it is no longer current the write \
                       is refused and the current version is named. metadata carries the other \
                       frontmatter keys of the memory's file, Claude Code's own type and any \
                       other key a person or a tool keeps there."
    )]
    async fn memory_put(
        &self,
        Parameters(params): Parameters<MemoryPutParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let author = parse_session_key(&params.session_key)?;
        let id = MemoryId::new(params.id);
        let branch = match requested_branch(params.branch.as_deref()) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        let catalog = match operations::target_catalog(&self.state, branch.as_ref()).await {
            Ok(catalog) => catalog,
            Err(error) => return Ok(tool_failure(&error)),
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
            metadata: params.metadata,
            body: params.body,
            base_version: params.base_version,
            author: author.to_string(),
            message: params.message,
        };
        match operations::memory_put(&self.state, &id, &request, mode, branch.as_ref()).await {
            Ok(outcome) => {
                self.note_writes(&author, branch.as_ref(), &[id]).await;
                json_text(&outcome)
            }
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
        let branch = match requested_branch(params.branch.as_deref()) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        let base_version = match params.base_version {
            Some(version) => version,
            None => {
                let catalog = match operations::target_catalog(&self.state, branch.as_ref()).await {
                    Ok(catalog) => catalog,
                    Err(error) => return Ok(tool_failure(&error)),
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
        let request = DeleteRequest {
            base_version,
            author: author.to_string(),
            message: params.message,
        };
        match operations::memory_delete(&self.state, &id, &request, branch.as_ref()).await {
            Ok(outcome) => {
                self.note_writes(&author, branch.as_ref(), &[id]).await;
                json_text(&outcome)
            }
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Memory management family: replace one exact snippet of one memory's body \
                       in the shared store, as one git commit authored by session_key. Everything \
                       else about the memory, its description, kind and scopes, is left alone. \
                       old_string is matched literally and must appear exactly once unless \
                       replace_all is set; a snippet that is not there is refused, so read the \
                       body with memory_get and copy from it."
    )]
    async fn memory_replace_text(
        &self,
        Parameters(params): Parameters<MemoryReplaceTextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let author = parse_session_key(&params.session_key)?;
        let branch = match requested_branch(params.branch.as_deref()) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        let request = ReplaceTextRequest {
            old_string: params.old_string,
            new_string: params.new_string,
            replace_all: params.replace_all,
            base_version: params.base_version,
            author: author.to_string(),
            message: params.message,
        };
        let id = MemoryId::new(params.id);
        match operations::memory_replace_text(&self.state, &id, &request, branch.as_ref()).await {
            Ok(outcome) => {
                self.note_writes(&author, branch.as_ref(), &[id]).await;
                json_text(&outcome)
            }
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Memory management family: set some of one memory's fields in the shared \
                       store, as one git commit authored by session_key. The body is not touched \
                       at all, and a field that is not sent keeps the value it has, so this is \
                       how a memory changes scope or kind without its text being sent back. \
                       metadata carries the other frontmatter keys of the memory's file, Claude \
                       Code's own type and any other key a person or a tool keeps there."
    )]
    async fn memory_set_fields(
        &self,
        Parameters(params): Parameters<MemorySetFieldsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let author = parse_session_key(&params.session_key)?;
        let branch = match requested_branch(params.branch.as_deref()) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        let request = SetFieldsRequest {
            description: params.description,
            kind: params.kind.map(Into::into),
            scopes: params
                .scopes
                .map(|scopes| scopes.into_iter().map(ScopeId::new).collect()),
            source: params.source.map(Into::into),
            metadata: params.metadata,
            base_version: params.base_version,
            author: author.to_string(),
            message: params.message,
        };
        let id = MemoryId::new(params.id);
        match operations::memory_set_fields(&self.state, &id, &request, branch.as_ref()).await {
            Ok(outcome) => {
                self.note_writes(&author, branch.as_ref(), &[id]).await;
                json_text(&outcome)
            }
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Memory management family: move one memory in the shared store to another \
                       id, as one git commit authored by session_key. Every [[link]] to it in \
                       every other memory is rewritten in the same commit, so no version of the \
                       store has the file moved and the links left behind. The new id has to be \
                       free."
    )]
    async fn memory_rename(
        &self,
        Parameters(params): Parameters<MemoryRenameParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let author = parse_session_key(&params.session_key)?;
        let branch = match requested_branch(params.branch.as_deref()) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        let request = RenameRequest {
            to: MemoryId::new(params.to),
            base_version: params.base_version,
            author: author.to_string(),
            message: params.message,
        };
        let from = MemoryId::new(params.from);
        match operations::memory_rename(&self.state, &from, &request, branch.as_ref()).await {
            Ok(outcome) => {
                // Both ids: the memory is gone from the old one and the session
                // holds it at the new one.
                self.note_writes(&author, branch.as_ref(), &[from, request.to])
                    .await;
                json_text(&outcome)
            }
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Branch family: open a branch, which is how several writes become one \
                       commit on main. Answers with its name; pass that name as branch to every \
                       write, and nothing any session is delivered changes until branch_land \
                       squashes the branch. The branch starts from main as it is now."
    )]
    async fn branch_create(
        &self,
        Parameters(params): Parameters<BranchCreateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let owner = parse_session_key(&params.session_key)?;
        match branches::branch_create(&self.state, &owner.to_string()).await {
            Ok(opened) => json_text(&opened),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Branch family: the branches that are open and waiting to become one \
                       commit on main, with the session that opened each, how far it is ahead of \
                       and behind main, and when it was last written to."
    )]
    async fn branch_list(&self) -> Result<CallToolResult, ErrorData> {
        match branches::branch_list(&self.state).await {
            Ok(rows) => json_text(&rows),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Branch family: what one branch would change if it became one commit on \
                       main. Reports each file with whether it is added, modified or deleted and \
                       its diff against the point the branch left main, and how far the branch is \
                       ahead and behind."
    )]
    async fn branch_diff(
        &self,
        Parameters(params): Parameters<BranchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let branch = match parse_branch(&params.branch) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        match branches::branch_diff(&self.state, &branch).await {
            Ok(diff) => json_text(&diff),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Branch family: land one branch, so every write on it becomes one commit on \
                       main with message as its title, and the branch is gone. A change made on \
                       main meanwhile is merged in. A file the branch and main both changed \
                       incompatibly is reported with the text of both sides and nothing lands: \
                       write the version you want on the branch and land again."
    )]
    async fn branch_land(
        &self,
        Parameters(params): Parameters<BranchLandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let author = parse_session_key(&params.session_key)?;
        let branch = match parse_branch(&params.branch) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        let request = LandRequest {
            message: params.message,
            author: author.to_string(),
        };
        match branches::branch_land(&self.state, &branch, &request).await {
            Ok(landed) => {
                // The land is where the writes on the branch reach `main`, so it
                // is where the landing session is recorded as holding them.
                let written = branches::landed_memories(&self.state, &landed).await;
                self.note_writes(&author, None, &written).await;
                json_text(&landed)
            }
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Branch family: delete one branch with everything written on it, so none of \
                       it ever becomes one commit on main. main is untouched. This is how a \
                       transaction is thrown away."
    )]
    async fn branch_abandon(
        &self,
        Parameters(params): Parameters<BranchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let branch = match parse_branch(&params.branch) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        match branches::branch_abandon(&self.state, &branch).await {
            Ok(abandoned) => json_text(&abandoned),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Settings management family: the shared store's behaviour settings, which \
                       say when a critical memory is repeated, when a tool call is held, what a \
                       subagent starts with and what is delivered. They are part of the store, so \
                       a change is one git commit and is in force for every session. This call \
                       only reads. Reports every setting with the value in force, the version to \
                       write against, and the type and meaning of each key."
    )]
    async fn settings_get(&self) -> Result<CallToolResult, ErrorData> {
        match settings_operations::settings_get(&self.state, None).await {
            Ok(document) => json_text(&document),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Settings management family: change one of the shared store's behaviour \
                       settings, as one git commit authored by session_key. The settings are part \
                       of the store, so the new value is in force in every session at its next \
                       hook event, not only in this one. key is one of the keys settings_get \
                       reports and value has the type it gives; every other setting is left as it \
                       is."
    )]
    async fn settings_set(
        &self,
        Parameters(params): Parameters<SettingsSetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let author = parse_session_key(&params.session_key)?;
        let branch = match requested_branch(params.branch.as_deref()) {
            Ok(branch) => branch,
            Err(failure) => return Ok(*failure),
        };
        let request = SettingsWriteRequest {
            value: params.value,
            // The tools take no version: a setting is one value, and a model
            // that read it and writes it back is not resolving an edit conflict.
            base_version: None,
            author: author.to_string(),
            message: params.message,
        };
        match settings_operations::settings_set(&self.state, &params.key, &request, branch.as_ref())
            .await
        {
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
                       touches the store. This session starts working in every scope in scopes, \
                       and those scopes' memories, together with everything they imply, are \
                       delivered at the next hook event. Name every scope to turn on in the \
                       one call; one scope is a list of one. An id that is not a scope of this \
                       store refuses the whole call, naming that id, and turns none of them on."
    )]
    async fn session_scope_on(
        &self,
        Parameters(params): Parameters<SessionScopeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let key = parse_session_key(&params.session_key)?;
        let scopes = requested_scopes(params.scopes);
        match operations::session_scope_on(&self.state, &key, &scopes).await {
            Ok(scopes) => json_text(&scopes),
            Err(error) => Ok(tool_failure(&error)),
        }
    }

    #[tool(
        description = "Session management family: changes only the calling session and never \
                       touches the store. This session stops working in every scope in scopes; \
                       nothing happens to any memory, and every other session keeps getting \
                       them. Those scopes' memories are reported as withdrawn at the next hook \
                       event. Name every scope to turn off in the one call; one scope is a \
                       list of one. An id that is always on for a session refuses the whole \
                       call, naming that id, and turns none of them off."
    )]
    async fn session_scope_off(
        &self,
        Parameters(params): Parameters<SessionScopeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let key = parse_session_key(&params.session_key)?;
        let scopes = requested_scopes(params.scopes);
        match operations::session_scope_off(&self.state, &key, &scopes).await {
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

/// The branch a write names, if any.
///
/// A name that is not a branch name is a failure the model reads and can correct,
/// not a protocol error: it wrote the name.
fn requested_branch(branch: Option<&str>) -> Result<Option<BranchName>, Box<CallToolResult>> {
    match branch.filter(|name| !name.is_empty()) {
        None => Ok(None),
        Some(name) => parse_branch(name).map(Some),
    }
}

/// The scopes a session tool is asked to change, in the order they were named.
///
/// An id the store does not have is left for the operation to refuse, which is
/// where the whole list is either applied or refused.
fn requested_scopes(scopes: Vec<String>) -> Vec<ScopeId> {
    scopes.into_iter().map(ScopeId::new).collect()
}

/// The branch a branch tool is about.
fn parse_branch(branch: &str) -> Result<BranchName, Box<CallToolResult>> {
    // Boxed because a tool result is large and the failure is the rare path.
    BranchName::parse(branch)
        .map_err(|error| Box::new(tool_failure(&OperationError::BadBranchName(error))))
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
/// line per problem, so it knows which part to fix; a land that could not be
/// merged carries each file with the text on both sides, so the model can write
/// the version it wants on the branch and land again.
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
        OperationError::MergeConflicts { conflicts } => {
            for conflict in conflicts {
                text.push_str(&format!(
                    "\n\n{}\n--- on main ---\n{}\n--- on the branch ---\n{}",
                    conflict.path,
                    conflict.ours.as_deref().unwrap_or(DELETED_SIDE),
                    conflict.theirs.as_deref().unwrap_or(DELETED_SIDE),
                ));
            }
            text.push_str(
                "\nWrite the version you want on the branch with a normal write, then land again.",
            );
        }
        _ => {}
    }
    CallToolResult::error(vec![ContentBlock::text(text)])
}

/// What a conflict shows for a side that no longer has the file.
const DELETED_SIDE: &str = "(the file was deleted on this side)";

/// The version of the document a conflict carries.
fn current_version(current: &CurrentDocument) -> &str {
    match current {
        CurrentDocument::Memory(document) => &document.version,
        CurrentDocument::Scope(document) => &document.version,
        // A store that has never had a settings file has no version to name.
        CurrentDocument::Settings(document) => document.version.as_deref().unwrap_or(NO_VERSION),
    }
}

/// What a conflict shows for a document the store does not have yet.
const NO_VERSION: &str = "none";

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects [`FORGETMENOT_TOOL_NAMES`] drifting from the tools the router
    /// registers: a tool missing from the list has its inputs and results
    /// matched against triggers, which activates scopes from the memory
    /// system's own traffic, and a name in the list that no tool has hides a
    /// typo that would do the same. Source: the `#[tool]` functions of this
    /// file, which are what a client can call.
    #[test]
    fn the_listed_tool_names_are_the_ones_the_router_registers() {
        let registered: BTreeSet<String> = ToolServer::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        let listed: BTreeSet<String> = FORGETMENOT_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        assert_eq!(
            listed, registered,
            "every registered tool must be listed and nothing else"
        );
    }
}
