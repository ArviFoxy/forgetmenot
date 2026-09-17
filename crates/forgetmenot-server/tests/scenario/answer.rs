//! One hook answer, and what a scenario may ask of it.
//!
//! What counts as a delivery is read out of the rendered text without depending
//! on any section label: the labels are wording, and the contract is that a
//! critical memory's body arrives whole, a knowledge memory arrives as its id
//! and description, and a withdrawal names the id and why. Each is read against
//! the store as it stands at the moment of the check, so a scenario that edits a
//! memory and asks about it afterwards asks about the text it wrote.

use std::panic::AssertUnwindSafe;

use forgetmenot_server::store::MemoryId;
use serde_json::Value;

use crate::scenario::world::World;

/// The two reasons a delivery is withdrawn. Source: `context::RetractReason`,
/// whose texts the renderer writes into the retracted lines.
pub const RETRACT_REASONS: [&str; 2] = ["deleted", "no longer in an active scope"];

/// How the notice that opens an answer Claude Code will save to a file begins.
/// Source: `render::file_notice`, whose first sentence is fixed.
pub const FILE_NOTICE_OPENING: &str = "[forgetmenot] READ THE FILE FIRST.";

/// One answer, with the event that produced it.
pub struct Answer<'w> {
    world: &'w World,
    /// The hook payload the server was sent, named in every failure so that a
    /// scenario failure says which event went wrong.
    sent: Value,
    json: Value,
}

impl<'w> Answer<'w> {
    pub(crate) fn new(world: &'w World, sent: Value, json: Value) -> Self {
        Self { world, sent, json }
    }

    /// The text Claude Code is asked to inject, if any.
    pub fn text(&self) -> Option<&str> {
        self.json
            .get("hookSpecificOutput")?
            .get("additionalContext")?
            .as_str()
    }

    /// Whether the answer is the bare object that asks Claude Code for nothing.
    pub fn is_empty_object(&self) -> bool {
        self.json == serde_json::json!({})
    }

    /// Whether the tool call was stopped.
    pub fn held(&self) -> bool {
        self.decision() == Some("deny")
    }

    /// Whether the answer lets the call go ahead, which every answer that is
    /// not a deny does.
    pub fn allowed(&self) -> bool {
        !self.held()
    }

    /// Why the call was stopped, if it was.
    pub fn deny_reason(&self) -> Option<&str> {
        self.json
            .get("hookSpecificOutput")?
            .get("permissionDecisionReason")?
            .as_str()
    }

    /// Whether the memory's whole body arrived: every non-empty line of it
    /// appears as a line of the answer's text.
    pub fn delivers_full(&self, id: &str) -> bool {
        self.seen(id).full
    }

    /// Whether the memory was named with its description on one line, and its
    /// body did not arrive, which is what an index line is.
    pub fn delivers_index(&self, id: &str) -> bool {
        self.seen(id).index
    }

    /// Whether the answer withdraws the memory, for either reason.
    pub fn retracts(&self, id: &str) -> bool {
        self.seen(id).retracted.is_some()
    }

    /// Which of the two reasons the answer gives for withdrawing the memory.
    ///
    /// Separate from [`Answer::retracts`] because the two reasons are different
    /// things to tell a model: one says the rule is out of the store, the other
    /// says this session has stopped working under it.
    pub fn retraction_reason(&self, id: &str) -> Option<&'static str> {
        self.seen(id).retracted
    }

    /// Whether the answer names the scope as one that became active with
    /// nothing to deliver: a line of the text is the scope id and nothing else.
    ///
    /// A session start also lists the scopes the session could turn on, one id
    /// per line, so this asks a useful question only of the events that are not
    /// session starts.
    pub fn announces_scope(&self, id: &str) -> bool {
        self.lines().contains(&id)
    }

    /// Whether the answer carries no text for the model at all.
    pub fn delivers_nothing(&self) -> bool {
        self.text().unwrap_or_default().is_empty()
    }

    /// The length of the answer's text as Claude Code measures it, in UTF-16
    /// code units, which is what its own limit is compared against.
    pub fn chars(&self) -> usize {
        self.text()
            .map(|text| text.encode_utf16().count())
            .unwrap_or(0)
    }

    /// Whether the answer opens with the notice telling the model to read the
    /// file Claude Code saved the answer to.
    pub fn starts_with_notice(&self) -> bool {
        self.text()
            .is_some_and(|text| text.starts_with(FILE_NOTICE_OPENING))
    }

    /// Run `check` against this answer and give the answer back, so that
    /// several checks chain. A failure inside `check` carries the whole answer
    /// and the event it answered.
    pub fn assert(self, check: impl FnOnce(&Answer<'w>)) -> Self {
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| check(&self)));
        if let Err(panic) = outcome {
            let said = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_string()))
                .unwrap_or_else(|| "the check panicked".to_string());
            panic!(
                "{said}\n\
                 --- the event sent ---\n{}\n\
                 --- the answer ---\n{}\n\
                 --- the text the model was given ---\n{}",
                serde_json::to_string_pretty(&self.sent).unwrap_or_default(),
                serde_json::to_string_pretty(&self.json).unwrap_or_default(),
                self.text().unwrap_or("(nothing)")
            );
        }
        self
    }

    fn decision(&self) -> Option<&str> {
        self.json
            .get("hookSpecificOutput")?
            .get("permissionDecision")?
            .as_str()
    }

    fn lines(&self) -> Vec<&str> {
        self.text()
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .collect()
    }

    fn seen(&self, id: &str) -> Seen {
        let catalog = self.world.catalog();
        let memory = catalog.memory(&MemoryId::new(id));
        let description = memory
            .map(|entry| entry.document.description())
            .unwrap_or("");
        let body = memory
            .map(|entry| entry.document.body.as_str())
            .unwrap_or("");
        seen_in(&self.lines(), id, description, body)
    }
}

/// What a text says about one memory.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Seen {
    pub full: bool,
    pub index: bool,
    pub retracted: Option<&'static str>,
}

/// What `lines` say about a memory with this id, description and body.
///
/// A memory the store no longer holds has neither description nor body, and
/// only its withdrawal is left to read, which is what a deletion leaves behind.
pub(crate) fn seen_in(lines: &[&str], id: &str, description: &str, body: &str) -> Seen {
    let body_lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let full = !body_lines.is_empty() && body_lines.iter().all(|line| lines.contains(line));
    let index = !full
        && !description.is_empty()
        && lines
            .iter()
            .any(|line| line.contains(id) && line.contains(description));
    let retracted = RETRACT_REASONS.into_iter().find(|reason| {
        lines
            .iter()
            .any(|line| line.contains(id) && line.contains(reason))
    });
    Seen {
        full,
        index,
        retracted,
    }
}
