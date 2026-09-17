//! Branches: several writes waiting to become one commit, and what the rest of
//! the LAN is given while they wait.
//!
//! A branch is the only way to change the store without changing it: the writes
//! are real commits on a real git branch, and every session goes on being
//! delivered what main says until the branch lands. That is a property of two
//! contexts over time and cannot be seen from the writer's own answers, which is
//! why it is here rather than beside the branch tools.

mod common;
mod scenario;

use scenario::{McpOutcome, Store, World};

/// The machine these scenarios run on.
const ALPHA: &str = "alpha";

/// A directory that matches no trigger of the example store, so that both
/// sessions work in the implicit scopes alone and every delivery is a global
/// one.
const NEUTRAL_DIRECTORY: &str = "/home/dev/notes";

/// The memory created on a branch: global and critical, so that every other
/// context is owed it in full at its next event.
const BRANCH_MEMORY: &str = "bench-lighting";
const BRANCH_MEMORY_BODY: &str = "# Light the bench before wiring\n\n\
     The wall switch in the corner lights the bench; the overhead lamp is on the \
     same rail as the supply.\n";

/// The edit made to `bench-power` on a branch, and the word it puts into the
/// body. The word is in nothing the example store holds, so a test can ask
/// whether the branch's text reached a session by asking for the word.
const OLD_TEXT: &str = "reads zero";
const BRANCH_TEXT: &str = "reads zero volts";
const BRANCH_WORD: &str = "volts";

/// The version of `bench-power` written to main behind the server's back, which
/// rewrites the whole body and so cannot be merged with an edit inside it.
const OUTSIDE_BENCH_POWER: &str = "# Cut bench power before rewiring\n\n\
     Throw the wall breaker for the bench, then check the rail with the meter and \
     wait for the charge to drain.\n";

/// Detects a branch whose writes are visible before it lands, and a landed
/// branch that reaches only the session that landed it.
///
/// The first would make a branch pointless: the reason to write on one is that
/// the half-finished state never binds anyone. The second would leave the LAN
/// split, with one session acting on a rule the others have never been given.
///
/// Both a change to a memory every context holds and a memory nobody has yet are
/// written, because they are delivered by different paths: one is a version that
/// moved, the other is a memory that is newly due. Before the land the other
/// session's prompt is owed neither; after it, both, in full, because they are
/// critical. The subagent is started after the land and holds nothing, so it is
/// given the landed text at its start; a context created from a catalog built
/// before the land would be given the old body instead.
#[test]
fn a_branch_is_invisible_until_it_lands_and_then_reaches_every_other_context() {
    let world = World::new().store(Store::example()).build();
    let claude = world.claude(ALPHA);
    let writer = claude.session_named("session-1");
    let other = claude.session_named("session-2");

    writer.start_in(NEUTRAL_DIRECTORY);
    other
        .start_in(NEUTRAL_DIRECTORY)
        .assert(|answer| assert!(answer.delivers_full("bench-power")));

    let mcp = writer.mcp();
    let branch = mcp.branch();
    branch
        .memory_replace_text("bench-power", OLD_TEXT, BRANCH_TEXT)
        .expect_ok();
    branch
        .memory_put(BRANCH_MEMORY, |memory| {
            memory
                .critical()
                .description("The wall switch in the corner lights the bench")
                .body(BRANCH_MEMORY_BODY);
        })
        .expect_ok();

    other.prompt("carry on where we left off").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the writes are on a branch, so main says what it always said"
        );
    });

    branch.land("light the bench and say what the meter reads");

    other.prompt("and now").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the landed edit moved the version this context holds, so the rule arrives again"
        );
        assert!(
            answer.delivers_full(BRANCH_MEMORY),
            "the memory created on the branch is newly due here"
        );
        assert!(
            answer.text_contains(BRANCH_WORD),
            "what arrives must be the text the branch carried"
        );
    });

    let child = other.subagent("general-purpose", "Check the bench wiring");
    assert!(
        child.start_answer().delivers_full("bench-power")
            && child.start_answer().delivers_full(BRANCH_MEMORY),
        "a context created after the land is given the landed store, not the one before it"
    );
}

/// Detects a conflicting land that is reported as a success: the branch's text
/// would silently replace a change made on main, or half of each would be
/// written, and the session that made the change on main would never be told.
///
/// The branch edits one sentence of `bench-power`; the store is rewritten behind
/// the server's back, which is what a person with a shell does and what another
/// machine's session looks like from here. The two changes cover the same lines,
/// so the land cannot merge them and must refuse, naming the file so that the
/// model can write the version it wants on the branch and land again.
///
/// The sequence is the one a session really meets, and it takes two calls: the
/// same change on main that makes the land conflict is a changed critical
/// memory the writer has not been given, so the land's own `PreToolUse` holds
/// it and delivers the rule, and only the land the model issues after reading
/// it reaches the tool and is refused.
///
/// What the other session is then given is the point: the store still holds the
/// version written on main, so the rewrite arrives in full and the branch's word
/// is nowhere in it. A land that wrote anything would show up here as the
/// branch's word arriving, or as the rewrite not arriving at all.
#[test]
fn a_conflicting_land_is_refused_and_delivers_nothing() {
    let world = World::new().store(Store::example()).build();
    let claude = world.claude(ALPHA);
    let writer = claude.session_named("session-1");
    let other = claude.session_named("session-2");

    writer.start_in(NEUTRAL_DIRECTORY);
    other
        .start_in(NEUTRAL_DIRECTORY)
        .assert(|answer| assert!(answer.delivers_full("bench-power")));

    let mcp = writer.mcp();
    let branch = mcp.branch();
    branch
        .memory_replace_text("bench-power", OLD_TEXT, BRANCH_TEXT)
        .expect_ok();

    world.rewrite_memory("bench-power", OUTSIDE_BENCH_POWER);

    // The change on main is what makes the land conflict, and it is also a
    // changed critical memory the writer has not seen, so the land's own
    // `PreToolUse` is the event that carries it and the call is held. The model
    // reads it and issues the land again, which is the call that reaches the
    // tool.
    let held = branch.land("say what the meter reads");
    let McpOutcome::Held(stopped) = &held else {
        panic!("the change on main must stop the land's own call before the tool runs")
    };
    assert!(
        stopped.delivers_full("bench-power"),
        "the held call must carry the version written on main, which is what stopped it"
    );

    let refused = branch.land_again();
    let message = refused
        .refusal()
        .unwrap_or_else(|| panic!("a land that conflicts must refuse, got {}", refused.text()))
        .to_string();
    assert!(
        message.contains("memories/bench-power.md"),
        "the refusal must name the file the two changes disagree about, got {message:?}"
    );
    assert_eq!(
        world.memory_body("bench-power"),
        OUTSIDE_BENCH_POWER,
        "a refused land writes nothing, so the store still holds what main said"
    );

    other.prompt("carry on where we left off").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the store holds the version written on main, and this context holds the older one"
        );
        assert!(
            answer.text_lacks(BRANCH_WORD),
            "the land wrote nothing, so no word of the branch may reach another session"
        );
    });
}
