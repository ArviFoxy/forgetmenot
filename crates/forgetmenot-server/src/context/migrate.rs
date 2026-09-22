//! Bringing a state file written by an earlier build up to the shape this one
//! reads.
//!
//! A snapshot carries the version of the shape it was written in, and what is
//! here is the chain from an earlier version to [`CURRENT_VERSION`]: one named
//! function per step, each repairing what the version before it left behind,
//! applied in order by [`ContextRegistry::load_from`]. A file written before
//! the version field existed carries none, which reads as version 0.
//!
//! A step works from the records alone. Nothing here reads the store, the
//! transcripts or the clock: by the time a snapshot is loaded the file is all
//! there is.
//!
//! [`ContextRegistry::load_from`]: super::registry::ContextRegistry::load_from

use super::registry::RegistrySnapshot;

/// The shape this build reads, and the version it writes on every snapshot.
pub const CURRENT_VERSION: u32 = 1;

/// One step of the chain: the version it reads, and the repair that turns a
/// snapshot of that version into one of the next.
struct Step {
    from: u32,
    repair: fn(&mut RegistrySnapshot),
}

/// The chain in order, each step reading the version the step before it wrote.
const STEPS: &[Step] = &[Step {
    from: 0,
    repair: give_every_subagent_the_context_it_inherited_from,
}];

/// Bring `snapshot` up to [`CURRENT_VERSION`] by applying every step from the
/// version it carries.
///
/// A snapshot already at the current version is left as it is, and so is one
/// from a version this build does not know: there is no step that reads it, and
/// the records it holds are read as they stand.
pub fn to_current(snapshot: &mut RegistrySnapshot) {
    for step in STEPS {
        if snapshot.version == step.from {
            (step.repair)(snapshot);
            snapshot.version = step.from + 1;
        }
    }
}

/// Version 0 to 1: a subagent recorded without a parent takes the main context
/// of its own session.
///
/// A subagent's parent is the context it inherited its scopes and its session
/// directory from, which version 1 records as the context that spawned it. In
/// version 0 that was the session in every case, so a record left without one
/// has the session for its answer, and the record's own key names that session.
fn give_every_subagent_the_context_it_inherited_from(snapshot: &mut RegistrySnapshot) {
    for record in &mut snapshot.contexts {
        if record.key.is_subagent() && record.state.parent.is_none() {
            record.state.parent = Some(record.key.session_context());
        }
    }
}
