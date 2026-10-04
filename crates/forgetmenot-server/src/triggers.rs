//! Compiled trigger patterns.
//!
//! Every pattern of one [`TriggerField`] is compiled into a single
//! [`regex::RegexSet`]. Matching a text therefore walks the text once through
//! one finite automaton: the `regex` crate does not backtrack, so the time is
//! linear in the length of the text, and adding patterns grows the automaton
//! rather than adding passes over the text. That is the property that lets a
//! hook match a 256 KiB tool result against every trigger in the store on the
//! hot path.
//!
//! A trigger on [`TriggerField::Any`] belongs to every field, so it is compiled
//! into every field's automaton: matching a text still costs the one pass,
//! whatever mixture of fields the store's triggers name. One further automaton
//! holds every trigger once, whatever field it names, which is what a trigger
//! test asks about when it asks about `any`.
//!
//! A set says which patterns matched and not where. Where a match lies is asked
//! of the one trigger that matched, through its own compiled [`Regex`], and only
//! for a hit that turns its scope on: see [`TriggerIndex::evidence`].

use std::collections::{BTreeMap, BTreeSet};

use regex::{Regex, RegexSet};
use serde::{Deserialize, Serialize};

use crate::store::ScopeId;
use crate::store::scope::{Trigger, TriggerField};

/// One compiled trigger and where it came from.
#[derive(Clone, Debug)]
struct CompiledTrigger {
    scope: ScopeId,
    /// The machine the trigger is restricted to, absent when it fires on every
    /// machine. A plain conjunct alongside the pattern, whatever field the
    /// trigger names.
    machine: Option<String>,
    pattern: String,
    /// This trigger's position in [`TriggerIndex::sources`].
    source: usize,
}

/// One trigger as its scope file writes it, with its pattern compiled on its
/// own.
#[derive(Clone, Debug)]
struct TriggerSource {
    trigger: Trigger,
    regex: Regex,
}

/// A trigger that matched.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TriggerHit {
    pub scope: ScopeId,
    pub field: TriggerField,
    /// The pattern text, so that the trigger test page and the statistics can
    /// name which trigger fired.
    pub pattern: String,
    /// The trigger's position in the index that reported this hit, which is
    /// what [`TriggerIndex::evidence`] reads it back by.
    source: usize,
}

/// What a trigger found in the text it matched, in the shape its kind of
/// trigger defines, tagged with that kind.
///
/// A kind that locates its match carries `start` and `end`, counted in
/// characters (Unicode scalar values) of the text that was matched, so that the
/// matched part can be read back out of a stored text whatever the kind is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Evidence {
    /// A regex trigger: its pattern and the leftmost match of it.
    Regex {
        pattern: String,
        start: usize,
        end: usize,
    },
}

impl Evidence {
    /// The kind of trigger that found this, as the log names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Evidence::Regex { .. } => "regex",
        }
    }
}

/// The trigger behind a hit, as its scope file writes it, and what it found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerEvidence {
    pub trigger: Trigger,
    pub evidence: Evidence,
}

/// The patterns of one field.
struct FieldIndex {
    set: RegexSet,
    /// Parallel to the patterns in `set`: index *i* of a match is entry *i*.
    entries: Vec<CompiledTrigger>,
}

impl FieldIndex {
    fn empty() -> Self {
        Self {
            set: RegexSet::empty(),
            entries: Vec::new(),
        }
    }
}

/// Every trigger in the store, ready to match.
pub struct TriggerIndex {
    /// One automaton per text a hook event carries, each holding that field's
    /// own triggers and every `any` trigger.
    fields: [FieldIndex; TriggerField::COUNT],
    /// Every trigger once, whatever field it names: what `any` matches against
    /// when it is asked about as a field of its own.
    every: FieldIndex,
    /// Every trigger once, in the order of `every`, as written and compiled on
    /// its own.
    sources: Vec<TriggerSource>,
    /// The transitive `implies` closure, applied by [`TriggerIndex::fire_closed`].
    implied: BTreeMap<ScopeId, BTreeSet<ScopeId>>,
}

impl TriggerIndex {
    /// An index with no triggers.
    pub fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| FieldIndex::empty()),
            every: FieldIndex::empty(),
            sources: Vec::new(),
            implied: BTreeMap::new(),
        }
    }

    /// The number of compiled triggers.
    ///
    /// Counted over the one automaton that holds each trigger once, since an
    /// `any` trigger sits in every field's automaton as well.
    pub fn len(&self) -> usize {
        self.every.entries.len()
    }

    /// Whether the store has no triggers at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The triggers on `field` whose pattern matches `text` and whose machine
    /// qualifier is absent or equal to `machine`.
    ///
    /// A trigger on `any` fires for every field. Asking about `any` itself asks
    /// about every trigger in the store, which is what the trigger test page
    /// offers when a person does not want to pick a field.
    pub fn fire(&self, field: TriggerField, text: &str, machine: &str) -> Vec<TriggerHit> {
        let index = match field.index() {
            Some(position) => &self.fields[position],
            None => &self.every,
        };
        index
            .set
            .matches(text)
            .into_iter()
            .map(|position| &index.entries[position])
            .filter(|entry| entry.machine.as_deref().is_none_or(|name| name == machine))
            .map(|entry| TriggerHit {
                scope: entry.scope.clone(),
                field,
                pattern: entry.pattern.clone(),
                source: entry.source,
            })
            .collect()
    }

    /// The trigger behind `hit` and where its pattern matches `text`, the text
    /// the hit was reported for.
    ///
    /// The set that reported the hit matched with the same pattern, so the
    /// pattern matches here as well; the match located is the leftmost one,
    /// which is the one `Regex::find` reports. Run once per hit that turns a
    /// scope on, never for the hits that find the scope already on.
    pub fn evidence(&self, hit: &TriggerHit, text: &str) -> TriggerEvidence {
        let source = &self.sources[hit.source];
        let found = source
            .regex
            .find(text)
            .expect("a pattern the set matched in this text matches it on its own");
        let start = text[..found.start()].chars().count();
        let end = start + found.as_str().chars().count();
        TriggerEvidence {
            trigger: source.trigger.clone(),
            evidence: Evidence::Regex {
                pattern: source.trigger.pattern.clone(),
                start,
                end,
            },
        }
    }

    /// The scopes [`TriggerIndex::fire`] activates, closed under `implies`.
    pub fn fire_closed(&self, field: TriggerField, text: &str, machine: &str) -> BTreeSet<ScopeId> {
        let mut activated = BTreeSet::new();
        for hit in self.fire(field, text, machine) {
            activated.insert(hit.scope.clone());
            if let Some(implied) = self.implied.get(&hit.scope) {
                activated.extend(implied.iter().cloned());
            }
        }
        activated
    }
}

/// Collects triggers and compiles them.
///
/// Patterns are compiled one at a time so that a broken pattern can be
/// reported with its own scope, field and error text; a `RegexSet` built from
/// all of them at once would only say that something in the set was invalid.
pub struct TriggerIndexBuilder {
    /// One slot per field, so that a field can never index a missing slot.
    per_field: [Vec<CompiledTrigger>; TriggerField::COUNT],
    /// Every trigger once, in the order it was pushed.
    every: Vec<CompiledTrigger>,
    /// Parallel to `every`.
    sources: Vec<TriggerSource>,
}

impl Default for TriggerIndexBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TriggerIndexBuilder {
    pub fn new() -> Self {
        Self {
            per_field: std::array::from_fn(|_| Vec::new()),
            every: Vec::new(),
            sources: Vec::new(),
        }
    }

    /// Add one trigger of `scope`, as its file writes it, or return the compile
    /// error of its pattern.
    ///
    /// A trigger on `any` is added to every field, so that the cost of matching
    /// one text stays the one pass over it.
    pub fn push(&mut self, scope: ScopeId, trigger: &Trigger) -> Result<(), regex::Error> {
        let regex = Regex::new(&trigger.pattern)?;
        let compiled = CompiledTrigger {
            scope,
            machine: trigger.machine.clone(),
            pattern: trigger.pattern.clone(),
            source: self.sources.len(),
        };
        self.sources.push(TriggerSource {
            trigger: trigger.clone(),
            regex,
        });
        match trigger.field().index() {
            Some(position) => self.per_field[position].push(compiled.clone()),
            None => {
                for slot in &mut self.per_field {
                    slot.push(compiled.clone());
                }
            }
        }
        self.every.push(compiled);
        Ok(())
    }

    /// Compile the collected triggers into one automaton per field, and one
    /// holding all of them.
    pub fn build(
        self,
        implied: BTreeMap<ScopeId, BTreeSet<ScopeId>>,
    ) -> Result<TriggerIndex, regex::Error> {
        let mut fields: [FieldIndex; TriggerField::COUNT] =
            std::array::from_fn(|_| FieldIndex::empty());
        for (position, entries) in self.per_field.into_iter().enumerate() {
            fields[position] = compile(entries)?;
        }
        Ok(TriggerIndex {
            fields,
            every: compile(self.every)?,
            sources: self.sources,
            implied,
        })
    }
}

/// One automaton over the patterns of `entries`, which keeps its parallel order.
fn compile(entries: Vec<CompiledTrigger>) -> Result<FieldIndex, regex::Error> {
    let set = RegexSet::new(entries.iter().map(|entry| &entry.pattern))?;
    Ok(FieldIndex { set, entries })
}
