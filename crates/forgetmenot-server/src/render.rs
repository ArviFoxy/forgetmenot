//! The text injected into a session's context.
//!
//! Plain text, deterministic, and ordered so that what the model must act on
//! comes before what it may look up: the critical bodies, then the index lines,
//! then what has been withdrawn. The section labels are wording, not contract;
//! the contract is that a critical body appears in full and a knowledge memory
//! appears as its id and description.

use std::collections::BTreeSet;

use crate::context::{ContextKey, Needs};
use crate::store::catalog::{Catalog, MemoryEntry};
use crate::store::memory::MemoryKind;
use crate::store::{MemoryId, ScopeId};

/// What to render for one event.
pub struct Delivery<'a> {
    pub key: &'a ContextKey,
    pub catalog: &'a Catalog,
    pub needs: &'a Needs,
    /// The context's scopes after this event's triggers were applied.
    pub active: &'a BTreeSet<ScopeId>,
    /// The scopes this event turned on, ordered by id: the ones its triggers
    /// named and the ones those imply alike.
    pub activated: &'a [ScopeId],
    /// Whether the store asks for a scope that became active with nothing to
    /// deliver to be named to the agent.
    pub announce_empty_scopes: bool,
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

    /// The scopes this event turned on that nothing it delivers belongs to,
    /// ordered by id, or nothing at all when the store does not ask for them.
    ///
    /// The one place this list is worked out: the text names exactly the scopes
    /// the answer exists for, and an event whose whole content is this list is
    /// answered because it is not empty.
    pub fn announced_scopes(&self) -> Vec<&ScopeId> {
        if !self.announce_empty_scopes {
            return Vec::new();
        }
        let delivering: BTreeSet<&ScopeId> = self
            .needs
            .delivered_ids()
            .filter_map(|id| self.catalog.memory(id))
            .flat_map(|memory| memory.scopes())
            .collect();
        self.activated
            .iter()
            .filter(|scope| !delivering.contains(scope))
            .collect()
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
        // A message has no file of its own: there is no id to fetch it by, and
        // the only scope it can belong to is the one whose file carries it, so
        // its heading names that scope and nothing else.
        match memory.id.scope_message_of() {
            Some(scope) => text.push_str(&format!("== scope: {scope} ==\n")),
            None => {
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
            }
        }
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

    let announced = delivery.announced_scopes();
    if !announced.is_empty() {
        text.push_str("== scopes activated ==\n");
        for scope in announced {
            text.push_str(scope.as_str());
            text.push('\n');
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
    let size = memory.document.delivered_text().len()
        + match memory.kind() {
            MemoryKind::Critical => 0,
            // An index line carries the id to fetch the memory by as well as the
            // description.
            MemoryKind::Knowledge => id.as_str().len(),
        };
    size as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Needs, RetractReason};
    use crate::store::MemoryId;
    use crate::test_support::{TestStore, catalog_of, memory_file, scope_file};

    /// A line of the critical memory's body and nothing else in the store, so
    /// that "delivered in full" can be told from "named in an index line".
    const RULE_BODY: &str = "Switch the bench supply off at the wall.";

    /// The description of the knowledge memory, which is the whole of what an
    /// index line says about it.
    const BINDER: &str = "The workshop references are all on paper in the binder";

    /// The message the scope below carries, which has no file and no id of its
    /// own.
    const BROAD_EXCEPT: &str =
        "Do not catch Exception broadly; catch the exception type the code can handle.";

    /// A store with one critical memory and one knowledge memory in `global`,
    /// and one scope whose only content is a message.
    fn store_files() -> Vec<(String, Option<Vec<u8>>)> {
        vec![
            memory_file(
                "bench-power",
                "critical",
                &["global", "workshop"],
                "Cut bench power at the wall before rewiring",
                &format!("# Cut bench power before rewiring\n\n{RULE_BODY}\n"),
            ),
            memory_file(
                "reading-list",
                "knowledge",
                &["global"],
                BINDER,
                "# The binder\n\nThe bench notes and the parts catalogue are in it.\n",
            ),
            scope_file("broad-except", &format!("message: '{BROAD_EXCEPT}'\n")),
        ]
    }

    /// The context these tests render for.
    fn key() -> ContextKey {
        ContextKey::main("alpha", "session-1")
    }

    /// The text rendered for `needs` in a context whose scopes are `active`,
    /// where `activated` is what this event turned on and the store's
    /// `announce_empty_scopes` is `announce`.
    fn render_of(
        catalog: &Catalog,
        needs: &Needs,
        active: &BTreeSet<ScopeId>,
        activated: &[ScopeId],
        announce: bool,
    ) -> String {
        let key = key();
        render(&Delivery {
            key: &key,
            catalog,
            needs,
            active,
            activated,
            announce_empty_scopes: announce,
            session_start: false,
            answer_file_threshold: None,
        })
    }

    /// What is owed when `new` has never been delivered here.
    fn owed(new: &[&str]) -> Needs {
        Needs {
            new: new.iter().map(|id| MemoryId::new(*id)).collect(),
            ..Needs::default()
        }
    }

    /// Detects a critical memory delivered as a line to look up, which leaves
    /// the model with a rule it has not read, and a knowledge memory delivered
    /// whole, which spends the context the index exists to save. The heading of
    /// a critical memory names the id and the scopes it is delivered for,
    /// because that is what the model passes back to the tools.
    #[test]
    fn a_critical_memory_arrives_in_full_and_a_knowledge_memory_as_its_id_and_description() {
        let (_store, catalog) = catalog_of(store_files());

        let text = render_of(
            &catalog,
            &owed(&["bench-power", "reading-list"]),
            &BTreeSet::from([ScopeId::global()]),
            &[],
            false,
        );

        assert!(
            text.contains("== critical: bench-power (scopes: global, workshop) ==")
                && text.contains(RULE_BODY),
            "the rule must arrive in full under a heading naming its id and scopes, got {text:?}"
        );
        assert!(
            text.contains(&format!("- reading-list: {BINDER}")),
            "the knowledge memory must arrive as its id and description, got {text:?}"
        );
        assert!(
            !text.contains("The bench notes and the parts catalogue"),
            "a knowledge memory's body must not be delivered, got {text:?}"
        );
    }

    /// Detects an answer whose sections are in an order the model reads the
    /// wrong way round: what it must act on has to come before what it may look
    /// up, and a withdrawal has to come after both, or a long answer cut to a
    /// preview loses the rules rather than the references.
    #[test]
    fn the_rules_to_act_on_come_before_the_index_and_the_withdrawals_come_after_both() {
        let (_store, catalog) = catalog_of(store_files());
        let needs = Needs {
            new: vec![MemoryId::new("bench-power"), MemoryId::new("reading-list")],
            retracted: vec![(MemoryId::new("bench-vice"), RetractReason::Deleted)],
            ..Needs::default()
        };

        let text = render_of(
            &catalog,
            &needs,
            &BTreeSet::from([ScopeId::global()]),
            &[],
            false,
        );

        let rule = text.find(RULE_BODY).expect("the rule is rendered");
        let index = text.find(BINDER).expect("the index line is rendered");
        let withdrawn = text.find("bench-vice").expect("the withdrawal is rendered");
        assert!(
            rule < index && index < withdrawn,
            "the order must be rule, index, withdrawal, got {text:?}"
        );
        assert!(
            text.contains(&format!(
                "- bench-vice ({})",
                RetractReason::Deleted.as_str()
            )),
            "a withdrawal must say why the memory is no longer to be acted on, got {text:?}"
        );
    }

    /// Detects a scope message printed as an ordinary memory: a message has no
    /// file, so there is no id to fetch it by and the only scope it can belong
    /// to is the one whose file carries it. A heading offering an id would send
    /// the model to a memory that does not exist.
    #[test]
    fn a_scope_message_is_headed_by_its_scope_and_offers_no_id_to_fetch_it_by() {
        let (_store, catalog) = catalog_of(store_files());
        let id = MemoryId::for_scope_message(&ScopeId::new("broad-except"));

        let text = render_of(
            &catalog,
            &Needs {
                new: vec![id],
                ..Needs::default()
            },
            &BTreeSet::from([ScopeId::new("broad-except")]),
            &[],
            false,
        );

        assert!(
            text.contains("== scope: broad-except ==") && text.contains(BROAD_EXCEPT),
            "the message must arrive in full under its scope, got {text:?}"
        );
        assert!(
            !text.contains("== critical:"),
            "a message must not be printed as a memory to fetch by id, got {text:?}"
        );
    }

    /// Detects an answer that names a scope the store did not ask to have
    /// named, one that names a scope whose memories it just delivered, and one
    /// that names nothing when a scope turned on with nothing to deliver: the
    /// first two report context where the memories already speak, and the third
    /// leaves the agent unable to tell that a scope came on at all.
    ///
    /// Source: the store's `announce_empty_scopes`, which exists for the scope
    /// that activates and delivers nothing.
    #[test]
    fn an_activated_scope_is_named_only_when_the_store_asks_and_only_if_it_delivered_nothing() {
        let (_store, catalog) = catalog_of(store_files());
        let global = ScopeId::global();
        let paperwork = ScopeId::new("paperwork");
        let active = BTreeSet::from([global.clone(), paperwork.clone()]);

        let silent = render_of(
            &catalog,
            &Needs::default(),
            &active,
            std::slice::from_ref(&paperwork),
            false,
        );
        assert!(
            !silent.contains("paperwork"),
            "the store did not ask for activated scopes to be named, got {silent:?}"
        );

        let announced = render_of(
            &catalog,
            &Needs::default(),
            &active,
            std::slice::from_ref(&paperwork),
            true,
        );
        assert!(
            announced.contains("== scopes activated ==")
                && announced.lines().any(|line| line == "paperwork"),
            "a scope that came on with nothing to deliver must be named, got {announced:?}"
        );

        let delivering = render_of(&catalog, &owed(&["bench-power"]), &active, &[global], true);
        assert!(
            !delivering.contains("== scopes activated =="),
            "a scope whose memory just arrived speaks for itself, got {delivering:?}"
        );
    }

    /// An answer built from `body` as one global critical memory, so that the
    /// whole of it is the notice and that memory.
    fn answer_of(body: &str, threshold: Option<u64>) -> String {
        let store = TestStore::with(vec![memory_file(
            "long-rule",
            "critical",
            &["global"],
            "A rule long enough to test the answer notice",
            body,
        )]);
        let catalog = store.catalog();
        let key = key();
        render(&Delivery {
            key: &key,
            catalog: &catalog,
            needs: &owed(&["long-rule"]),
            active: &BTreeSet::from([ScopeId::global()]),
            activated: &[],
            announce_empty_scopes: false,
            session_start: false,
            answer_file_threshold: threshold,
        })
    }

    /// Detects a notice that is missing, that comes after content the preview
    /// cut would remove, or that replaces the answer: past the threshold Claude
    /// Code shows the model a preview and a file path, so the first line has to
    /// tell it to read the file, and the file has to still hold the memory.
    ///
    /// Source: the setting's documented meaning and the preview rule in the
    /// README.
    #[test]
    fn an_answer_past_the_threshold_opens_with_the_notice_and_still_carries_the_whole_text() {
        let body = "Stop the run and read the log. ".repeat(20);

        let text = answer_of(&body, Some(200));

        let opening = text.lines().next().unwrap_or_default();
        assert!(
            opening.contains("READ THE FILE FIRST") && opening.contains("Read tool"),
            "the first line must tell the model to read the file, got {opening:?}"
        );
        assert!(
            text.contains(body.trim_end()),
            "the memory must still be in the answer in full, got {text:?}"
        );
    }

    /// Detects a notice sent whatever the length, and one sent although the
    /// store turned the notice off: Claude Code writes a file only past the
    /// threshold, so either would send the model looking for a file that was
    /// never written.
    #[test]
    fn no_notice_opens_an_answer_within_the_threshold_or_one_the_store_wants_none_for() {
        let long = "Stop the run and read the log. ".repeat(20);

        let within = answer_of("Stop the run and read the log.", Some(100_000));
        assert!(
            !within.contains("READ THE FILE"),
            "an answer Claude Code shows whole must carry no notice, got {within:?}"
        );
        assert!(
            within.starts_with("[forgetmenot] context "),
            "the answer must open with its context line, got {within:?}"
        );

        let turned_off = answer_of(&long, None);
        assert!(
            !turned_off.contains("READ THE FILE"),
            "a store that asks for no notice must get none, got {turned_off:?}"
        );
    }

    /// Detects the answer measured in bytes: Claude Code compares its own
    /// string length, in UTF-16 code units, so an answer of multi-byte
    /// characters that is past the threshold in bytes and within it in code
    /// units is shown whole and must carry no notice.
    #[test]
    fn the_threshold_is_counted_in_the_code_units_claude_code_measures_not_in_bytes() {
        let body = "\u{20AC}".repeat(120);
        let whole = answer_of(&body, None);
        let code_units = whole.encode_utf16().count() as u64;
        assert!(
            whole.len() as u64 > code_units,
            "the fixture must be longer in bytes than in code units, got {} against {code_units}",
            whole.len()
        );

        let at_its_length = answer_of(&body, Some(code_units));

        assert!(
            !at_its_length.contains("READ THE FILE"),
            "an answer within the threshold in code units must carry no notice, got \
             {at_its_length:?}"
        );
    }
}
