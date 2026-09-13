//! The per-context state machine.
//!
//! A context is one Claude Code session, or one subagent inside it. Its state
//! is the set of scopes it works in and the record of what has been delivered
//! into it; the functions here are pure, so what a context needs next is
//! decided by the catalog and the state alone and can be tested without a
//! server.

pub mod registry;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::store::catalog::Catalog;
use crate::store::memory::MemoryKind;
use crate::store::{MemoryId, ScopeId};

/// The agent part of the key of a session's main context, as opposed to a
/// subagent's.
pub const MAIN_AGENT: &str = "main";

/// Which context an event belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContextKey {
    pub machine: String,
    pub session_id: String,
    /// [`MAIN_AGENT`] for the session itself, else the subagent's id.
    pub agent: String,
}

impl ContextKey {
    /// The key of a session's main context.
    pub fn main(machine: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            machine: machine.into(),
            session_id: session_id.into(),
            agent: MAIN_AGENT.to_string(),
        }
    }

    /// The key of one subagent inside a session.
    pub fn subagent(
        machine: impl Into<String>,
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            machine: machine.into(),
            session_id: session_id.into(),
            agent: agent_id.into(),
        }
    }

    /// Whether this is a subagent's context.
    pub fn is_subagent(&self) -> bool {
        self.agent != MAIN_AGENT
    }

    /// The main context of the same session, which is a subagent's parent.
    pub fn session_context(&self) -> Self {
        Self::main(self.machine.clone(), self.session_id.clone())
    }

    /// The implicit scope of this context's session.
    pub fn session_scope(&self) -> ScopeId {
        ScopeId::session(&self.machine, &self.session_id)
    }
}

impl fmt::Display for ContextKey {
    /// The form MCP tools take as `session_key`: the agent part is written only
    /// when there is a subagent to name.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.machine, self.session_id)?;
        if self.is_subagent() {
            write!(formatter, "/{}", self.agent)?;
        }
        Ok(())
    }
}

/// How much of a memory a context has been given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Form {
    /// The whole body.
    Full,
    /// One index line, with the id to fetch the body by.
    Index,
}

impl Form {
    /// The form a memory of this kind is delivered in when it becomes due.
    pub fn for_kind(kind: MemoryKind) -> Self {
        match kind {
            MemoryKind::Critical => Form::Full,
            MemoryKind::Knowledge => Form::Index,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Form::Full => "full",
            Form::Index => "index",
        }
    }
}

/// What was delivered to a context for one memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    /// The blob id of the file that was delivered, as hex.
    pub version: String,
    pub form: Form,
    /// The context size when it was delivered, absent when the client could not
    /// read one; staleness cannot be judged without it.
    pub tokens: Option<u64>,
}

/// One context's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextState {
    /// The scopes this context works in.
    pub active: BTreeSet<ScopeId>,
    /// What has been delivered, and in what form.
    pub delivered: BTreeMap<MemoryId, Shown>,
    /// The context this one inherited its scopes from, for a subagent.
    pub parent: Option<ContextKey>,
    /// The directory `claude` was started in: the `cwd` of this context's first
    /// event, remembered for as long as the context lives and never moved by a
    /// later event. Defaulted, so that a snapshot an older build wrote loads.
    #[serde(default)]
    pub session_directory: Option<String>,
    /// The name the user gave this session with `/rename`, as the client last
    /// read it from the transcript. `None` while the session has never been
    /// named; an event that could not read the transcript leaves the name
    /// already held alone rather than clearing it.
    #[serde(default)]
    pub session_title: Option<String>,
    /// The first thing the user said in this session, as the client last read
    /// it from the transcript, already cut short there. `None` while the
    /// transcript carries none.
    #[serde(default)]
    pub first_prompt: Option<String>,
    /// The task a subagent was given at its `SubagentStart`, which is the only
    /// thing that says what the subagent is for. `None` for a session's own
    /// context, which is given no task.
    #[serde(default)]
    pub task: Option<String>,
    pub last_seen: DateTime<Utc>,
}

impl ContextState {
    /// A context that has been told nothing yet.
    pub fn fresh(
        active: BTreeSet<ScopeId>,
        parent: Option<ContextKey>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            active,
            delivered: BTreeMap::new(),
            parent,
            session_directory: None,
            session_title: None,
            first_prompt: None,
            task: None,
            last_seen: now,
        }
    }
}

/// The scopes a context starts in: everything global, everything for its
/// machine, and its own session's silo.
pub fn initial_active(machine: &str, session_id: &str) -> BTreeSet<ScopeId> {
    BTreeSet::from([
        ScopeId::global(),
        ScopeId::machine(machine),
        ScopeId::session(machine, session_id),
    ])
}

/// Why a memory stopped being delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetractReason {
    /// The memory is no longer in the store: its file was deleted.
    Deleted,
    /// No scope this context works in covers the memory any more, whether
    /// because a scope was turned off or because the memory was moved out of
    /// every scope this context works in.
    NoActiveScope,
}

impl RetractReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RetractReason::Deleted => "deleted",
            RetractReason::NoActiveScope => "no longer in an active scope",
        }
    }
}

/// What a context is owed, computed against one catalog snapshot.
///
/// Each list is sorted by memory id so that the rendered context does not
/// depend on the order the catalog happened to be walked in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    /// Due and never delivered here.
    pub new: Vec<MemoryId>,
    /// Delivered, but the store holds a different version now.
    pub changed: Vec<MemoryId>,
    /// Delivered so long ago in context that it is out of the model's reach,
    /// whatever form it was delivered in.
    pub stale: Vec<MemoryId>,
    /// Delivered, and no longer to be acted on.
    pub retracted: Vec<(MemoryId, RetractReason)>,
}

impl Needs {
    /// Whether nothing at all is owed, in which case the event is answered with
    /// an empty response.
    pub fn is_empty(&self) -> bool {
        self.new.is_empty()
            && self.changed.is_empty()
            && self.stale.is_empty()
            && self.retracted.is_empty()
    }

    /// The memories to deliver, in the order they are rendered.
    pub fn delivered_ids(&self) -> impl Iterator<Item = &MemoryId> {
        self.new
            .iter()
            .chain(self.changed.iter())
            .chain(self.stale.iter())
    }

    /// Whether a critical memory is new or changed here, which is the condition
    /// that interrupts a tool call.
    pub fn has_critical_arrival(&self, catalog: &Catalog) -> bool {
        self.new
            .iter()
            .chain(self.changed.iter())
            .filter_map(|id| catalog.memory(id))
            .any(|memory| memory.kind() == MemoryKind::Critical)
    }
}

/// What a context is owed given the store, its state and its context size.
///
/// The store's own settings decide two of the answers: `reminder_tokens` is the
/// growth in context after which anything already delivered is delivered again,
/// and without it nothing is ever stale; `deliver_knowledge_index` decides
/// whether a knowledge memory is delivered at all or only fetched on demand.
/// Staleness is judged only when both the delivery and the event carry a context
/// size.
pub fn compute_needs(catalog: &Catalog, state: &ContextState, tokens_now: Option<u64>) -> Needs {
    let settings = catalog.settings();
    let mut needs = Needs::default();
    let due = catalog.due(&state.active);
    // Every due memory, whether or not it is delivered, so that a memory the
    // context holds is not withdrawn merely because its kind is not delivered.
    let due_ids: BTreeSet<&MemoryId> = due.iter().map(|memory| &memory.id).collect();

    for memory in &due {
        let version = memory.version.to_string();
        let delivers = memory.kind() == MemoryKind::Critical || settings.deliver_knowledge_index;
        match state.delivered.get(&memory.id) {
            None if delivers => needs.new.push(memory.id.clone()),
            None => {}
            Some(shown) if shown.version != version => {
                if delivers {
                    needs.changed.push(memory.id.clone());
                }
            }
            Some(shown) => {
                // Every kind goes stale: what was shown that long ago is out of
                // the model's reach whatever form it took, so it is delivered
                // again in the form its kind gets, the rule in full and the
                // knowledge memory as its index line.
                if delivers
                    && let Some(reminder_tokens) = settings.reminder_tokens
                    && let (Some(delivered_at), Some(now)) = (shown.tokens, tokens_now)
                    && now.saturating_sub(delivered_at) >= reminder_tokens
                {
                    needs.stale.push(memory.id.clone());
                }
            }
        }
    }

    for id in state.delivered.keys() {
        if due_ids.contains(id) {
            continue;
        }
        let reason = match catalog.memory(id) {
            // Gone from the store: the model must stop acting on it, and the
            // store no longer says why it was ever due.
            None => RetractReason::Deleted,
            Some(_) => RetractReason::NoActiveScope,
        };
        needs.retracted.push((id.clone(), reason));
    }

    needs.new.sort();
    needs.changed.sort();
    needs.stale.sort();
    needs.retracted.sort();
    needs
}

/// Record that `needs` was delivered: what was sent is now delivered at the
/// store's version and this context size, and what was retracted is forgotten.
pub fn record_delivery(
    state: &mut ContextState,
    needs: &Needs,
    catalog: &Catalog,
    tokens_now: Option<u64>,
) {
    for id in needs.delivered_ids() {
        let Some(memory) = catalog.memory(id) else {
            continue;
        };
        state.delivered.insert(
            id.clone(),
            Shown {
                version: memory.version.to_string(),
                form: Form::for_kind(memory.kind()),
                tokens: tokens_now,
            },
        );
    }
    for (id, _) in &needs.retracted {
        state.delivered.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a key printed in a form MCP tools cannot take: the session key a
    /// main context prints must not carry the word `main`, and a subagent's
    /// must name the agent.
    #[test]
    fn context_keys_print_the_session_key_mcp_tools_take() {
        assert_eq!(
            ContextKey::main("alpha", "session-1").to_string(),
            "alpha/session-1"
        );
        assert_eq!(
            ContextKey::subagent("alpha", "session-1", "agent-7").to_string(),
            "alpha/session-1/agent-7"
        );
    }

    /// Detects a subagent key whose parent is computed as anything other than
    /// its own session, which would make a subagent inherit from elsewhere.
    #[test]
    fn a_subagent_keys_parent_is_its_own_session() {
        let child = ContextKey::subagent("alpha", "session-1", "agent-7");
        assert!(child.is_subagent());
        assert_eq!(
            child.session_context(),
            ContextKey::main("alpha", "session-1")
        );
        assert!(!child.session_context().is_subagent());
    }
}
