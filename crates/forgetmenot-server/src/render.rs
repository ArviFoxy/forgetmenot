//! The text injected into a session's context.
//!
//! Plain text, deterministic, and ordered so that what the model must act on
//! comes before what it may look up: the critical bodies, then the index lines,
//! then what has been withdrawn. The section labels are wording, not contract;
//! the contract is that a critical body appears in full and a knowledge memory
//! appears as its id and description.

use std::collections::BTreeSet;

use crate::context::{ContextKey, Needs};
use crate::store::MemoryId;
use crate::store::catalog::{Catalog, MemoryEntry};
use crate::store::memory::MemoryKind;

/// What to render for one event.
pub struct Delivery<'a> {
    pub key: &'a ContextKey,
    pub catalog: &'a Catalog,
    pub needs: &'a Needs,
    /// The context's scopes after this event's triggers were applied.
    pub active: &'a BTreeSet<crate::store::ScopeId>,
    /// At a session start the model is also told which scopes it can turn on
    /// and which session key to pass to the MCP tools.
    pub session_start: bool,
}

impl Delivery<'_> {
    /// The critical memories to deliver in full, new before changed before
    /// stale, by id within each.
    fn critical(&self) -> Vec<&MemoryEntry> {
        self.needs
            .delivered_ids()
            .filter_map(|id| self.catalog.memory(id))
            .filter(|memory| memory.kind() == MemoryKind::Critical)
            .collect()
    }

    /// The knowledge memories to deliver as index lines, by id.
    fn knowledge(&self) -> Vec<&MemoryEntry> {
        let mut entries: Vec<&MemoryEntry> = self
            .needs
            .delivered_ids()
            .filter_map(|id| self.catalog.memory(id))
            .filter(|memory| memory.kind() == MemoryKind::Knowledge)
            .collect();
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        entries
    }
}

/// Render the text for one event. Empty sections are left out entirely.
pub fn render(delivery: &Delivery<'_>) -> String {
    let mut text = format!("[forgetmenot] context {}\n", delivery.key);

    for memory in delivery.critical() {
        let scopes = memory
            .scopes()
            .iter()
            .map(|scope| scope.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!(
            "== critical: {} (scopes: {scopes}) ==\n",
            memory.id
        ));
        text.push_str(memory.document.body.trim_end());
        text.push_str("\n\n");
    }

    let knowledge = delivery.knowledge();
    if !knowledge.is_empty() {
        text.push_str("== knowledge (memory_get <id>) ==\n");
        for memory in knowledge {
            text.push_str(&format!(
                "- {}: {}\n",
                memory.id,
                memory.document.description()
            ));
        }
    }

    if !delivery.needs.retracted.is_empty() {
        text.push_str("== retracted ==\n");
        for (id, reason) in &delivery.needs.retracted {
            text.push_str(&format!("- {id} ({})\n", reason.as_str()));
        }
    }

    if delivery.session_start {
        let available: Vec<&str> = delivery
            .catalog
            .scopes()
            .filter(|scope| !delivery.active.contains(&scope.id))
            .map(|scope| scope.id.as_str())
            .collect();
        if !available.is_empty() {
            text.push_str("== scopes available (session_scope_on <id>) ==\n");
            for id in available {
                text.push_str(id);
                text.push('\n');
            }
        }
        text.push_str(&format!("session_key: {}\n", delivery.key));
    }

    text
}

/// The size of what one memory contributes to the rendered text, recorded in
/// the statistics so that delivered bytes per session can be reported.
pub fn delivery_bytes(catalog: &Catalog, id: &MemoryId) -> u64 {
    let Some(memory) = catalog.memory(id) else {
        return 0;
    };
    let size = match memory.kind() {
        MemoryKind::Critical => memory.document.body.trim_end().len(),
        MemoryKind::Knowledge => memory.document.description().len() + id.as_str().len(),
    };
    size as u64
}
