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

use std::collections::{BTreeMap, BTreeSet};

use regex::RegexSet;

use crate::store::ScopeId;
use crate::store::scope::TriggerField;

/// One compiled trigger and where it came from.
#[derive(Clone, Debug)]
struct CompiledTrigger {
    scope: ScopeId,
    /// Present only for `working_directory` and `any` triggers, where a path
    /// means different things on different machines.
    machine: Option<String>,
    pattern: String,
}

/// A trigger that matched.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TriggerHit {
    pub scope: ScopeId,
    pub field: TriggerField,
    /// The pattern text, so that the trigger test page and the statistics can
    /// name which trigger fired.
    pub pattern: String,
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
    /// The transitive `implies` closure, applied by [`TriggerIndex::fire_closed`].
    implied: BTreeMap<ScopeId, BTreeSet<ScopeId>>,
}

impl TriggerIndex {
    /// An index with no triggers.
    pub fn empty() -> Self {
        Self {
            fields: std::array::from_fn(|_| FieldIndex::empty()),
            every: FieldIndex::empty(),
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
            })
            .collect()
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
        }
    }

    /// Add one trigger, or return the compile error of its pattern.
    ///
    /// A trigger on `any` is added to every field, so that the cost of matching
    /// one text stays the one pass over it.
    pub fn push(
        &mut self,
        scope: ScopeId,
        field: TriggerField,
        pattern: &str,
        machine: Option<String>,
    ) -> Result<(), regex::Error> {
        regex::Regex::new(pattern)?;
        let compiled = CompiledTrigger {
            scope,
            machine,
            pattern: pattern.to_string(),
        };
        match field.index() {
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
            implied,
        })
    }
}

/// One automaton over the patterns of `entries`, which keeps its parallel order.
fn compile(entries: Vec<CompiledTrigger>) -> Result<FieldIndex, regex::Error> {
    let set = RegexSet::new(entries.iter().map(|entry| &entry.pattern))?;
    Ok(FieldIndex { set, entries })
}
