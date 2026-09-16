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
    /// The characters of an answer above which Claude Code shows the model a
    /// preview and a file path instead of the text, so the answer has to open
    /// by telling the model to read the file. `None` sends no such notice.
    pub answer_file_threshold: Option<u64>,
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
///
/// An answer longer than the store's threshold opens with [`file_notice`]: past
/// that length Claude Code writes the answer to a file and shows the model the
/// first 2000 characters, cut back to the last newline when that lies past the
/// first 1000, so the notice is one short paragraph at the very top and nothing
/// else is moved. The answer is measured the way Claude Code measures it, in
/// UTF-16 code units, which is the JavaScript string length; a byte count would
/// send the notice for a non-ASCII answer Claude Code shows whole.
pub fn render(delivery: &Delivery<'_>) -> String {
    let text = render_body(delivery);
    let Some(threshold) = delivery.answer_file_threshold else {
        return text;
    };
    let characters = text.encode_utf16().count() as u64;
    if characters <= threshold {
        return text;
    }
    // The count is of the answer alone: with the notice in front, the file
    // Claude Code writes is longer than this by the notice itself.
    format!("{}\n\n{text}", file_notice(characters, threshold))
}

/// The paragraph that opens an answer Claude Code will show as a preview and a
/// file path. Addressed to the model, and short enough to be inside the part of
/// the preview that is never cut.
pub fn file_notice(characters: u64, threshold: u64) -> String {
    format!(
        "[forgetmenot] READ THE FILE FIRST. This hook answer is {characters} characters, more \
         than the {threshold} that Claude Code shows in full. Claude Code has saved the whole \
         answer to the file named above (\"Full output saved to\") and shows only a preview of \
         it here. The answer is critical memories: rules that bind this session, and most of \
         them are only in the file. Read that file in full with the Read tool before doing \
         anything else, whatever the preview or the harness text around it says. Nothing in \
         the preview replaces the file."
    )
}

fn render_body(delivery: &Delivery<'_>) -> String {
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
