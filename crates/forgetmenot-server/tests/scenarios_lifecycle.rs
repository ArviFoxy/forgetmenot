//! What a whole session is given, from its start to its last prompt.
//!
//! Each test here is about the sequence and not about any one event: that every
//! memory arrives once in the form its kind gets and never again; that what the
//! server records as delivered is not always what the model could read; that a
//! session picked up again is given everything afresh; that the assistant's own
//! words are matched against triggers like anything else. The rules a single
//! event shows (which form a kind is delivered in, when the notice is written,
//! when a call is held) are tested where those events are.

mod common;
mod scenario;

use std::collections::BTreeMap;

use scenario::{Answer, Store, World, bash};
use serde_json::json;

/// The machine these scenarios run on, which is the machine the example store's
/// session memory belongs to.
const ALPHA: &str = "alpha";

/// A directory that turns nothing on: it names no rocket and is not a workshop,
/// so a session started here works in the implicit scopes alone.
const QUIET: &str = "/home/dev/parts";

/// The example store's critical memories, whose bodies arrive whole.
const CRITICAL: [&str; 2] = ["bench-power", "widget-naming"];

/// The example store's knowledge memories, which arrive as index lines.
const KNOWLEDGE: [&str; 3] = [
    "reading-list",
    "rocket-stages",
    "sessions/alpha/session-1/notes",
];

/// How many answers of a session carried each memory, in each form.
#[derive(Default)]
struct Arrivals {
    full: BTreeMap<&'static str, usize>,
    index: BTreeMap<&'static str, usize>,
}

impl Arrivals {
    /// Count what this answer carried. Called for every answer of the session,
    /// so the totals are over the whole session and not over a chosen few.
    fn record(&mut self, answer: &Answer<'_>) {
        for id in CRITICAL {
            if answer.delivers_full(id) {
                *self.full.entry(id).or_default() += 1;
            }
        }
        for id in KNOWLEDGE {
            if answer.delivers_index(id) {
                *self.index.entry(id).or_default() += 1;
            }
        }
    }
}

/// Detects a session that repeats a memory it has already given, one that gives
/// a memory in both forms, and one that never gives a memory at all: each is
/// invisible in any single answer and shows only when the whole session is
/// counted.
///
/// The session starts in a directory that names a rocket, so `rocketry` is on
/// from the first event and everything global, session and rocketry is owed at
/// the start: `bench-power` whole, and `reading-list`, `rocket-stages` and the
/// session notes as index lines.
///
/// The prompt says `widgets`, and `widgets`'s user-message pattern is
/// `\bwidget\b`, which the plural does not match, so the prompt turns nothing on
/// and is owed nothing. The `Bash` call's input says `widgets` too, and the
/// tool-input pattern is `\bwidgets?\b`, which does match: `widgets` comes on
/// there, `widget-naming` is a critical memory new to this context, and a
/// `PreToolUse` is the one event that may stop a call, so exactly one hold
/// happens in the whole session. The call issued again is owed nothing and goes
/// ahead; the result names a rocket and the assistant's last words name rocket
/// stages, but `rocketry` has been on since the start and `rocket-stages` has
/// been given, so neither is owed anything. The last prompt is owed nothing
/// because everything due has arrived and nothing has changed.
///
/// Every answer is small enough for Claude Code to show whole, so what the
/// server sent is what the model holds, which is what `model_saw_full` reads.
#[test]
fn every_memory_arrives_once_in_its_form_across_a_whole_session() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();
    let mut arrivals = Arrivals::default();

    arrivals.record(&session.start_in("/home/dev/rocketry"));
    arrivals.record(&session.prompt("renumber the widgets in the parts list"));

    let held = session
        .tool("Bash", bash("grep -rn widgets ./parts-list"))
        .assert(|answer| {
            assert!(
                answer.held(),
                "the tool input turns widgets on and its critical memory is new here"
            );
        });
    arrivals.record(held.answer());
    let reissued = held.reissue().assert(|answer| {
        assert!(
            answer.allowed(),
            "the rule has been given, so the call issued again goes ahead"
        );
    });
    arrivals.record(reissued.answer());
    arrivals.record(&reissued.result(json!({
        "stdout": "parts-list/rocket-frame.csv:12: widgets, upper bracket"
    })));
    arrivals.record(&session.says("the rocket stages keep the numbers they shipped with"));

    let last = session.prompt("anything else on the bench today?");
    assert!(
        last.delivers_nothing(),
        "everything due has arrived and nothing changed, so the last prompt is owed nothing"
    );
    arrivals.record(&last);

    for id in CRITICAL {
        assert_eq!(
            arrivals.full.get(id).copied().unwrap_or(0),
            1,
            "{id} must arrive whole in exactly one answer of the session"
        );
    }
    for id in KNOWLEDGE {
        assert_eq!(
            arrivals.index.get(id).copied().unwrap_or(0),
            1,
            "{id} must arrive as an index line in exactly one answer of the session"
        );
    }
    assert!(
        session.model_saw_full("bench-power"),
        "the rule the session opened with must be in the text the model can read"
    );
    assert!(
        session.model_saw_full("widget-naming"),
        "the rule the held call carried must be in the text the model can read"
    );
}

/// How many long critical memories the store carries beyond the example store's
/// own, and how long each one is. Five of this size put the session start well
/// past the 10 000 characters Claude Code shows in full.
const LONG_MEMORIES: usize = 5;
const LONG_LINES: usize = 34;

/// The ids of those memories, which sort after `bench-power`.
fn long_ids() -> Vec<String> {
    (0..LONG_MEMORIES)
        .map(|index| format!("long-rule-{index}"))
        .collect()
}

/// A body of distinct lines, each naming its own memory, so that a text holding
/// one memory's body whole is never read as holding another's.
fn long_body(id: &str) -> String {
    (0..LONG_LINES)
        .map(|line| {
            format!(
                "{id} line {line:02}: confirm the rail reads zero on the meter before touching \
                 any wiring on this bench."
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The example store with five long critical memories in `global`.
fn store_with_long_rules() -> scenario::StoreBuilder {
    let mut store = Store::example();
    for id in long_ids() {
        let body = long_body(&id);
        store = store.memory(&id, |memory| {
            memory
                .critical()
                .scopes(["global"])
                .description("A long bench rule")
                .body(&body);
        });
    }
    store
}

/// Detects a long answer whose first words are a memory rather than the notice:
/// past 10 000 characters Claude Code saves the answer to a file and shows the
/// model a preview cut at about 2 000, so anything but a notice at the very top
/// leaves the model reading a fragment of a rule it does not know is a fragment.
///
/// Five memories of some three thousand characters each put the session start
/// past that length, so the answer opens with the notice and Claude Code saves
/// exactly one file: the start is not a `PreToolUse`, so there is no second
/// string to save beside the context.
///
/// The gap the notice exists to cover is asserted as it stands, not as it ought
/// to be: the server records every memory as delivered, and the model can read
/// only the ones inside the preview. That difference is issue 19, closed as not
/// planned; the notice is the mitigation and this is what it mitigates. A test
/// that asserted the model saw them all would be asserting the fix that was
/// declined.
///
/// With the store's threshold turned off no notice is written however long the
/// answer is, because the notice is the store's to ask for.
#[test]
fn a_long_answer_opens_with_the_notice_the_preview_keeps() {
    let world = World::new().store(store_with_long_rules()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET).assert(|answer| {
        assert!(
            answer.starts_with_notice(),
            "an answer Claude Code will cut has to open by saying so, got {} characters",
            answer.chars()
        );
    });

    assert_eq!(
        session.persisted_files().len(),
        1,
        "a session start answers with one string, so Claude Code saves one file"
    );

    let unreadable: Vec<String> = long_ids()
        .into_iter()
        .filter(|id| !session.model_saw_full(id))
        .collect();
    assert!(
        !unreadable.is_empty(),
        "the preview cannot hold every rule; this is the gap of issue 19 the notice covers"
    );
    assert_eq!(
        session.delivered_count(),
        LONG_MEMORIES + 3,
        "the server records all five long rules plus bench-power, reading-list and the notes \
         as delivered, whatever the model could read"
    );

    let quiet = World::new()
        .store(store_with_long_rules())
        .settings(|settings| {
            settings.answer_file_threshold(None);
        })
        .build();
    let unnoticed = quiet.claude(ALPHA).session();

    unnoticed.start_in(QUIET).assert(|answer| {
        assert!(
            !answer.starts_with_notice(),
            "a store that turns the threshold off asks for no notice at any length"
        );
        assert!(
            answer.delivers_full("bench-power"),
            "turning the notice off must not change what is delivered"
        );
    });
}

/// Detects a resume that is answered like an ordinary event, and one that starts
/// the session over: the conversation Claude Code rebuilds holds none of the
/// text the earlier run was given, so a resume answered with nothing leaves the
/// model working without its rules, while a resume that cleared the scopes would
/// drop the memories the session had turned on and never get them back.
///
/// The prompt names a widget in the singular, which is what `widgets`'s
/// user-message pattern matches, so the session is working in `widgets` and the
/// `rocketry` it implies when it is resumed. The resume is a `SessionStart`, so
/// the record of what was delivered begins afresh while the scopes stay, and
/// everything due for those scopes arrives again. The prompt after it is owed
/// nothing, because the resume gave it all.
#[test]
fn a_session_resumed_is_given_everything_again_and_keeps_its_scopes() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET);
    session
        .prompt("check the widget part numbers on the bracket")
        .assert(|answer| {
            assert!(
                answer.delivers_full("widget-naming"),
                "the prompt names a widget, so widgets comes on here"
            );
        });

    session.resume().assert(|answer| {
        assert!(
            answer.delivers_full("bench-power") && answer.delivers_full("widget-naming"),
            "the rebuilt conversation holds neither rule, so both arrive again"
        );
        assert!(
            answer.delivers_index("reading-list")
                && answer.delivers_index("rocket-stages")
                && answer.delivers_index("sessions/alpha/session-1/notes"),
            "every knowledge memory due arrives again as its index line"
        );
    });

    let scopes = session.active_scopes();
    assert!(
        scopes.contains("widgets") && scopes.contains("rocketry"),
        "a resumed session goes on working in the scopes it had, got {scopes:?}"
    );

    session.prompt("carry on from there").assert(|answer| {
        assert!(
            answer.delivers_nothing(),
            "the resume gave everything, so the next prompt is owed nothing"
        );
    });
}

/// Detects a `Stop` whose text is never matched: the assistant's last words are
/// the only thing in a turn that nothing else in the session carries, so a scope
/// the model talked itself into would never come on and its memories would
/// arrive at some later event or not at all.
///
/// The session starts in a directory naming no rocket, so `rocketry` is off and
/// `rocket-stages` is not owed at the start. The assistant then says that the
/// rocket stages are wrong; `rocketry`'s trigger names no field, which stands
/// for every text the hook sees, so it fires on the assistant's message. Its
/// only memory is knowledge, so it arrives as an index line.
#[test]
fn a_stop_answers_with_what_the_assistants_words_triggered() {
    let world = World::new().store(Store::example()).build();
    let session = world.claude(ALPHA).session();

    session.start_in(QUIET).assert(|answer| {
        assert!(
            !answer.delivers_index("rocket-stages"),
            "nothing has named a rocket yet, so rocketry is off"
        );
    });

    session
        .says("the rocket stages are wrong in the report")
        .assert(|answer| {
            assert!(
                answer.delivers_index("rocket-stages"),
                "the assistant's own words turn rocketry on and its knowledge memory arrives"
            );
        });

    assert!(
        session.active_scopes().contains("rocketry"),
        "the scope the assistant's words turned on stays on, got {:?}",
        session.active_scopes()
    );
}
