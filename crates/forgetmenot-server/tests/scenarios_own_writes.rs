//! What a session's own writes do to the sessions around it, and to itself.
//!
//! A write through the MCP tools is a commit to a store every context reads, so
//! it has to arrive everywhere except where it came from: the session that wrote
//! it already has the text in its context, and delivering it back would spend
//! the context twice and read to the model as news. That is bug #11, and it is
//! only visible across two contexts and two events — the write, and the next
//! event of each. What one write does to the store is level 2, and what
//! `note_own_writes` does to one context's record is level 3; neither is
//! asserted again here.
//!
//! Every scenario has the same shape: a writer and another session, both started
//! and both holding everything the example store owes them, then one write, then
//! the next event of each.
//!
//! The writer's next event is the write's own `PostToolUse`, which is where a
//! write handed back to its author would arrive, so that is where each scenario
//! asks the writer for nothing; the prompt after it asks again, because a write
//! recorded at the wrong version would come back one event later as a change.

mod common;
mod scenario;

use scenario::{Session, Store, World};
use serde_json::json;

/// The machine these scenarios run on.
const ALPHA: &str = "alpha";

/// A directory no trigger of the example store fires on, so that every session
/// here works in the implicit scopes alone.
const NEUTRAL: &str = "/home/dev/notes";

/// The body every scenario that rewrites `bench-power` writes, so that a
/// delivery of it is unmistakably the new version.
const REWRITTEN_BENCH_POWER: &str = "# Cut bench power before rewiring\n\n\
     The rule now also covers the charger bench, which feeds the same rail.\n";

/// Two started sessions on one machine: the one that will write, and one that
/// will not.
///
/// Both are given everything the store owes them at their start, so that
/// anything either of them is delivered afterwards came from the write.
fn two_sessions(world: &World) -> (Session<'_>, Session<'_>) {
    let claude = world.claude(ALPHA);
    let writer = claude.session();
    let other = claude.session();
    writer.start_in(NEUTRAL).assert(|answer| {
        assert!(
            answer.delivers_full("bench-power") && answer.delivers_index("reading-list"),
            "the writer must hold the store's memories before it changes any of them"
        );
    });
    other.start_in(NEUTRAL).assert(|answer| {
        assert!(
            answer.delivers_full("bench-power") && answer.delivers_index("reading-list"),
            "the other session must hold them too, so a later delivery is the change"
        );
    });
    (writer, other)
}

/// Rewrite `bench-power` through `memory_put`, as the writing session, and
/// require that the call's own `PostToolUse` hands nothing back.
fn rewrite_bench_power(writer: &Session<'_>) {
    writer
        .mcp()
        .memory_put("bench-power", |memory| {
            memory.critical();
            memory.scopes(["global"]);
            memory.source("user");
            memory.description("Cut bench power at the wall and confirm the rail with the meter");
            memory.body(REWRITTEN_BENCH_POWER);
        })
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the event right after the write must not hand the write back"
            );
        });
}

/// Detects the write that started #11: a memory written through the tools and
/// delivered straight back to the session that wrote it, which reads to the
/// model as a rule it has not seen and spends the context on text it just sent.
///
/// The write is a commit, so every other context meets the new version at its
/// next event; the writer's own context is recorded as holding what it wrote, so
/// its next event carries nothing at all.
#[test]
fn memory_put_by_a_session_is_delivered_to_others_and_not_back() {
    let world = World::new().store(Store::example()).build();
    let (writer, other) = two_sessions(&world);

    rewrite_bench_power(&writer);

    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the session that wrote the memory already holds it"
        );
    });
    other.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "a write is in force in every session, and this one holds the old version"
        );
    });
}

/// Detects an own-write record kept for the tool that sends a whole document and
/// not for the one that edits a fragment: the session that made a one-line edit
/// would be sent the whole memory back.
///
/// The edit changes the memory's blob like any other write, so the other session
/// meets it as a change; the writer is recorded as holding the version its own
/// edit produced.
#[test]
fn memory_replace_text_by_a_session_is_delivered_to_others_and_not_back() {
    let world = World::new().store(Store::example()).build();
    let (writer, other) = two_sessions(&world);

    writer
        .mcp()
        .memory_replace_text("bench-power", "reads zero", "reads zero on both ranges")
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the event right after the edit must not hand the edit back"
            );
        });

    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the session that made the edit already holds the text it produced"
        );
    });
    other.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the other session holds the version before the edit"
        );
    });
}

/// Detects an own-write record kept only for the writes that touch a body: a
/// description is what a knowledge memory is delivered as, so changing one is a
/// delivery to every other session and must not be one here.
///
/// `reading-list` is a knowledge memory, so what every session was given is its
/// index line. The new description makes that line different text, which the
/// other session is owed and the writer is not.
#[test]
fn memory_set_fields_changing_the_description_is_delivered_to_others_and_not_back() {
    let world = World::new().store(Store::example()).build();
    let (writer, other) = two_sessions(&world);

    writer
        .mcp()
        .memory_set_fields(
            "reading-list",
            json!({ "description": "The workshop references are in the binder on the bench" }),
        )
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the event right after the change must not hand the new index line back"
            );
        });

    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the session that wrote the description already holds it"
        );
    });
    other.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_index("reading-list"),
            "the index line is what a knowledge memory is delivered as, and it changed"
        );
    });
}

/// Detects a rename recorded under one of its two ids: the writer would be told
/// its own memory was deleted, or would be delivered the memory it had just
/// moved, both from a change it made itself.
///
/// A rename is a deletion and a creation in one commit. The other session holds
/// the old id and is owed both halves: the new id arrives as a memory it does
/// not hold, and the old one is withdrawn as deleted, which is what the store
/// now says about it. The writer made the move and is owed neither half.
#[test]
fn memory_rename_by_a_session_reaches_others_under_the_new_id_and_the_writer_under_neither() {
    let world = World::new().store(Store::example()).build();
    let (writer, other) = two_sessions(&world);

    writer
        .mcp()
        .memory_rename("reading-list", "binder-notes")
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the event right after the move carries neither the new id nor the old"
            );
        });

    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the session that moved the memory is owed neither the new id nor the old"
        );
    });
    other.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_index("binder-notes"),
            "the other session does not hold the new id, so it arrives"
        );
        assert_eq!(
            answer.retraction_reason("reading-list"),
            Some("deleted"),
            "the id the other session holds is no longer in the store"
        );
    });
}

/// Detects a deletion withdrawn from the session that made it: the model would
/// be told a memory it had just deleted is gone, which is noise, while a session
/// that is still acting on the memory has to be told.
///
/// The other session was given the memory and has nothing in its context saying
/// it was retired, so the withdrawal is owed there and names the deletion. The
/// writer's record simply loses the entry, and its next event carries nothing.
#[test]
fn memory_delete_by_a_session_is_retracted_for_others_and_silently_for_the_writer() {
    let world = World::new().store(Store::example()).build();
    let (writer, other) = two_sessions(&world);

    writer
        .mcp()
        .memory_delete("reading-list")
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the event right after the deletion must not withdraw it from its deleter"
            );
        });

    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the session that deleted the memory is not told it is gone"
        );
    });
    other.prompt("what next").assert(|answer| {
        assert_eq!(
            answer.retraction_reason("reading-list"),
            Some("deleted"),
            "a session still acting on the memory has to be told it left the store"
        );
    });
}

/// Detects a branch whose writes are delivered before they land, and a land that
/// is delivered back to the session that made it: the first would put work in
/// front of every session while it is still being drafted, the second is #11
/// again at the moment a branch becomes real.
///
/// Nothing on a branch is in any context's store, so the other session's event
/// between the writes and the land carries nothing. The land is one commit
/// carrying both memories, so both arrive at the other session's next event,
/// each in the form its kind gets. The session that landed the branch is
/// recorded as holding everything the land carried, so its own next event is
/// empty — which is the only place a branch's writes are ever noted for it, since
/// a write on a branch notes nothing.
#[test]
fn a_landed_branch_is_delivered_to_others_and_not_to_the_session_that_landed_it() {
    let world = World::new().store(Store::example()).build();
    let (writer, other) = two_sessions(&world);

    let mcp = writer.mcp();
    let branch = mcp.branch();
    branch
        .memory_put("bench-power", |memory| {
            memory.critical();
            memory.scopes(["global"]);
            memory.source("user");
            memory.description("Cut bench power at the wall and confirm the rail with the meter");
            memory.body(REWRITTEN_BENCH_POWER);
        })
        .expect_ok();
    branch
        .memory_put("reading-list", |memory| {
            memory.knowledge();
            memory.scopes(["global"]);
            memory.source("assistant");
            memory.description("The workshop references are in the binder on the bench");
            memory.body("# Where the workshop references live\n\nAll of it is on paper.\n");
        })
        .expect_ok();

    other.prompt("anything new").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "a branch is invisible to every session until it lands"
        );
    });

    branch
        .land("the bench rework")
        .expect_ok()
        .assert_post(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the event right after the land must not hand the landed work back"
            );
        });

    other.prompt("anything new now").assert(|answer| {
        assert!(
            answer.delivers_full("bench-power"),
            "the land is one commit, and the critical memory it carried is due in full"
        );
        assert!(
            answer.delivers_index("reading-list"),
            "the knowledge memory it carried is due as its index line"
        );
    });
    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the session that landed the branch wrote every line of it"
        );
    });
}

/// Detects a branch whose writes reach the store, or the delivery record, before
/// anyone decided to keep them: work thrown away would go on being in force
/// somewhere, and the session that abandoned it would be recorded as holding
/// text that exists nowhere.
///
/// Abandoning leaves the store exactly as it was, so neither session is owed
/// anything: the other session holds the version that is still there, and the
/// writer holds it too.
#[test]
fn an_abandoned_branch_delivers_nothing_to_anyone() {
    let world = World::new().store(Store::example()).build();
    let (writer, other) = two_sessions(&world);

    let mcp = writer.mcp();
    let branch = mcp.branch();
    branch
        .memory_put("bench-power", |memory| {
            memory.critical();
            memory.scopes(["global"]);
            memory.source("user");
            memory.description("Cut bench power at the wall and confirm the rail with the meter");
            memory.body(REWRITTEN_BENCH_POWER);
        })
        .expect_ok();
    branch.abandon().expect_ok().assert_post(|answer| {
        assert!(
            answer.delivers_nothing(),
            "abandoning changes the store not at all, so nothing is due anywhere"
        );
    });

    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the store is as it was, and the writer holds what is in it"
        );
    });
    other.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "work that was thrown away must not be in force anywhere"
        );
    });
}

/// Detects an own-write record read for the whole session rather than for the
/// context that made the write: a subagent shares its parent's session id, so a
/// record kept per session would hide the parent's write from every child it
/// spawns, and the child would act on the version the store no longer holds.
///
/// A child created after the write copies the parent's scopes and starts with an
/// empty record, so the memory is due to it at its start and arrives in the
/// version the store now holds. A child created before the write was given the
/// old version, so it meets the change at its own next event; that event is a
/// `PreToolUse` and the memory is critical, so the call is stopped and carries
/// the new body.
#[test]
fn a_write_by_the_session_still_reaches_its_own_subagent() {
    let world = World::new().store(Store::example()).build();
    let (writer, _other) = two_sessions(&world);
    let before = writer.subagent("general-purpose", "list the bench files");

    rewrite_bench_power(&writer);

    let after = writer.subagent("general-purpose", "list the wiring files");
    assert!(
        after.start_answer().delivers_full("bench-power"),
        "a child started after the write holds no record of it, so it arrives at its start"
    );

    before
        .tool("Grep", json!({ "pattern": "pub fn" }))
        .assert(|answer| {
            assert!(
                answer.delivers_full("bench-power"),
                "a child started before the write was given the old version and is owed the new"
            );
        });
    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "what the children were given is not the writer's, and it wrote the memory itself"
        );
    });
}

/// Detects an own-write record that silences the whole context instead of the
/// memory it wrote: a session that wrote one memory would stop hearing about
/// every other, and an edit made from a shell would never reach it.
///
/// The writer is recorded as holding what it wrote, and nothing else. An edit
/// committed behind the server is nobody's write, so the writer meets it at its
/// next event like any other context, as the changed index line of a knowledge
/// memory, while the memory it wrote itself stays quiet.
#[test]
fn an_edit_outside_the_server_still_reaches_the_writer() {
    let world = World::new().store(Store::example()).build();
    let (writer, _other) = two_sessions(&world);

    rewrite_bench_power(&writer);
    world.rewrite_memory(
        "reading-list",
        "# Where the workshop references live\n\n\
         The binder moved to the shelf above the bench.\n",
    );

    writer.prompt("what next").assert(|answer| {
        assert!(
            answer.delivers_index("reading-list"),
            "an edit nobody in this session made is due here like anywhere else"
        );
        assert!(
            !answer.delivers_full("bench-power"),
            "the memory the session wrote itself is still not sent back to it"
        );
    });
}
