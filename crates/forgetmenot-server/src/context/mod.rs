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

use crate::store::catalog::{Catalog, MemoryEntry};
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

    /// The context a key names, in the form [`fmt::Display`] writes it: the form
    /// the MCP tools take as `session_key` and the API takes in a path.
    ///
    /// The one place the grammar lives, so a key printed to a session is read
    /// back as the same context wherever it is handed in. A key with a part
    /// missing names nothing: `alpha/` would be the session with the empty id,
    /// which no session ever reads.
    pub fn parse(text: &str) -> Result<Self, ContextKeyError> {
        let segments: Vec<&str> = text.split('/').collect();
        let complete = segments.iter().all(|segment| !segment.is_empty());
        match segments.as_slice() {
            [machine, session_id] if complete => Ok(Self::main(*machine, *session_id)),
            [machine, session_id, agent] if complete => {
                Ok(Self::subagent(*machine, *session_id, *agent))
            }
            _ => Err(ContextKeyError(text.to_string())),
        }
    }
}

/// A text that names no context.
#[derive(Clone, Debug, thiserror::Error)]
#[error(
    "a context key is `machine/session-id`, or `machine/session-id/agent-id` for a subagent, not `{0}`"
)]
pub struct ContextKeyError(String);

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
    /// The context size the staleness of this delivery is counted from, absent
    /// while no event of this context has reported one. Staleness cannot be
    /// judged without it; the first event that carries a size fills it in, in
    /// [`ContextState::note_tokens`].
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
    /// The task a subagent was given, as the client last read it from the
    /// subagent's metadata file, which is the only thing that says what the
    /// subagent is for. `None` for a session's own context, which is given no
    /// task, and for a subagent whose metadata file could not be read.
    #[serde(default)]
    pub task: Option<String>,
    /// The kind of subagent this is, from its `SubagentStart`, for example
    /// `general-purpose`. `None` for a session's own context, and for a
    /// subagent first seen at an event other than its start.
    #[serde(default)]
    pub agent_type: Option<String>,
    /// The context size the last event that carried one reported. `None` while
    /// no event of this context has read one. Defaulted, so that a snapshot an
    /// older build wrote loads.
    #[serde(default)]
    pub tokens: Option<u64>,
    /// The context size at the last activation of each scope that has a
    /// `forget` rule, which is what the count since that activation is measured
    /// from. Only those scopes are in here: the count is the only thing the
    /// record is read for.
    #[serde(default)]
    pub activated_at: BTreeMap<ScopeId, u64>,
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
            agent_type: None,
            tokens: None,
            activated_at: BTreeMap::new(),
            last_seen: now,
        }
    }

    /// Record that an event of this context reported `tokens` as its context
    /// size, and take that size as the baseline of everything here that has
    /// none.
    ///
    /// A count in tokens starts at the first event of this context that carried
    /// a size, so this is where anything recorded without one gets its baseline:
    /// a delivery made where nothing reported a size, which is a `memory_get`
    /// fetch or a write by the context itself, and a scope with a `forget` rule
    /// that was turned on the same way or inherited from a parent whose sizes
    /// are not this context's. Until then neither can be counted at all: a
    /// staleness or a forgetting measured from a size nobody read is measured
    /// from nothing.
    ///
    /// The one place a missing baseline is decided, so that a delivery and a
    /// scope answer for it the same way.
    pub fn note_tokens(&mut self, tokens: u64, catalog: &Catalog) {
        self.tokens = Some(tokens);
        for shown in self.delivered.values_mut() {
            if shown.tokens.is_none() {
                shown.tokens = Some(tokens);
            }
        }
        for scope in &self.active {
            if catalog.forget_rule(scope).is_some() {
                self.activated_at.entry(scope.clone()).or_insert(tokens);
            }
        }
    }

    /// Record that `scope` was activated in this context at `tokens` context
    /// tokens: a trigger matched for it, or a tool call turned it on.
    ///
    /// This is the one place an activation is recorded, so that a trigger fire
    /// and a `session_scope_on` start the same count. A scope the catalog gives
    /// no `forget` rule is not recorded at all, because nothing reads the
    /// activation of a scope that never turns itself off.
    ///
    /// `tokens` is the context size at the activation: for a trigger the size
    /// the event carries, and for an activation with no event of its own the
    /// size the last event of this context reported, which is what
    /// [`ContextState::tokens`] holds. `None` records nothing and leaves an
    /// earlier activation standing, because a count cannot run from a context
    /// size nobody read.
    pub fn note_activation(&mut self, scope: &ScopeId, tokens: Option<u64>, catalog: &Catalog) {
        let Some(tokens) = tokens else {
            return;
        };
        if catalog.forget_rule(scope).is_none() {
            return;
        }
        self.activated_at.insert(scope.clone(), tokens);
    }

    /// Record that this context holds `memory` at its current version, in the
    /// form its kind is delivered in.
    ///
    /// This is the one place a [`Shown`] is made, so that a fetch, a delivery
    /// and a write by the context itself all record the same thing: the version
    /// and the form come from the catalog entry, never from what a caller asked
    /// for. `tokens` is the context size at that moment, absent where nothing
    /// reports one, in which case the count starts at the next event that does:
    /// see [`ContextState::note_tokens`].
    pub fn note_shown(&mut self, memory: &MemoryEntry, tokens: Option<u64>) {
        self.delivered.insert(
            memory.id.clone(),
            Shown {
                version: memory.version.to_string(),
                form: Form::for_kind(memory.kind()),
                tokens,
            },
        );
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
    /// Delivered at another version whose delivered text the new one only
    /// removed lines from, so the context has already been told everything the
    /// new text says. Nothing is rendered for these; the new version is recorded
    /// as seen.
    pub shrunk: Vec<MemoryId>,
    /// Delivered so long ago in context that it is out of the model's reach,
    /// whatever form it was delivered in.
    pub stale: Vec<MemoryId>,
    /// Delivered, and no longer to be acted on.
    pub retracted: Vec<(MemoryId, RetractReason)>,
}

impl Needs {
    /// Whether nothing at all is owed, in which case the event is answered with
    /// an empty response.
    ///
    /// A memory that only shrank is not owed: it puts no text in the answer, so
    /// an event that found nothing else is answered with nothing.
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

/// The text a context was given for a memory, at the version it holds, for the
/// memories the store now holds another version of.
pub type PreviousTexts = BTreeMap<MemoryId, String>;

/// Whether `new` is `old` with lines taken out and nothing put in: every line of
/// `new` appears in `old` in order, and the two texts differ.
///
/// Lines are compared exactly, because this is asked of the text the model was
/// actually given, in which a reworded line is new text. A trailing newline is
/// not a line, so two texts that differ only in one are the same text here and
/// nothing shrank.
pub fn shrinks(old: &str, new: &str) -> bool {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines == new_lines {
        return false;
    }
    // `any` leaves the iterator just past the line it matched, so each line of
    // the new text is looked for after the one before it: the walk accepts only
    // an in-order match.
    let mut remaining = old_lines.into_iter();
    new_lines
        .into_iter()
        .all(|line| remaining.any(|old_line| old_line == line))
}

/// What a context is owed given the store, its state and its context size.
///
/// The store's own settings decide two of the answers: `reminder_tokens` is the
/// growth in context after which anything already delivered is delivered again,
/// and without it nothing is ever stale; `deliver_knowledge_index` decides
/// whether a knowledge memory is delivered at all or only fetched on demand.
/// Staleness is judged only when both the delivery and the event carry a context
/// size.
///
/// `previous` is the text the context was given for a memory whose version has
/// moved since. A memory it has no text for is delivered whole, so a version that
/// cannot be read costs a delivery and never a missed one.
pub fn compute_needs(
    catalog: &Catalog,
    state: &ContextState,
    tokens_now: Option<u64>,
    previous: &PreviousTexts,
) -> Needs {
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
                    // A version whose delivered text only lost lines says nothing
                    // this context has not been given, so it is recorded as seen
                    // and not sent again.
                    match previous.get(&memory.id) {
                        Some(old) if shrinks(old, memory.document.delivered_text()) => {
                            needs.shrunk.push(memory.id.clone())
                        }
                        _ => needs.changed.push(memory.id.clone()),
                    }
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
    needs.shrunk.sort();
    needs.stale.sort();
    needs.retracted.sort();
    needs
}

/// Record that `needs` was delivered: what was sent is now delivered at the
/// store's version and this context size, and what was retracted is forgotten.
///
/// A memory that only shrank is recorded at the new version as well, although
/// nothing was sent for it: the context has been given every line the new text
/// holds, so it is not owed the memory again.
pub fn record_delivery(
    state: &mut ContextState,
    needs: &Needs,
    catalog: &Catalog,
    tokens_now: Option<u64>,
) {
    for id in needs.delivered_ids().chain(&needs.shrunk) {
        let Some(memory) = catalog.memory(id) else {
            continue;
        };
        state.note_shown(memory, tokens_now);
    }
    for (id, _) in &needs.retracted {
        state.delivered.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestStore, catalog_of, memory_file, settings_file};

    /// The growth in context after which a delivery is out of the model's
    /// reach, as the store below asks for it.
    const REMINDER: u64 = 1_000;

    /// The context size the first delivery of these tests was made at, so that
    /// every later size is this plus a stated amount.
    const START: u64 = 10_000;

    /// The critical memory's body, written out here rather than read back from
    /// the store, so the lines an edit takes out are visible in the test.
    const THREE_RULES: &str = "# Cut bench power before rewiring\n\n\
                               Switch the bench supply off at the wall.\n\
                               Confirm on the meter that the rail reads zero.\n\
                               Label the supply before leaving the bench.\n";

    /// The same body with the last line taken out and nothing put in.
    const TWO_RULES: &str = "# Cut bench power before rewiring\n\n\
                             Switch the bench supply off at the wall.\n\
                             Confirm on the meter that the rail reads zero.\n";

    /// The same body with the last line reworded, which leaves it the same
    /// number of lines and says something the context has not been given.
    const THREE_RULES_REWORDED: &str = "# Cut bench power before rewiring\n\n\
                                        Switch the bench supply off at the wall.\n\
                                        Confirm on the meter that the rail reads zero.\n\
                                        Leave the supply labelled for the next person.\n";

    /// The description of the knowledge memory, which is the whole of what a
    /// context is ever given for it.
    const BINDER: &str = "The workshop references are all on paper in the binder";

    /// A store with one critical memory and one knowledge memory in `global`,
    /// one critical memory in `widgets`, and reminders at [`REMINDER`].
    fn store_files() -> Vec<(String, Option<Vec<u8>>)> {
        store_files_with_settings(&format!("reminder_tokens: {REMINDER}\n"))
    }

    /// The same store with `yaml` as the whole of its settings file, or with no
    /// settings file at all when `yaml` is empty.
    fn store_files_with_settings(yaml: &str) -> Vec<(String, Option<Vec<u8>>)> {
        let mut files = vec![
            memory_file(
                "bench-power",
                "critical",
                &["global"],
                "Cut bench power at the wall before rewiring",
                THREE_RULES,
            ),
            memory_file(
                "reading-list",
                "knowledge",
                &["global"],
                BINDER,
                "# The binder\n\nThe bench notes and the parts catalogue are in it.\n",
            ),
            memory_file(
                "widget-naming",
                "critical",
                &["widgets"],
                "A widget part number is never reused",
                "# Widget part numbers are immutable\n\nDownstream drawings cite them.\n",
            ),
        ];
        if !yaml.is_empty() {
            files.push(settings_file(yaml));
        }
        files
    }

    /// A context in `active` that was given each of `delivered` at the store's
    /// current version, in the form its kind gets, at `tokens` context tokens.
    fn state_holding(
        catalog: &Catalog,
        active: BTreeSet<ScopeId>,
        delivered: &[&str],
        tokens: Option<u64>,
    ) -> ContextState {
        let mut state = ContextState::fresh(active, None, chrono::Utc::now());
        for name in delivered {
            let memory = catalog
                .memory(&MemoryId::new(*name))
                .unwrap_or_else(|| panic!("{name} must be in the store"));
            state.note_shown(memory, tokens);
        }
        state.tokens = tokens;
        state
    }

    /// The scopes every context of this session works in.
    fn implicit() -> BTreeSet<ScopeId> {
        initial_active("alpha", "session-1")
    }

    /// Detects a context started in a scope nobody turned on, and one missing
    /// any of the three every context works in: memories of another session's
    /// work would arrive, or a machine's and a session's own rules never would.
    ///
    /// Source: the rule that a context begins in everything global, everything
    /// for its machine, and its own session's silo.
    #[test]
    fn a_context_begins_in_the_global_the_machine_and_its_own_session_scope_and_no_other() {
        assert_eq!(
            initial_active("alpha", "session-1"),
            BTreeSet::from([
                ScopeId::global(),
                ScopeId::machine("alpha"),
                ScopeId::session("alpha", "session-1"),
            ])
        );
    }

    /// Detects a withdrawal reported as a deletion when the memory is still in
    /// the store, and one reported as out of scope when the file is gone: the
    /// model would be told a rule was retired when the session merely stopped
    /// working in its scope, or told to look up a memory that no longer exists.
    #[test]
    fn a_memory_out_of_every_active_scope_is_withdrawn_as_such_and_a_deleted_one_as_deleted() {
        let (_store, catalog) = catalog_of(store_files());
        let mut state = state_holding(&catalog, implicit(), &["widget-naming"], Some(START));
        // A memory this context holds that the store does not have at all,
        // which is what a file deleted behind the server leaves behind.
        state.delivered.insert(
            MemoryId::new("bench-vice"),
            Shown {
                version: "0".repeat(40),
                form: Form::Full,
                tokens: Some(START),
            },
        );

        let needs = compute_needs(&catalog, &state, Some(START), &PreviousTexts::new());

        assert_eq!(
            needs.retracted,
            vec![
                (MemoryId::new("bench-vice"), RetractReason::Deleted),
                (MemoryId::new("widget-naming"), RetractReason::NoActiveScope),
            ],
            "each withdrawal must name the reason the model can act on"
        );
    }

    /// Detects an index line that is never sent again after the memory's text
    /// changes: the description the model holds would stay the old one.
    #[test]
    fn a_knowledge_memory_is_sent_again_when_its_version_changes() {
        let (_store, catalog) = catalog_of(store_files());
        let id = MemoryId::new("reading-list");
        let mut state = state_holding(
            &catalog,
            implicit(),
            &["bench-power", "reading-list"],
            Some(START),
        );
        assert!(
            compute_needs(&catalog, &state, Some(START), &PreviousTexts::new()).is_empty(),
            "nothing is owed while the delivered version is current"
        );

        state
            .delivered
            .get_mut(&id)
            .expect("the memory was delivered")
            .version = "0".repeat(40);
        let needs = compute_needs(&catalog, &state, Some(START), &PreviousTexts::new());

        assert_eq!(
            needs.changed,
            vec![id],
            "a memory delivered at another version must count as changed"
        );
    }

    /// Detects staleness judged without knowing the context size a memory was
    /// delivered at, which would make every delivery stale at once.
    #[test]
    fn staleness_is_not_judged_without_the_context_size_of_the_delivery() {
        let (_store, catalog) = catalog_of(store_files());
        let state = state_holding(&catalog, implicit(), &["bench-power"], None);

        let needs = compute_needs(&catalog, &state, Some(START * 100), &PreviousTexts::new());

        assert!(
            needs.stale.is_empty(),
            "without the size at delivery nothing may be called stale, got {:?}",
            needs.stale
        );
    }

    /// Detects a reminder threshold that fires early, one that fires late, and
    /// one that covers the critical memories alone: what was delivered
    /// [`REMINDER`] tokens of context ago is out of the model's reach whatever
    /// form it took, and until then delivering any of it again spends the
    /// context the reminder exists to protect.
    ///
    /// Source: the store's `reminder_tokens`, which is a growth in context
    /// since the delivery, so 999 tokens on is short of it and 1 000 is it.
    #[test]
    fn a_delivery_of_either_form_goes_stale_at_the_stores_threshold_and_not_a_token_before() {
        let (_store, catalog) = catalog_of(store_files());
        let state = state_holding(
            &catalog,
            implicit(),
            &["bench-power", "reading-list"],
            Some(START),
        );

        let short = compute_needs(
            &catalog,
            &state,
            Some(START + REMINDER - 1),
            &PreviousTexts::new(),
        );
        assert!(
            short.stale.is_empty(),
            "999 tokens on, everything delivered is still within the model's reach, got {:?}",
            short.stale
        );

        let reached = compute_needs(
            &catalog,
            &state,
            Some(START + REMINDER),
            &PreviousTexts::new(),
        );
        assert_eq!(
            reached.stale,
            vec![MemoryId::new("bench-power"), MemoryId::new("reading-list")],
            "at the threshold every form delivered is owed again"
        );
    }

    /// Detects a reminder applied when no store asked for one: a store with no
    /// settings file must never repeat what it has already delivered, however
    /// far the context has grown, because nobody asked for the context to be
    /// spent that way.
    #[test]
    fn nothing_goes_stale_when_the_store_asks_for_no_reminder() {
        let (_store, catalog) = catalog_of(store_files_with_settings(""));
        let state = state_holding(&catalog, implicit(), &["bench-power"], Some(START));

        let needs = compute_needs(
            &catalog,
            &state,
            Some(START + 100 * REMINDER),
            &PreviousTexts::new(),
        );

        assert!(
            needs.stale.is_empty(),
            "with no reminder asked for, nothing may be repeated, got {:?}",
            needs.stale
        );
    }

    /// Detects an index line owed although the store asked for knowledge to be
    /// fetched on demand: those lines are what the setting exists to keep out
    /// of the context, while the critical memories must be unaffected.
    #[test]
    fn no_knowledge_memory_is_owed_when_the_store_turns_the_index_off() {
        let (_store, catalog) = catalog_of(store_files_with_settings(
            "deliver_knowledge_index: false\n",
        ));
        let state = ContextState::fresh(implicit(), None, chrono::Utc::now());

        let needs = compute_needs(&catalog, &state, Some(START), &PreviousTexts::new());

        assert_eq!(
            needs.new,
            vec![MemoryId::new("bench-power")],
            "only the memories delivered in full may be owed"
        );
    }

    /// Detects a change judged by the amount of text rather than by which lines
    /// it holds: a body that only lost lines says nothing the context has not
    /// been given, and one line rewritten leaves the body the same length while
    /// saying something new. Source: issue 14.
    #[test]
    fn a_version_that_only_lost_lines_is_shrunk_while_a_reworded_line_is_changed() {
        let store = TestStore::with(store_files());
        let catalog = store.catalog();
        let id = MemoryId::new("bench-power");
        let state = state_holding(&catalog, implicit(), &["bench-power"], Some(START));
        let previous = PreviousTexts::from([(id.clone(), THREE_RULES.trim_end().to_string())]);

        store.commit(vec![memory_file(
            "bench-power",
            "critical",
            &["global"],
            "Cut bench power at the wall before rewiring",
            TWO_RULES,
        )]);
        let shorter = compute_needs(&store.catalog(), &state, Some(START), &previous);
        assert_eq!(
            shorter.shrunk,
            vec![id.clone()],
            "every line of the new text was already given, so nothing is owed"
        );
        assert!(
            shorter.changed.is_empty(),
            "a shrink must not also count as a change, got {:?}",
            shorter.changed
        );

        store.commit(vec![memory_file(
            "bench-power",
            "critical",
            &["global"],
            "Cut bench power at the wall before rewiring",
            THREE_RULES_REWORDED,
        )]);
        let reworded = compute_needs(&store.catalog(), &state, Some(START), &previous);
        assert_eq!(
            reworded.changed,
            vec![id],
            "a reworded line is text the context has not been given"
        );
    }

    /// Detects a skipped version left recorded as unseen, which would deliver
    /// it at the next event anyway or silence the next real change: nothing is
    /// sent for a shrink, and the new version is still what the context holds.
    /// Source: issue 14.
    #[test]
    fn a_shrunk_version_is_recorded_as_seen_so_only_a_later_change_is_owed() {
        let store = TestStore::with(store_files());
        let id = MemoryId::new("bench-power");
        let mut state = state_holding(&store.catalog(), implicit(), &["bench-power"], Some(START));
        let previous = PreviousTexts::from([(id.clone(), THREE_RULES.trim_end().to_string())]);

        store.commit(vec![memory_file(
            "bench-power",
            "critical",
            &["global"],
            "Cut bench power at the wall before rewiring",
            TWO_RULES,
        )]);
        let catalog = store.catalog();
        let needs = compute_needs(&catalog, &state, Some(START), &previous);
        record_delivery(&mut state, &needs, &catalog, Some(START));

        assert!(
            compute_needs(&catalog, &state, Some(START), &PreviousTexts::new()).is_empty(),
            "the shrunk version is what the context holds, so nothing is owed at the next event"
        );

        store.commit(vec![memory_file(
            "bench-power",
            "critical",
            &["global"],
            "Cut bench power at the wall before rewiring",
            &format!("{TWO_RULES}Keep the meter on the bench.\n"),
        )]);
        let grown = compute_needs(&store.catalog(), &state, Some(START), &PreviousTexts::new());
        assert_eq!(
            grown.changed,
            vec![id],
            "a later version that adds a line is still owed"
        );
    }

    /// Detects a knowledge memory judged on its body: a context is given only
    /// the description, so a body with lines removed shrinks nothing that was
    /// ever delivered and the new version is still owed as an index line.
    /// Source: issue 14.
    #[test]
    fn a_knowledge_memory_whose_body_lost_lines_is_still_owed_because_only_its_description_was_given()
     {
        let store = TestStore::with(store_files());
        let id = MemoryId::new("reading-list");
        let state = state_holding(&store.catalog(), implicit(), &["reading-list"], Some(START));
        let previous = PreviousTexts::from([(id.clone(), BINDER.to_string())]);

        store.commit(vec![memory_file(
            "reading-list",
            "knowledge",
            &["global"],
            BINDER,
            "# The binder\n",
        )]);
        let needs = compute_needs(&store.catalog(), &state, Some(START), &previous);

        assert_eq!(
            needs.changed,
            vec![id],
            "the index line is what was delivered, and its version has moved"
        );
        assert!(
            needs.shrunk.is_empty(),
            "nothing the context was given lost a line, got {:?}",
            needs.shrunk
        );
    }

    /// Detects a call stopped for a memory the model may merely look up, and
    /// one stopped by a reminder of a rule it already holds: the interrupt is
    /// for a rule the context has not been given at this version, because the
    /// model would otherwise act under a rule it has never read.
    #[test]
    fn only_a_new_or_changed_critical_memory_counts_as_a_critical_arrival() {
        let (_store, catalog) = catalog_of(store_files());
        let rule = MemoryId::new("bench-power");
        let reference = MemoryId::new("reading-list");

        for needs in [
            Needs {
                new: vec![rule.clone()],
                ..Needs::default()
            },
            Needs {
                changed: vec![rule.clone()],
                ..Needs::default()
            },
        ] {
            assert!(
                needs.has_critical_arrival(&catalog),
                "a critical memory the context has not been given must stop the call, got {needs:?}"
            );
        }

        for needs in [
            Needs {
                new: vec![reference],
                ..Needs::default()
            },
            Needs {
                stale: vec![rule],
                ..Needs::default()
            },
        ] {
            assert!(
                !needs.has_critical_arrival(&catalog),
                "nothing the model has not read arrives here, so the call runs, got {needs:?}"
            );
        }
    }

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

    /// Detects a shrink test that accepts text the context has not been given:
    /// a reworded line, a reordering, or a line added. Source: the rule that a
    /// delivered text shrinks only when every one of its lines already appeared,
    /// in order, in the text the context holds (issue 14).
    #[test]
    fn only_a_text_whose_lines_all_appeared_in_order_before_counts_as_shrunk() {
        let old = "first line\nsecond line\nthird line";
        assert!(
            shrinks(old, "first line\nthird line"),
            "a text with one line removed is the old text with lines taken out"
        );
        assert!(
            !shrinks(old, "first line\nsecond line rewritten\nthird line"),
            "a reworded line is text the context has not been given"
        );
        assert!(
            !shrinks(old, old),
            "an unchanged text has not shrunk; there is nothing to record"
        );
        assert!(
            !shrinks(old, "third line\nfirst line"),
            "the same lines in another order are not the old text with lines taken out"
        );
        assert!(
            !shrinks(old, "first line\nsecond line\nthird line\nfourth line"),
            "a line added is text the context has not been given"
        );
    }

    /// Detects a shrink test that mishandles an empty text at either end: a
    /// memory whose delivered text was emptied has had everything removed, and a
    /// memory that had no delivered text before has only gained text.
    #[test]
    fn emptying_a_text_shrinks_it_and_filling_an_empty_one_does_not() {
        assert!(
            shrinks("first line\nsecond line", ""),
            "a text emptied has had every line removed"
        );
        assert!(
            !shrinks("", "first line"),
            "a text that was empty has only gained a line"
        );
        assert!(!shrinks("", ""), "two empty texts are the same text");
    }

    /// Detects a shrink test that reads the newline at the end of a text as a
    /// line of its own: a memory written back with or without one would be
    /// recorded as having shrunk and never delivered again.
    #[test]
    fn a_trailing_newline_alone_is_not_a_shrink() {
        assert!(!shrinks(
            "first line\nsecond line\n",
            "first line\nsecond line"
        ));
        assert!(!shrinks(
            "first line\nsecond line",
            "first line\nsecond line\n"
        ));
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
