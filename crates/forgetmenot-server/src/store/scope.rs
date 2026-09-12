//! Scope files: what a scope implies and which regexes turn it on.

use serde::{Deserialize, Serialize};

use super::ScopeId;
use super::frontmatter::FrontmatterError;

/// The label a scope carries. It has no effect on delivery; it exists so that
/// a person reading the index knows what kind of thing the scope stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeType {
    Global,
    Machine,
    Session,
    Project,
    Domain,
    Directory,
}

/// The string a trigger regex is matched against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerField {
    UserMessage,
    AssistantMessage,
    ToolName,
    ToolInput,
    ToolResult,
    WorkingDirectory,
}

impl TriggerField {
    /// Every field, in declaration order.
    pub const ALL: [TriggerField; 6] = [
        TriggerField::UserMessage,
        TriggerField::AssistantMessage,
        TriggerField::ToolName,
        TriggerField::ToolInput,
        TriggerField::ToolResult,
        TriggerField::WorkingDirectory,
    ];

    /// The number of fields.
    pub const COUNT: usize = TriggerField::ALL.len();

    /// This field's position in [`TriggerField::ALL`], used to index the
    /// per-field regex sets.
    pub fn index(self) -> usize {
        self as usize
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
    /// The string the pattern is matched against.
    pub on: TriggerField,
    /// A regex in the syntax of the `regex` crate.
    pub pattern: String,
    /// Restricts the trigger to one machine. Valid only with
    /// `on: working_directory`, because a path means different things on
    /// different machines while a message does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

/// A parsed scope file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeDocument {
    /// Must equal the file stem.
    pub id: ScopeId,
    #[serde(rename = "type")]
    pub scope_type: ScopeType,
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
                position,
                "field {field} indexes the wrong slot"
            );
        }
    }

    /// Detects a rename that breaks the file format: the names in scope files
    /// are snake_case and a change to them would silently stop parsing stores
    /// people already wrote.
    #[test]
    fn field_names_are_the_snake_case_names_used_in_files() {
        let trigger: Trigger =
            yaml_serde::from_str("on: working_directory\npattern: /x\nmachine: alpha\n").unwrap();
        assert_eq!(trigger.on, TriggerField::WorkingDirectory);
        assert_eq!(trigger.machine.as_deref(), Some("alpha"));
        assert!(
            yaml_serde::to_string(&trigger)
                .unwrap()
                .contains("working_directory")
        );
    }

    /// Detects `implies` or `triggers` being required, which would reject the
    /// common scope file that only labels a scope.
    #[test]
    fn implies_and_triggers_default_to_empty() {
        let scope = ScopeDocument::parse(b"id: widgets\ntype: project\n").unwrap();
        assert!(scope.implies.is_empty());
        assert!(scope.triggers.is_empty());
    }
}
