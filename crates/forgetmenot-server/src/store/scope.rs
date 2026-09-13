//! Scope files: what a scope implies and which regexes turn it on.

use serde::{Deserialize, Serialize};

use super::ScopeId;
use super::frontmatter::FrontmatterError;

/// The string a trigger regex is matched against.
///
/// [`TriggerField::Any`] is not one of the texts a hook event carries: it
/// stands for all of them at once, and is what a trigger means when its file
/// does not say which field it is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerField {
    UserMessage,
    AssistantMessage,
    ToolName,
    ToolInput,
    ToolResult,
    WorkingDirectory,
    Any,
}

impl TriggerField {
    /// Every text a hook event carries, in declaration order. `Any` is not
    /// among them, because it is every one of them rather than a text of its
    /// own.
    pub const ALL: [TriggerField; 6] = [
        TriggerField::UserMessage,
        TriggerField::AssistantMessage,
        TriggerField::ToolName,
        TriggerField::ToolInput,
        TriggerField::ToolResult,
        TriggerField::WorkingDirectory,
    ];

    /// The number of texts a hook event carries.
    pub const COUNT: usize = TriggerField::ALL.len();

    /// This field's position in [`TriggerField::ALL`], used to index the
    /// per-field regex sets, or `None` for `Any`, which has no position of its
    /// own because it belongs in every one of them.
    pub fn index(self) -> Option<usize> {
        TriggerField::ALL.iter().position(|field| *field == self)
    }

    /// The name used in scope files and in the API.
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerField::UserMessage => "user_message",
            TriggerField::AssistantMessage => "assistant_message",
            TriggerField::ToolName => "tool_name",
            TriggerField::ToolInput => "tool_input",
            TriggerField::ToolResult => "tool_result",
            TriggerField::WorkingDirectory => "working_directory",
            TriggerField::Any => "any",
        }
    }
}

impl std::fmt::Display for TriggerField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One regex that turns its scope on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    /// The string the pattern is matched against, or absent for
    /// [`TriggerField::Any`], which is every text the hook sees.
    ///
    /// An `Option` rather than a defaulted field, so that a file that says
    /// nothing about `on` is written back saying nothing about it, and one that
    /// spells `on: any` out keeps the line its author wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<TriggerField>,
    /// A regex in the syntax of the `regex` crate.
    pub pattern: String,
    /// Restricts the trigger to one machine. Valid with `on: working_directory`
    /// and with `any`, because a path means different things on different
    /// machines while a message does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

impl Trigger {
    /// The field this trigger matches on: `any` when the file does not say.
    pub fn field(&self) -> TriggerField {
        self.on.unwrap_or(TriggerField::Any)
    }

    /// Whether a machine qualifier means anything on this trigger's field: a
    /// path and "every text the hook sees" are machine-specific, a message is
    /// the same text everywhere.
    pub fn takes_machine_qualifier(&self) -> bool {
        matches!(
            self.field(),
            TriggerField::WorkingDirectory | TriggerField::Any
        )
    }
}

/// A parsed scope file.
///
/// A scope is a flag identified by its id. Keys the format no longer defines,
/// such as the `type` label older stores wrote, are ignored on parse and are
/// not written back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeDocument {
    /// Must equal the file stem.
    pub id: ScopeId,
    /// Scopes that are on whenever this one is.
    #[serde(default)]
    pub implies: Vec<ScopeId>,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
}

impl ScopeDocument {
    /// Parse a scope file.
    pub fn parse(bytes: &[u8]) -> Result<Self, FrontmatterError> {
        let text = std::str::from_utf8(bytes).map_err(|_| FrontmatterError::NotUtf8)?;
        Ok(yaml_serde::from_str(text)?)
    }

    /// Render the scope back to file text.
    pub fn render(&self) -> Result<String, FrontmatterError> {
        Ok(yaml_serde::to_string(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects an index derived from the enum that does not line up with
    /// `ALL`, which would route a trigger to the wrong field's regex set and
    /// fire it on the wrong text.
    #[test]
    fn every_field_indexes_its_own_position() {
        for (position, field) in TriggerField::ALL.iter().enumerate() {
            assert_eq!(
                field.index(),
                Some(position),
                "field {field} indexes the wrong slot"
            );
        }
        assert_eq!(
            TriggerField::Any.index(),
            None,
            "any is every field at once, so it must not claim one field's slot"
        );
    }

    /// Detects a rename that breaks the file format: the names in scope files
    /// are snake_case and a change to them would silently stop parsing stores
    /// people already wrote.
    #[test]
    fn field_names_are_the_snake_case_names_used_in_files() {
        let trigger: Trigger =
            yaml_serde::from_str("on: working_directory\npattern: /x\nmachine: alpha\n").unwrap();
        assert_eq!(trigger.field(), TriggerField::WorkingDirectory);
        assert_eq!(trigger.machine.as_deref(), Some("alpha"));
        assert!(
            yaml_serde::to_string(&trigger)
                .unwrap()
                .contains("working_directory")
        );
    }

    /// Detects two failures of the default field. A trigger whose file says
    /// nothing about `on` must match every text the hook sees, so reading the
    /// omission as a concrete field, or refusing the file, would leave the
    /// trigger firing on a fraction of what it was written for. And rendering
    /// the omission as `on: any` would add a line to every such file the moment
    /// the scope is saved, so a write that changed a pattern would show up as a
    /// rewrite of the whole trigger.
    #[test]
    fn a_trigger_that_omits_on_means_any_and_is_written_back_without_it() {
        let text = "id: widgets\ntriggers:\n- pattern: '\\bwidgets?\\b'\n";
        let scope = ScopeDocument::parse(text.as_bytes()).expect("a trigger may omit `on`");

        assert_eq!(
            scope.triggers[0].field(),
            TriggerField::Any,
            "a trigger without `on` must match every text the hook sees"
        );

        let rendered = scope.render().expect("the scope renders");
        assert!(
            !rendered.contains("on:"),
            "an omitted `on` must not be written back, got {rendered:?}"
        );
        assert_eq!(
            ScopeDocument::parse(rendered.as_bytes()).expect("the rendered scope parses"),
            scope,
            "rendering and parsing again must give the same scope, got {rendered:?}"
        );
    }

    /// Detects `implies` or `triggers` being required, which would reject the
    /// common scope file that only names a scope.
    #[test]
    fn implies_and_triggers_default_to_empty() {
        let scope = ScopeDocument::parse(b"id: widgets\n").unwrap();
        assert!(scope.implies.is_empty());
        assert!(scope.triggers.is_empty());
    }
}
