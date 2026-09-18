//! What the memory system's own traffic does to the session it runs in.
//!
//! Every MCP call is three events: the `PreToolUse` that may stop it, the call,
//! and the `PostToolUse` carrying what it answered. The inputs and answers of
//! this server's own tools are full of scope ids, memory ids and memory bodies,
//! so a session that asks the memory system a question would, without the
//! exemption, turn scopes on from the question and from the answer to it — and
//! would undo with the answer what the call itself had just done. None of that
//! is visible in one event: it takes the call and the event after it. That the
//! exemption keeps a tool's texts out of `EventPlan::texts` is `hook::events`'s
//! own unit test; what these assert is where the session ends up.

mod common;
mod scenario;

use scenario::{Store, World, bash, read};
use serde_json::json;

/// The machine these scenarios run on.
const ALPHA: &str = "alpha";

/// A directory no trigger of the example store fires on, so that a scenario's
/// scopes are the ones it asked for.
const NEUTRAL: &str = "/home/dev/notes";

/// Detects the memory system undoing its own work: the call that turns scopes
/// off names them in its input, and its answer lists them again among the scopes
/// on offer — and the `PostToolUse` carrying that answer arrives after the scopes
/// are off. Matching either text would turn them straight back on, and the model
/// would be told it had stepped out of a scope and be working in it again by the
/// next event, with no way out of it at all.
///
/// The prompt names a widget, so `widgets` and the `rocketry` it implies are on.
/// Both are turned off in one call, so both are on offer in its answer, and
/// `rocketry`'s trigger is on every field, the result of a tool call included.
/// After the whole wrapped call both are off and stay off, and the next call —
/// whose own input names nothing of the store — goes ahead carrying nothing,
/// because there is nothing left that this session does not hold.
#[test]
fn a_scope_turned_off_through_mcp_stays_off_through_the_answer_that_names_it() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    session
        .prompt("rename the widget brackets")
        .assert(|answer| assert!(answer.delivers_full("widget-naming")));
    assert!(
        session.active_scopes().contains("widgets") && session.active_scopes().contains("rocketry"),
        "the prompt turned the scope and what it implies on, got {:?}",
        session.active_scopes()
    );

    session
        .mcp()
        .session_scope_off(["widgets", "rocketry"])
        .expect_ok();

    let after = session.active_scopes();
    assert!(
        !after.contains("widgets") && !after.contains("rocketry"),
        "neither the call naming the scopes nor the answer listing them may turn them on \
         again, got {after:?}"
    );
    session
        .tool("Bash", bash("cargo test -p bench"))
        .assert(|answer| {
            assert!(
                answer.allowed(),
                "nothing is due, so the call after the scope went off is not stopped"
            );
            assert!(
                answer.delivers_nothing(),
                "the scope is off and its memory was withdrawn at the call's own PostToolUse"
            );
        });
}

/// Detects an exemption applied to the tool's input but not to its answer, and
/// one applied to every tool rather than to this server's own: the first would
/// let a session that read one memory be pulled into every scope that memory's
/// text names, the second would stop the work itself from activating anything.
///
/// The same bytes travel twice. Read out of `memory_get`, they are the memory
/// system answering a question about itself, and no scope comes on. Handed back
/// as the result of a `Bash` call, they are what the session's work returned, and
/// `rocketry`, whose trigger is on every field and matches the text of a rocket,
/// comes on with its memory. The call is made without a session key, so the
/// fetch leaves no mark on the delivery record and the index line that arrives
/// afterwards is owed to the scope alone.
#[test]
fn a_memory_get_answer_naming_every_scope_activates_nothing_while_the_same_text_from_bash_does() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    let before = session.active_scopes();
    assert!(
        !before.contains("rocketry"),
        "the session starts outside rocketry, got {before:?}"
    );

    let fetched = session
        .mcp()
        .call("memory_get", json!({ "id": "rocket-stages" }))
        .expect_ok();
    let answered = fetched.text().to_string();
    assert!(
        answered.contains("rocket"),
        "the text this scenario is about has to name a rocket, got {answered}"
    );

    assert_eq!(
        session.active_scopes(),
        before,
        "asking the memory system about a memory must move no scope"
    );

    session
        .tool("Bash", bash("cat /home/dev/notes/log.txt"))
        .result(json!({ "stdout": answered }))
        .assert(|answer| {
            assert!(
                answer.delivers_index("rocket-stages"),
                "the same text from the work turns rocketry on, and its memory is due"
            );
        });
    assert!(
        session.active_scopes().contains("rocketry"),
        "the scope the tool result named is on, got {:?}",
        session.active_scopes()
    );
}

/// Detects a `trigger_exempt_tools` setting that is read but never reaches the
/// running server's event handling: a store that exempted a noisy tool would go
/// on being pulled into scopes by everything that tool printed, and nothing in
/// any single answer would say so.
///
/// One result, two tools. Through the tool the store exempts, the text is not
/// matched at all and no scope moves. Through a tool the store says nothing
/// about, the same text turns `rocketry` on and its memory arrives. Neither
/// call's input names a rocket, so the result is the only thing that could have
/// done it.
#[test]
fn a_tool_the_store_exempts_activates_nothing_from_its_result() {
    let world = World::new()
        .store(Store::example())
        .settings(|settings| {
            settings.trigger_exempt_tools(["Read"]);
        })
        .build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    let result = json!({ "stdout": "the rocket ships on friday" });

    session
        .tool("Read", read("/home/dev/notes/log.txt"))
        .result(result.clone())
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the store exempts this tool, so its result is matched against nothing"
            );
        });
    assert!(
        !session.active_scopes().contains("rocketry"),
        "no scope may come on from an exempt tool's result, got {:?}",
        session.active_scopes()
    );

    session
        .tool("Grep", json!({ "pattern": "pub fn" }))
        .result(result)
        .assert(|answer| {
            assert!(
                answer.delivers_index("rocket-stages"),
                "the same text from a tool the store says nothing about turns rocketry on"
            );
        });
    assert!(
        session.active_scopes().contains("rocketry"),
        "the exemption is for the tools the store named and no others, got {:?}",
        session.active_scopes()
    );
}

/// Detects a read of the session's scopes that is not a read: a context created
/// or touched by the listing would answer for a session that never asked
/// anything, and a listing that marked its own answer as delivered would leave
/// the session owed nothing it had not seen.
///
/// The session is in the three scopes every context has by construction and the
/// store's three file-backed scopes are all on offer, which is what the model
/// reads before choosing one. Nothing moves: the scopes after the call are the
/// scopes it reported, and the next prompt is delivered nothing, because
/// everything due arrived at the start and the listing changed neither the store
/// nor the record.
#[test]
fn a_session_scope_list_call_changes_nothing_and_answers_the_scopes() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    let before = session.active_scopes();

    let listed = session.mcp().session_scopes().expect_ok();
    let reported = |key: &str| -> std::collections::BTreeSet<String> {
        listed.json()[key]
            .as_array()
            .unwrap_or_else(|| panic!("the answer lists {key}, got {}", listed.json()))
            .iter()
            .map(|id| id.as_str().expect("a scope id is a string").to_string())
            .collect()
    };

    assert_eq!(
        reported("active"),
        before,
        "the tool must answer the scopes the server has this context working in"
    );
    assert_eq!(
        reported("available"),
        ["rocketry", "widgets", "workshop"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "every file-backed scope this session is not in is on offer"
    );
    assert_eq!(
        session.active_scopes(),
        before,
        "a listing changes no scope of the session that asked for it"
    );

    session
        .prompt("carry on with the bench supply")
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "everything due arrived at the start, and asking for the scopes moved nothing"
            );
        });
}

/// How many long critical rules the store carries beyond the example store's
/// own, and how many lines each one has. Five of this size put the session
/// start past the 10 000 characters Claude Code shows in full, so the answer is
/// saved to a file.
const LONG_RULES: usize = 5;
const LONG_LINES: usize = 34;

/// The example store with five long critical rules in `global`, each naming a
/// rocket.
///
/// `rocketry`'s trigger has no field, so it fires on any text: the bodies name
/// a rocket and the session start lists `rocketry` itself among the scopes on
/// offer, which is what makes the answer, read back, able to turn it on.
fn store_with_long_rocket_rules() -> scenario::StoreBuilder {
    let mut store = Store::example();
    for index in 0..LONG_RULES {
        let id = format!("long-rule-{index}");
        let body: String = (0..LONG_LINES)
            .map(|line| {
                format!(
                    "{id} line {line:02}: keep the rocket on its pad until the rail reads zero \
                     on the meter."
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        store = store.memory(&id, |memory| {
            memory
                .critical()
                .scopes(["global"])
                .description("A long launch rule")
                .body(&body);
        });
    }
    store
}

/// `text` as the `Read` tool prints a file: every line behind its number, right
/// aligned in six columns and followed by a tab.
fn numbered(text: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(index, line)| format!("{:>6}\t{line}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` without the line that marks it as this server's answer.
fn without_the_marker(text: &str) -> String {
    text.lines()
        .filter(|line| !line.contains("[forgetmenot] context "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Detects the server's own answer turning scopes on when it comes back through
/// a tool. A session start past 10 000 characters is saved to a file, and the
/// notice at the top tells the model to read that file; the whole answer then
/// arrives as the result of a `Read` — every critical body it delivered and the
/// list of every scope on offer — and matching it against the tool-result
/// triggers activates every scope the answer names at once, from the store's own
/// traffic rather than from the work. Same class as issue 17.
///
/// `rocketry`'s trigger has no field, so it fires on any text, and the answer
/// names a rocket twice over: in the bodies of the five long rules and as the
/// id `rocketry` in the scopes on offer. The store's threshold is set below the
/// answer's length as well, so the answer opens with the notice that sends the
/// model to the file.
///
/// The same numbered text handed back as a `Bash` result moves nothing either:
/// the marker line decides, not the tool. With that line taken out, the very
/// same bytes turn `rocketry` on and its memory arrives, which is what says the
/// marker is what silenced the read and not the text or the tool.
#[test]
fn the_persisted_answer_read_back_through_the_read_tool_activates_nothing() {
    let world = World::new()
        .store(store_with_long_rocket_rules())
        .settings(|settings| {
            settings.answer_file_threshold(Some(400));
        })
        .build();
    let session = world.claude(ALPHA).session();

    session.start_in(NEUTRAL);
    let before = session.active_scopes();
    assert!(
        !before.contains("rocketry"),
        "the session starts outside rocketry, got {before:?}"
    );

    let saved = session.persisted_files();
    assert_eq!(
        saved.len(),
        1,
        "the start answer is long enough for Claude Code to save it to one file, got {saved:?}"
    );
    let path = &saved[0];
    let answer = std::fs::read_to_string(path).expect("the saved answer is readable");
    assert!(
        answer.contains("rocket") && answer.contains("rocketry"),
        "the answer this scenario is about names a rocket and offers rocketry, got {answer}"
    );
    let printed = numbered(&answer);

    session
        .tool("Read", read(path.to_str().expect("a utf-8 path")))
        .assert(|answer| assert!(answer.allowed(), "nothing is due, so the read goes ahead"))
        .result(json!({ "content": printed.clone() }))
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the server's own answer read back is the store's traffic, not the work's"
            );
        });
    assert_eq!(
        session.active_scopes(),
        before,
        "no scope may come on from the answer this session was given"
    );

    session
        .tool("Bash", bash("cat the-saved-answer.txt"))
        .result(json!({ "stdout": printed.clone() }))
        .assert(|answer| {
            assert!(
                answer.delivers_nothing(),
                "the marker decides and not the tool, so the same text through Bash is silent too"
            );
        });
    assert_eq!(
        session.active_scopes(),
        before,
        "the same text through another tool must move no scope either"
    );

    session
        .tool("Bash", bash("cat the-launch-log.txt"))
        .result(json!({ "stdout": without_the_marker(&printed) }))
        .assert(|answer| {
            assert!(
                answer.delivers_index("rocket-stages"),
                "without the marker line the very same bytes are the work's own output"
            );
        });
    assert!(
        session.active_scopes().contains("rocketry"),
        "the scope the text names is on, got {:?}",
        session.active_scopes()
    );
}
