//! The text injected into a session's context, and the accounting of that text.
//!
//! The answer is built scope by scope: one section per scope that has something
//! to deliver, holding that scope's critical bodies and then its index lines. A
//! memory in several active scopes is printed once, under the first of them, so
//! every character of a section belongs to that scope alone. The lines that
//! belong to no scope, the context line, the notice, the withdrawals, the
//! activated scopes and the session start's lines, are the answer's overhead.
//!
//! Inside a section what the model must act on comes before what it may look up:
//! the critical bodies, then the index lines. The section labels are wording,
//! not contract; the contract is that a critical body appears in full, a
//! knowledge memory appears as its id and description, and the sections' lengths
//! plus the overhead are the length of the text.

use std::collections::{BTreeMap, BTreeSet};

use crate::context::{ContextKey, Needs};
use crate::store::catalog::{Catalog, MemoryEntry};
use crate::store::memory::MemoryKind;
use crate::store::{MemoryId, ScopeId, ScopeKind};

/// The line every answer opens with, before the context key: the one mark that
/// says a text is this server's own answer.
///
/// The renderer prints it and [`crate::hook::events::carries_own_answer`] looks
/// for it, so a change here changes both at once.
pub const ANSWER_MARKER: &str = "[forgetmenot] context ";

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
    /// At a session start the model is also told which session key to pass to
    /// the MCP tools.
    pub session_start: bool,
    /// The characters of an answer above which Claude Code shows the model a
    /// preview and a file path instead of the text, so the answer has to open
    /// by telling the model to read the file. `None` sends no such notice.
    pub answer_file_threshold: Option<u64>,
}

/// One rendered answer with the accounting of its text.
///
/// The accounting is what the statistics record: a scope's cost is its section's
/// length and a memory's cost is the length of the text printed for it, both
/// measured at the moment the answer was built rather than derived later from
/// the catalog.
#[derive(Clone, Debug)]
pub struct Rendered {
    pub text: String,
    /// One per scope with something to deliver, in the order printed.
    pub sections: Vec<Section>,
    /// UTF-16 units of the text that belongs to no scope: the context line, the
    /// notice, the retractions, the activated-scopes list, the session start's
    /// lines.
    pub overhead_chars: u64,
}

impl Rendered {
    /// The length of the whole text, in the UTF-16 code units Claude Code
    /// measures a hook answer in.
    pub fn chars(&self) -> u64 {
        chars_of(&self.text)
    }
}

/// What one scope's section of an answer holds.
#[derive(Clone, Debug)]
pub struct Section {
    pub scope: ScopeId,
    /// The section's heading line plus every memory printed under it, in UTF-16
    /// code units.
    pub chars: u64,
    /// Each memory printed here, in the order printed.
    pub memories: Vec<SectionMemory>,
}

/// One memory as it was printed in a section.
///
/// Both lengths are of exactly the text printed for this memory: its header
/// line or lines, its body or index line, and the blank line that follows it.
/// The two units are kept apart because they answer different questions: Claude
/// Code's own limit is in UTF-16 code units and the raw log is in bytes.
#[derive(Clone, Debug)]
pub struct SectionMemory {
    pub id: MemoryId,
    pub chars: u64,
    pub bytes: u64,
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

    /// The memories to deliver grouped into the sections they are printed in,
    /// in the order the sections are printed, each with its critical memories
    /// and then its knowledge memories.
    fn grouped_sections(&self) -> Vec<(ScopeId, Vec<&MemoryEntry>, Vec<&MemoryEntry>)> {
        type Grouped<'a> = BTreeMap<(bool, ScopeId), (Vec<&'a MemoryEntry>, Vec<&'a MemoryEntry>)>;
        let mut grouped: Grouped<'_> = BTreeMap::new();
        for memory in self.critical() {
            grouped
                .entry(section_key(&self.section_of(memory)))
                .or_default()
                .0
                .push(memory);
        }
        for memory in self.knowledge() {
            grouped
                .entry(section_key(&self.section_of(memory)))
                .or_default()
                .1
                .push(memory);
        }
        grouped
            .into_iter()
            .map(|((_, scope), (critical, knowledge))| (scope, critical, knowledge))
            .collect()
    }

    /// The scope one memory is printed under: the first of its active scopes in
    /// section order.
    ///
    /// A memory belongs to as many scopes as its file names, and the context
    /// works in as many as its triggers turned on; printing it once, under the
    /// first of the scopes it is delivered for, is what makes every character of
    /// the answer belong to exactly one scope. A memory whose scopes are all off
    /// is not delivered by the state machine, so the fall back to its own first
    /// scope only covers a rendering asked for outside an event.
    fn section_of(&self, memory: &MemoryEntry) -> ScopeId {
        let first = |scopes: &mut dyn Iterator<Item = &ScopeId>| -> Option<ScopeId> {
            scopes.min_by_key(|scope| section_key(scope)).cloned()
        };
        first(
            &mut memory
                .scopes()
                .iter()
                .filter(|scope| self.active.contains(scope)),
        )
        .or_else(|| first(&mut memory.scopes().iter()))
        .unwrap_or_else(ScopeId::global)
    }
}

/// How sections sort: `global` first wherever its id would fall, then by id.
///
/// `global` is the one scope every context works in, so its section opens the
/// answer rather than landing among the subjects alphabetically.
fn section_key(scope: &ScopeId) -> (bool, ScopeId) {
    (scope.kind() != ScopeKind::Global, scope.clone())
}

/// The length of a piece of the answer as Claude Code measures it, in UTF-16
/// code units, which is the JavaScript string length.
fn chars_of(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

/// Render the text for one event and account for every character of it. Empty
/// sections are left out entirely.
///
/// An answer longer than the store's threshold opens with [`file_notice`]: past
/// that length Claude Code writes the answer to a file and shows the model the
/// first 2000 characters, cut back to the last newline when that lies past the
/// first 1000, so the notice is one short paragraph at the very top and nothing
/// else is moved. The answer is measured the way Claude Code measures it, in
/// UTF-16 code units, which is the JavaScript string length; a byte count would
/// send the notice for a non-ASCII answer Claude Code shows whole. The notice
/// belongs to no scope, so it counts as overhead.
pub fn render(delivery: &Delivery<'_>) -> Rendered {
    let mut rendered = render_body(delivery);
    let Some(threshold) = delivery.answer_file_threshold else {
        return rendered;
    };
    // The count is of the answer alone: with the notice in front, the file
    // Claude Code writes is longer than this by the notice itself.
    let characters = rendered.chars();
    if characters <= threshold {
        return rendered;
    }
    let notice = format!("{}\n\n", file_notice(characters, threshold));
    rendered.overhead_chars += chars_of(&notice);
    rendered.text.insert_str(0, &notice);
    rendered
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

fn render_body(delivery: &Delivery<'_>) -> Rendered {
    let mut text = String::new();
    let mut overhead_chars = 0u64;
    let mut sections = Vec::new();

    let mut overhead = |text: &mut String, piece: &str| {
        overhead_chars += chars_of(piece);
        text.push_str(piece);
    };

    overhead(&mut text, &format!("{ANSWER_MARKER}{}\n", delivery.key));

    for (scope, critical, knowledge) in delivery.grouped_sections() {
        let heading = format!("== scope: {scope} ==\n");
        let mut section = Section {
            scope,
            chars: chars_of(&heading),
            memories: Vec::new(),
        };
        text.push_str(&heading);
        for memory in critical {
            // A message has no file of its own: there is no id to fetch it by,
            // and the only scope it can belong to is the one whose file carries
            // it, which is the section it is printed in.
            let header = match memory.id.scope_message_of() {
                Some(_) => "-- message --\n".to_string(),
                None => format!("-- critical: {} --\n", memory.id),
            };
            let piece = format!("{header}{}\n\n", memory.document.body.trim_end());
            section.push(&mut text, &memory.id, &piece);
        }
        for (position, memory) in knowledge.iter().enumerate() {
            // The block's own heading is printed once and goes with the first
            // index line under it, so that every character of the section is
            // accounted to a memory of it.
            let header = match position {
                0 => "-- knowledge --\n",
                _ => "",
            };
            let piece = format!(
                "{header}- {}: {}\n",
                memory.id,
                memory.document.description()
            );
            section.push(&mut text, &memory.id, &piece);
        }
        sections.push(section);
    }

    if !delivery.needs.retracted.is_empty() {
        overhead(&mut text, "== retracted ==\n");
        for (id, reason) in &delivery.needs.retracted {
            overhead(&mut text, &format!("- {id} ({})\n", reason.as_str()));
        }
    }

    let announced = delivery.announced_scopes();
    if !announced.is_empty() {
        overhead(&mut text, "== scopes activated ==\n");
        for scope in announced {
            overhead(&mut text, &format!("{scope}\n"));
        }
    }

    // A session start prints the key the MCP tools take and nothing about the
    // scopes the session is not in: what is not active is not delivered, not
    // named, not listed. The tools are where a session asks what else exists.
    if delivery.session_start {
        overhead(&mut text, &format!("session_key: {}\n", delivery.key));
    }

    Rendered {
        text,
        sections,
        overhead_chars,
    }
}

impl Section {
    /// Print one memory's text into the answer and account for it here.
    fn push(&mut self, text: &mut String, id: &MemoryId, piece: &str) {
        let chars = chars_of(piece);
        text.push_str(piece);
        self.chars += chars;
        self.memories.push(SectionMemory {
            id: id.clone(),
            chars,
            bytes: piece.len() as u64,
        });
    }
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

    /// A store with one critical memory in `global` and `workshop`, one
    /// knowledge memory in `global`, and one scope whose only content is a
    /// message.
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

    /// Every character of the text belongs to exactly one place: a memory of a
    /// section, that section's heading, or the overhead.
    ///
    /// The identity the whole statistics report rests on, so every test here
    /// asserts it on the answer it rendered: a section that counts a character
    /// twice, or a line nothing counts at all, makes what a scope cost a
    /// different number from what the model was sent.
    fn assert_accounted(rendered: &Rendered) {
        let sections: u64 = rendered.sections.iter().map(|section| section.chars).sum();
        assert_eq!(
            rendered.overhead_chars + sections,
            rendered.chars(),
            "the overhead {} and the sections {sections} must be the whole text, got {:?}",
            rendered.overhead_chars,
            rendered.text
        );
        for section in &rendered.sections {
            let memories: u64 = section.memories.iter().map(|memory| memory.chars).sum();
            assert!(
                memories <= section.chars,
                "a section's memories must fit inside it, got {memories} in {section:?}"
            );
        }
    }

    /// The answer rendered for `needs` in a context whose scopes are `active`,
    /// where `activated` is what this event turned on and the store's
    /// `announce_empty_scopes` is `announce`, with its accounting checked.
    fn render_of(
        catalog: &Catalog,
        needs: &Needs,
        active: &BTreeSet<ScopeId>,
        activated: &[ScopeId],
        announce: bool,
    ) -> Rendered {
        let key = key();
        let rendered = render(&Delivery {
            key: &key,
            catalog,
            needs,
            active,
            activated,
            announce_empty_scopes: announce,
            session_start: false,
            answer_file_threshold: None,
        });
        assert_accounted(&rendered);
        rendered
    }

    /// The text of the answer rendered for those arguments.
    fn text_of(
        catalog: &Catalog,
        needs: &Needs,
        active: &BTreeSet<ScopeId>,
        activated: &[ScopeId],
        announce: bool,
    ) -> String {
        render_of(catalog, needs, active, activated, announce).text
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
    /// a critical memory names the id, because that is what the model passes
    /// back to the tools.
    #[test]
    fn a_critical_memory_arrives_in_full_and_a_knowledge_memory_as_its_id_and_description() {
        let (_store, catalog) = catalog_of(store_files());

        let text = text_of(
            &catalog,
            &owed(&["bench-power", "reading-list"]),
            &BTreeSet::from([ScopeId::global()]),
            &[],
            false,
        );

        assert!(
            text.contains("-- critical: bench-power --") && text.contains(RULE_BODY),
            "the rule must arrive in full under a heading naming its id, got {text:?}"
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

    /// Detects an answer whose parts are in an order the model reads the wrong
    /// way round: what it must act on has to come before what it may look up,
    /// and a withdrawal has to come after both, or a long answer cut to a
    /// preview loses the rules rather than the references.
    #[test]
    fn the_rules_to_act_on_come_before_the_index_and_the_withdrawals_come_after_both() {
        let (_store, catalog) = catalog_of(store_files());
        let needs = Needs {
            new: vec![MemoryId::new("bench-power"), MemoryId::new("reading-list")],
            retracted: vec![(MemoryId::new("bench-vice"), RetractReason::Deleted)],
            ..Needs::default()
        };

        let text = text_of(
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
    /// file, so there is no id to fetch it by, and the only scope it can belong
    /// to is the one whose file carries it. A heading offering an id would send
    /// the model to a memory that does not exist, and a message printed outside
    /// its scope's section would charge another scope for it.
    #[test]
    fn a_scope_message_is_printed_in_its_scopes_section_and_offers_no_id_to_fetch_it_by() {
        let (_store, catalog) = catalog_of(store_files());
        let scope = ScopeId::new("broad-except");
        let id = MemoryId::for_scope_message(&scope);

        let rendered = render_of(
            &catalog,
            &Needs {
                new: vec![id.clone()],
                ..Needs::default()
            },
            &BTreeSet::from([scope.clone()]),
            &[],
            false,
        );

        assert!(
            rendered.text.contains("== scope: broad-except ==")
                && rendered.text.contains("-- message --")
                && rendered.text.contains(BROAD_EXCEPT),
            "the message must arrive in full under its scope, got {:?}",
            rendered.text
        );
        assert!(
            !rendered.text.contains("-- critical:"),
            "a message must not be printed as a memory to fetch by id, got {:?}",
            rendered.text
        );
        assert_eq!(
            rendered
                .sections
                .iter()
                .map(|section| (
                    section.scope.clone(),
                    section
                        .memories
                        .iter()
                        .map(|memory| memory.id.clone())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            vec![(scope, vec![id])],
            "the message must be accounted to the scope whose file carries it"
        );
    }

    /// Detects a memory in several active scopes printed once per scope, which
    /// would send the model the same rule twice, and one accounted to a scope
    /// it was not printed under, which would charge two scopes for one text.
    ///
    /// Source: `bench-power` is in `global` and `workshop` and both are active,
    /// and sections are printed with `global` first, so it belongs to `global`.
    #[test]
    fn a_memory_in_two_active_scopes_is_printed_once_under_the_first_of_them() {
        let (_store, catalog) = catalog_of(store_files());
        let workshop = ScopeId::new("workshop");

        let rendered = render_of(
            &catalog,
            &owed(&["bench-power"]),
            &BTreeSet::from([ScopeId::global(), workshop.clone()]),
            &[],
            false,
        );

        assert_eq!(
            rendered.text.matches(RULE_BODY).count(),
            1,
            "the rule must be sent once, got {:?}",
            rendered.text
        );
        assert_eq!(
            rendered
                .sections
                .iter()
                .map(|section| section.scope.clone())
                .collect::<Vec<_>>(),
            vec![ScopeId::global()],
            "the memory belongs to the first of its active scopes, so `workshop` has no section"
        );
        assert!(
            !rendered.text.contains(workshop.as_str()),
            "a scope nothing was printed under must not be named, got {:?}",
            rendered.text
        );
    }

    /// Detects sections printed in the order the catalog happened to be walked,
    /// and `global` printed among the subjects alphabetically: the order is what
    /// decides which scope a shared memory is charged to, so it has to be the
    /// same for every answer.
    ///
    /// Source: the rule that sections are in scope id order with `global` first.
    /// `alpha-scope` sorts before `global`, `zeta-scope` after it.
    #[test]
    fn sections_are_printed_with_global_first_and_the_rest_in_scope_id_order() {
        let (_store, catalog) = catalog_of(vec![
            memory_file(
                "global-rule",
                "critical",
                &["global"],
                "A rule for every session",
                "# Global\n\nThe global rule.\n",
            ),
            memory_file(
                "alpha-rule",
                "critical",
                &["alpha-scope"],
                "A rule for alpha",
                "# Alpha\n\nThe alpha rule.\n",
            ),
            memory_file(
                "zeta-rule",
                "critical",
                &["zeta-scope"],
                "A rule for zeta",
                "# Zeta\n\nThe zeta rule.\n",
            ),
        ]);
        let active = BTreeSet::from([
            ScopeId::global(),
            ScopeId::new("alpha-scope"),
            ScopeId::new("zeta-scope"),
        ]);

        let rendered = render_of(
            &catalog,
            &owed(&["alpha-rule", "global-rule", "zeta-rule"]),
            &active,
            &[],
            false,
        );

        assert_eq!(
            rendered
                .sections
                .iter()
                .map(|section| section.scope.to_string())
                .collect::<Vec<_>>(),
            vec![
                "global".to_string(),
                "alpha-scope".to_string(),
                "zeta-scope".to_string()
            ],
            "global opens the answer and the rest follow by id"
        );
        let printed: Vec<usize> = ["global", "alpha-scope", "zeta-scope"]
            .into_iter()
            .map(|scope| {
                rendered
                    .text
                    .find(&format!("== scope: {scope} =="))
                    .unwrap_or_else(|| panic!("the section of {scope} is rendered"))
            })
            .collect();
        assert!(
            printed[0] < printed[1] && printed[1] < printed[2],
            "the text must be in the same order as the accounting, got {:?}",
            rendered.text
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

        let silent = text_of(
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

        let announced = text_of(
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

        let delivering = text_of(&catalog, &owed(&["bench-power"]), &active, &[global], true);
        assert!(
            !delivering.contains("== scopes activated =="),
            "a scope whose memory just arrived speaks for itself, got {delivering:?}"
        );
    }

    /// An answer built from `body` as one global critical memory, so that the
    /// whole of it is the notice and that memory.
    fn answer_of(body: &str, threshold: Option<u64>) -> Rendered {
        let store = TestStore::with(vec![memory_file(
            "long-rule",
            "critical",
            &["global"],
            "A rule long enough to test the answer notice",
            body,
        )]);
        let catalog = store.catalog();
        let key = key();
        let rendered = render(&Delivery {
            key: &key,
            catalog: &catalog,
            needs: &owed(&["long-rule"]),
            active: &BTreeSet::from([ScopeId::global()]),
            activated: &[],
            announce_empty_scopes: false,
            session_start: false,
            answer_file_threshold: threshold,
        });
        assert_accounted(&rendered);
        rendered
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

        let text = answer_of(&body, Some(200)).text;

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

    /// Detects a notice counted against the scope whose memory made the answer
    /// long: the notice is Claude Code's own limit speaking and belongs to no
    /// scope, so a report of what a scope costs must not grow when the notice
    /// appears.
    #[test]
    fn the_notice_is_overhead_and_leaves_what_the_scope_cost_where_it_was() {
        let body = "Stop the run and read the log. ".repeat(20);

        let without = answer_of(&body, None);
        let with = answer_of(&body, Some(200));

        assert!(
            with.text.len() > without.text.len(),
            "the fixture must be past the threshold, so the notice is in the answer"
        );
        assert_eq!(
            with.sections
                .iter()
                .map(|section| section.chars)
                .collect::<Vec<_>>(),
            without
                .sections
                .iter()
                .map(|section| section.chars)
                .collect::<Vec<_>>(),
            "the notice must not change what the scope was charged"
        );
        assert_eq!(
            with.overhead_chars - without.overhead_chars,
            with.chars() - without.chars(),
            "every character the notice added must be overhead"
        );
    }

    /// Detects a notice sent whatever the length, and one sent although the
    /// store turned the notice off: Claude Code writes a file only past the
    /// threshold, so either would send the model looking for a file that was
    /// never written.
    #[test]
    fn no_notice_opens_an_answer_within_the_threshold_or_one_the_store_wants_none_for() {
        let long = "Stop the run and read the log. ".repeat(20);

        let within = answer_of("Stop the run and read the log.", Some(100_000)).text;
        assert!(
            !within.contains("READ THE FILE"),
            "an answer Claude Code shows whole must carry no notice, got {within:?}"
        );
        assert!(
            within.starts_with(ANSWER_MARKER),
            "the answer must open with its context line, got {within:?}"
        );

        let turned_off = answer_of(&long, None).text;
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
        let code_units = whole.chars();
        assert!(
            whole.text.len() as u64 > code_units,
            "the fixture must be longer in bytes than in code units, got {} against {code_units}",
            whole.text.len()
        );

        let at_its_length = answer_of(&body, Some(code_units));

        assert!(
            !at_its_length.text.contains("READ THE FILE"),
            "an answer within the threshold in code units must carry no notice, got {:?}",
            at_its_length.text
        );
    }
}
