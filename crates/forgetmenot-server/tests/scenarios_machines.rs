//! One store, several machines: what reaches a session depends on the machine
//! its Claude Code runs on, and two machines never share a context.
//!
//! The machine is not in any hook event. It is the `--machine` flag of the hook
//! client, sent beside every event, and it decides which qualified triggers can
//! fire, which implicit scopes a context starts in, and which context an event
//! belongs to. None of that is visible from one machine's events, which is why
//! every test here runs two.

mod common;
mod scenario;

use scenario::{Field, Store, World, bash};
use serde_json::json;

/// The two machines. `alpha` is the one the example store's session memories and
/// its `workshop` trigger are written for; `beta` is any other machine on the
/// LAN reading the same store.
const ALPHA: &str = "alpha";
const BETA: &str = "beta";

/// A directory that matches no trigger of the example store, so that the scope a
/// scenario is about is the only one its sessions turn on.
const NEUTRAL_DIRECTORY: &str = "/home/dev/notes";

/// The scope whose trigger is qualified for one machine, and the critical memory
/// in it. The word is in no memory of the example store, so nothing else can
/// turn the scope on.
const LAB: &str = "lab";
const LAB_RULE: &str = "lab-rule";
const LAB_RULE_BODY: &str = "# The lab supply is shared\n\n\
     The lab bench supply feeds two benches, so it is switched at the wall and \
     never at the bench.\n";

/// A memory whose only scope is one machine's implicit scope, and which is
/// therefore never due anywhere else.
const BETA_ONLY: &str = "beta-disk-layout";
const BETA_ONLY_BODY: &str = "# Where the captures live on this machine\n\n\
     Captures are written to the second disk, which is mounted at /captures and \
     is not backed up.\n";

/// Detects a machine qualifier that is dropped once the trigger is matched
/// against a text rather than a directory: the qualifier would compile, the
/// scope would fire everywhere, and a session on another machine would be given
/// a rule written for a room it is not in.
///
/// The trigger is on every field, so the qualifier has to survive the path that
/// matches a user's message, not only the one that matches a shell directory
/// (which `scenarios_ported::two_machines_in_the_same_directory_end_up_in_different_scopes`
/// covers). Both sessions send the same words, so the machine is the only thing
/// that differs between them, and the memory arriving in one and not the other is
/// the whole answer.
#[test]
fn a_machine_qualified_trigger_activates_its_scope_on_that_machine_only() {
    let world = World::new()
        .store(
            Store::example()
                .scope(LAB, |scope| {
                    scope.trigger_on_machine(Field::Any, r"\blab\b", ALPHA);
                })
                .memory(LAB_RULE, |memory| {
                    memory
                        .critical()
                        .scopes([LAB])
                        .description("The lab bench supply is switched at the wall")
                        .body(LAB_RULE_BODY);
                }),
        )
        .build();
    let on_alpha = world.claude(ALPHA).session_named("session-1");
    let on_beta = world.claude(BETA).session_named("session-2");

    on_alpha.start_in(NEUTRAL_DIRECTORY);
    on_beta.start_in(NEUTRAL_DIRECTORY);

    on_alpha.prompt("we are in the lab today").assert(|answer| {
        assert!(
            answer.delivers_full(LAB_RULE),
            "the trigger is qualified for this machine, so the scope comes on here"
        );
    });
    on_beta.prompt("we are in the lab today").assert(|answer| {
        assert!(
            !answer.delivers_full(LAB_RULE),
            "the same words on another machine name a different place"
        );
    });
    assert!(
        !on_beta.active_scopes().contains(LAB),
        "the scope must not be on for the other machine, got {:?}",
        on_beta.active_scopes()
    );
}

/// Detects a machine scope treated as a label rather than as the machine's own:
/// every session on the LAN would be given one machine's notes, which are about
/// disks, mounts and hardware that the others do not have.
///
/// A machine's implicit scope is on in every context on that machine and can be
/// turned on nowhere else, so the memory is due at the first event of every
/// session on `beta` and at no event on `alpha`. The second session on `beta` is
/// what separates "the memory reaches this machine" from "the memory reaches the
/// first context that asked": it has a delivery record of its own and is owed the
/// memory in full like the first.
#[test]
fn a_machine_memory_reaches_only_that_machines_sessions() {
    let world = World::new()
        .store(Store::example().memory(BETA_ONLY, |memory| {
            memory
                .critical()
                .scopes([format!("machine:{BETA}")])
                .description("Captures live on the second disk, which is not backed up")
                .body(BETA_ONLY_BODY);
        }))
        .build();
    let on_alpha = world.claude(ALPHA).session_named("session-1");
    let beta = world.claude(BETA);

    on_alpha.start_in(NEUTRAL_DIRECTORY).assert(|answer| {
        assert!(
            !answer.delivers_full(BETA_ONLY),
            "the memory's only scope belongs to another machine, which nothing here can turn on"
        );
    });
    beta.session_named("session-2")
        .start_in(NEUTRAL_DIRECTORY)
        .assert(|answer| {
            assert!(
                answer.delivers_full(BETA_ONLY),
                "the machine's own scope is on in every context on it"
            );
        });
    beta.session_named("session-3")
        .start_in(NEUTRAL_DIRECTORY)
        .assert(|answer| {
            assert!(
                answer.delivers_full(BETA_ONLY),
                "a second session on the machine holds nothing yet, so it is owed the rule too"
            );
        });
}

/// Detects a context keyed by the session id alone: Claude Code numbers sessions
/// per machine, so two machines reach the same ids within a day of each other.
/// Two sessions sharing one context would share scopes, share a delivery record,
/// and each stop the other being delivered what it has not seen.
///
/// The prompt names a widget, which turns `widgets` on and the `rocketry` it
/// implies. If the two sessions were one context, those scopes would be on for
/// the session on `beta` as well, and `widget-naming` would already be recorded
/// as delivered there, so its own start would not have carried it. Both are
/// asked, because either alone can pass on a half-shared context.
#[test]
fn the_same_session_id_on_two_machines_is_two_contexts() {
    let world = World::new().build();
    let on_alpha = world.claude(ALPHA).session_named("session-1");
    let on_beta = world.claude(BETA).session_named("session-1");

    on_alpha.start_in(NEUTRAL_DIRECTORY);
    on_beta.start_in(NEUTRAL_DIRECTORY);

    on_alpha
        .prompt("rename the widget brackets")
        .assert(|answer| assert!(answer.delivers_full("widget-naming")));

    assert!(
        !on_beta.active_scopes().contains("widgets"),
        "the other machine's session said nothing about widgets, got {:?}",
        on_beta.active_scopes()
    );
    on_beta.tool("Bash", bash("cargo build")).assert(|answer| {
        assert!(
            !answer.delivers_full("widget-naming"),
            "the rule is not due here, and nothing the other machine was given is recorded here"
        );
    });

    let listed: Vec<serde_json::Value> = world
        .contexts()
        .iter()
        .map(|row| row["key"].clone())
        .collect();
    assert!(
        listed.contains(&json!(format!("{ALPHA}/session-1")))
            && listed.contains(&json!(format!("{BETA}/session-1"))),
        "the page must list one row per machine for the shared id, got {listed:?}"
    );
}
