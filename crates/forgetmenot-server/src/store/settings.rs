//! The store's behaviour settings: `config.yml` at the root of the repository.
//!
//! These settings change what an agent experiences: when a rule is repeated,
//! when a call is held, what a subagent starts with, what is delivered. They are
//! therefore versioned with the memories they act on rather than passed on a
//! command line: a commit that changes `config.yml` takes effect at the next
//! event in every context, and a branch carries a change to them like any other
//! file. Where the server listens, what it writes and how long it keeps things
//! stay on the command line, because none of that is part of the store.
//!
//! The file is a flat mapping of the keys in [`KEYS`]. A key the schema does not
//! define, and a value of the wrong type, are validation errors rather than
//! things to ignore: a misspelled setting must not read as "the default", since
//! writing it down was an instruction to behave differently.

use serde_json::Value as Json;
use yaml_serde::{Mapping, Value as Yaml};

/// The settings file, at the root of the store.
pub const SETTINGS_PATH: &str = "config.yml";

/// The bytes of a tool result matched against triggers unless the store says
/// otherwise. Matching is linear in the text, so a 10 MB file read must not cost
/// a 10 MB regex pass on the hot path.
pub const DEFAULT_TOOL_RESULT_MATCH_LIMIT: u64 = 256 * 1024;

/// The characters of a hook answer above which Claude Code writes the answer
/// to a file and shows the model a preview of it instead of the whole text.
/// Read from Claude Code 2.1.270 (`HEr = 1e4`, compared against the JavaScript
/// string length, so UTF-16 code units). A newer Claude Code may move it, which
/// is why the store can set it rather than the server fixing it.
pub const DEFAULT_ANSWER_FILE_THRESHOLD: u64 = 10_000;

/// Characters of delivered text per token unless the store says otherwise.
/// Anthropic's published figure for Claude.
pub const DEFAULT_CHARACTERS_PER_TOKEN: f64 = 3.5;

/// The smallest divisor a store may set. A token is at least a character, and a
/// divisor at or below zero would make every token figure infinite or negative.
pub const SMALLEST_CHARACTERS_PER_TOKEN: f64 = 0.1;

/// The type one setting takes, as the schema reports it and as a write is
/// checked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingType {
    /// A whole number of at least zero, or null to turn the behaviour off.
    IntegerOrNull,
    /// A whole number of at least zero.
    Integer,
    /// A number, whole or fractional, of at least
    /// [`SMALLEST_CHARACTERS_PER_TOKEN`].
    Number,
    Boolean,
    StringList,
}

impl SettingType {
    /// The name the schema and every refusal use.
    pub fn as_str(self) -> &'static str {
        match self {
            SettingType::IntegerOrNull => "integer or null",
            SettingType::Integer => "integer",
            SettingType::Number => "number",
            SettingType::Boolean => "bool",
            SettingType::StringList => "list of strings",
        }
    }
}

impl std::fmt::Display for SettingType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One key of the settings file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKey {
    ReminderTokens,
    InterruptOnCritical,
    InterruptExemptTools,
    TriggerExemptTools,
    SubagentsInheritScopes,
    DeliverKnowledgeIndex,
    ToolResultMatchLimit,
    AnswerFileThreshold,
    AnnounceEmptyScopes,
    CharactersPerToken,
}

/// Every key the settings file may carry, in the order the schema lists them.
pub const KEYS: [SettingKey; 10] = [
    SettingKey::ReminderTokens,
    SettingKey::InterruptOnCritical,
    SettingKey::InterruptExemptTools,
    SettingKey::TriggerExemptTools,
    SettingKey::SubagentsInheritScopes,
    SettingKey::DeliverKnowledgeIndex,
    SettingKey::ToolResultMatchLimit,
    SettingKey::AnswerFileThreshold,
    SettingKey::AnnounceEmptyScopes,
    SettingKey::CharactersPerToken,
];

impl SettingKey {
    /// The name this key has in the file, in the API and in the tools.
    pub fn as_str(self) -> &'static str {
        match self {
            SettingKey::ReminderTokens => "reminder_tokens",
            SettingKey::InterruptOnCritical => "interrupt_on_critical",
            SettingKey::InterruptExemptTools => "interrupt_exempt_tools",
            SettingKey::TriggerExemptTools => "trigger_exempt_tools",
            SettingKey::SubagentsInheritScopes => "subagents_inherit_scopes",
            SettingKey::DeliverKnowledgeIndex => "deliver_knowledge_index",
            SettingKey::ToolResultMatchLimit => "tool_result_match_limit",
            SettingKey::AnswerFileThreshold => "answer_file_threshold",
            SettingKey::AnnounceEmptyScopes => "announce_empty_scopes",
            SettingKey::CharactersPerToken => "characters_per_token",
        }
    }

    /// The key of this name, or `None` when the schema defines no such key.
    pub fn parse(name: &str) -> Option<Self> {
        KEYS.into_iter().find(|key| key.as_str() == name)
    }

    pub fn value_type(self) -> SettingType {
        match self {
            SettingKey::ReminderTokens | SettingKey::AnswerFileThreshold => {
                SettingType::IntegerOrNull
            }
            SettingKey::InterruptOnCritical
            | SettingKey::SubagentsInheritScopes
            | SettingKey::DeliverKnowledgeIndex
            | SettingKey::AnnounceEmptyScopes => SettingType::Boolean,
            SettingKey::InterruptExemptTools | SettingKey::TriggerExemptTools => {
                SettingType::StringList
            }
            SettingKey::ToolResultMatchLimit => SettingType::Integer,
            SettingKey::CharactersPerToken => SettingType::Number,
        }
    }

    /// What this key does, as the schema reports it to a person or a model
    /// choosing a value.
    pub fn description(self) -> &'static str {
        match self {
            SettingKey::ReminderTokens => {
                "Deliver everything that applies again after this many context tokens, critical \
                 memories in full and knowledge memories as their description; null turns \
                 reminders off"
            }
            SettingKey::InterruptOnCritical => {
                "Hold a tool call when a critical memory is due and this context has not seen it"
            }
            SettingKey::InterruptExemptTools => {
                "Tool names that are never held, matched against the tool name exactly"
            }
            SettingKey::TriggerExemptTools => {
                "Tool names whose inputs and results are never matched against triggers, matched \
                 against the tool name exactly; the store's own MCP tools are never matched \
                 whatever this says"
            }
            SettingKey::SubagentsInheritScopes => {
                "A subagent starts with its parent's active scopes rather than the implicit ones \
                 alone"
            }
            SettingKey::DeliverKnowledgeIndex => {
                "Deliver the description of each knowledge memory that applies, so the agent \
                 can ask for the full text; off, nothing is delivered about knowledge memories"
            }
            SettingKey::ToolResultMatchLimit => {
                "Bytes of a tool result matched against triggers; the rest is not matched"
            }
            SettingKey::AnswerFileThreshold => {
                "Characters of a hook answer above which Claude Code saves it to a file and \
                 shows the model a preview; an answer past this opens with a notice to read \
                 the file; null turns the notice off"
            }
            SettingKey::AnnounceEmptyScopes => {
                "Name a scope in the rendered context when it becomes active but delivers \
                 nothing; off, nothing is said about it"
            }
            SettingKey::CharactersPerToken => {
                "Characters of delivered text per token, for every token figure the server \
                 reports; 3.5 is Anthropic's published figure for Claude"
            }
        }
    }
}

impl std::fmt::Display for SettingKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The settings in force, which is the file's keys over the defaults.
///
/// Not `Eq`: `characters_per_token` is a fractional number, and two settings are
/// compared for equality only in tests, where the values are the ones the test
/// wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Context growth after which anything already delivered is delivered
    /// again, each in the form its kind gets; `None` turns reminders off.
    pub reminder_tokens: Option<u64>,
    /// Whether a tool call is held when a critical memory is due and unseen.
    pub interrupt_on_critical: bool,
    /// Tool names that are never held, by exact match on the tool name.
    pub interrupt_exempt_tools: Vec<String>,
    /// Tool names whose inputs and results are kept away from the triggers, by
    /// exact match on the tool name.
    pub trigger_exempt_tools: Vec<String>,
    /// Whether a subagent starts with its parent's active scopes.
    pub subagents_inherit_scopes: bool,
    /// Whether the descriptions of knowledge memories are delivered.
    pub deliver_knowledge_index: bool,
    /// Bytes of a tool result matched against triggers.
    pub tool_result_match_limit: u64,
    /// Characters of a hook answer above which Claude Code shows the model a
    /// preview and a file path instead of the answer, so the answer opens with
    /// a notice to read the file; `None` sends no notice.
    pub answer_file_threshold: Option<u64>,
    /// Whether a scope that becomes active with nothing to deliver is named to
    /// the agent.
    pub announce_empty_scopes: bool,
    /// Characters of delivered text per token, the divisor of every token figure
    /// the server reports.
    pub characters_per_token: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            reminder_tokens: None,
            interrupt_on_critical: true,
            interrupt_exempt_tools: Vec::new(),
            trigger_exempt_tools: Vec::new(),
            subagents_inherit_scopes: true,
            deliver_knowledge_index: true,
            tool_result_match_limit: DEFAULT_TOOL_RESULT_MATCH_LIMIT,
            answer_file_threshold: Some(DEFAULT_ANSWER_FILE_THRESHOLD),
            announce_empty_scopes: false,
            characters_per_token: DEFAULT_CHARACTERS_PER_TOKEN,
        }
    }
}

impl Settings {
    /// One setting's value, in the JSON form the API and the tools report.
    pub fn value(&self, key: SettingKey) -> Json {
        match key {
            SettingKey::ReminderTokens => match self.reminder_tokens {
                Some(tokens) => Json::from(tokens),
                None => Json::Null,
            },
            SettingKey::InterruptOnCritical => Json::from(self.interrupt_on_critical),
            SettingKey::InterruptExemptTools => Json::from(self.interrupt_exempt_tools.clone()),
            SettingKey::TriggerExemptTools => Json::from(self.trigger_exempt_tools.clone()),
            SettingKey::SubagentsInheritScopes => Json::from(self.subagents_inherit_scopes),
            SettingKey::DeliverKnowledgeIndex => Json::from(self.deliver_knowledge_index),
            SettingKey::ToolResultMatchLimit => Json::from(self.tool_result_match_limit),
            SettingKey::AnswerFileThreshold => match self.answer_file_threshold {
                Some(characters) => Json::from(characters),
                None => Json::Null,
            },
            SettingKey::AnnounceEmptyScopes => Json::from(self.announce_empty_scopes),
            SettingKey::CharactersPerToken => Json::from(self.characters_per_token),
        }
    }

    /// Every setting with the value in force, which is what `GET /api/settings`
    /// and `settings_get` report.
    pub fn as_json(&self) -> serde_json::Map<String, Json> {
        KEYS.into_iter()
            .map(|key| (key.as_str().to_string(), self.value(key)))
            .collect()
    }

    /// Set one key from the JSON form of its value, or report the type the key
    /// takes and what arrived instead.
    pub fn set_json(&mut self, key: SettingKey, value: &Json) -> Result<(), TypeMismatch> {
        let expected = key.value_type();
        let mismatch = || TypeMismatch {
            expected,
            found: describe_json(value),
        };
        match key {
            SettingKey::ReminderTokens => {
                self.reminder_tokens = match value {
                    Json::Null => None,
                    _ => Some(value.as_u64().ok_or_else(mismatch)?),
                };
            }
            SettingKey::InterruptOnCritical => {
                self.interrupt_on_critical = value.as_bool().ok_or_else(mismatch)?;
            }
            SettingKey::InterruptExemptTools => {
                self.interrupt_exempt_tools = strings(value).ok_or_else(mismatch)?;
            }
            SettingKey::TriggerExemptTools => {
                self.trigger_exempt_tools = strings(value).ok_or_else(mismatch)?;
            }
            SettingKey::SubagentsInheritScopes => {
                self.subagents_inherit_scopes = value.as_bool().ok_or_else(mismatch)?;
            }
            SettingKey::DeliverKnowledgeIndex => {
                self.deliver_knowledge_index = value.as_bool().ok_or_else(mismatch)?;
            }
            SettingKey::ToolResultMatchLimit => {
                self.tool_result_match_limit = value.as_u64().ok_or_else(mismatch)?;
            }
            SettingKey::AnswerFileThreshold => {
                self.answer_file_threshold = match value {
                    Json::Null => None,
                    _ => Some(value.as_u64().ok_or_else(mismatch)?),
                };
            }
            SettingKey::AnnounceEmptyScopes => {
                self.announce_empty_scopes = value.as_bool().ok_or_else(mismatch)?;
            }
            SettingKey::CharactersPerToken => {
                let characters = value.as_f64().ok_or_else(mismatch)?;
                if !characters.is_finite() || characters < SMALLEST_CHARACTERS_PER_TOKEN {
                    return Err(mismatch());
                }
                self.characters_per_token = characters;
            }
        }
        Ok(())
    }

    /// Whether a tool call may be held for a critical memory: the interrupt has
    /// to be on and the tool must not be one the store exempts. A call that
    /// names no tool is not a tool call and is never held.
    pub fn interrupts(&self, tool_name: Option<&str>) -> bool {
        let Some(tool_name) = tool_name else {
            return false;
        };
        self.interrupt_on_critical
            && !self
                .interrupt_exempt_tools
                .iter()
                .any(|exempt| exempt == tool_name)
    }

    /// The tool result cap as a byte count. A limit past what this machine can
    /// address caps at the whole text, which is what "no cap" means.
    pub fn tool_result_cap(&self) -> usize {
        usize::try_from(self.tool_result_match_limit).unwrap_or(usize::MAX)
    }
}

/// A value that is not of the type its key takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeMismatch {
    pub expected: SettingType,
    /// What arrived instead, named as a person reads it: `a string`, `a list`.
    pub found: String,
}

/// One thing wrong with a settings file, reported per key so that the rest of
/// the file still takes effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingProblem {
    /// A key the schema does not define. Not ignored: a misspelled key was
    /// meant to change something and silently does not.
    UnknownKey {
        key: String,
    },
    WrongType {
        key: String,
        mismatch: TypeMismatch,
    },
}

/// Why a settings file could not be read at all, as opposed to one key of it.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("file is not valid UTF-8")]
    NotUtf8,
    #[error("file is not valid YAML: {0}")]
    Yaml(#[from] yaml_serde::Error),
    #[error("file is not a mapping of settings but {0}")]
    NotAMapping(String),
}

/// The settings one revision of the store holds.
#[derive(Clone, Debug, Default)]
pub struct SettingsFile {
    /// The settings in force: the file's keys over the defaults.
    pub settings: Settings,
    /// The file's own entries, kept so that writing one key leaves the rest of
    /// the file as its author wrote it.
    pub entries: Mapping,
}

/// Read a settings file: the settings it puts in force, and one problem per key
/// it gets wrong.
///
/// A file with a bad key still yields the settings its good keys set, so that
/// one mistyped line does not take a store's whole behaviour back to the
/// defaults without anyone being told.
pub fn parse(bytes: &[u8]) -> Result<(SettingsFile, Vec<SettingProblem>), SettingsError> {
    let text = std::str::from_utf8(bytes).map_err(|_| SettingsError::NotUtf8)?;
    let document: Yaml = yaml_serde::from_str(text)?;
    let entries = match document {
        // An empty file, or one holding only comments, sets nothing.
        Yaml::Null => Mapping::new(),
        Yaml::Mapping(mapping) => mapping,
        other => {
            return Err(SettingsError::NotAMapping(
                describe_yaml(&other).to_string(),
            ));
        }
    };

    let mut settings = Settings::default();
    let mut problems = Vec::new();
    for (name, value) in &entries {
        let name = key_text(name);
        let Some(key) = SettingKey::parse(&name) else {
            problems.push(SettingProblem::UnknownKey { key: name });
            continue;
        };
        if let Err(mismatch) = settings.set_json(key, &json_of_yaml(value)) {
            problems.push(SettingProblem::WrongType {
                key: name,
                mismatch,
            });
        }
    }
    Ok((SettingsFile { settings, entries }, problems))
}

/// The file text for a set of entries, which is what a write commits.
pub fn render(entries: &Mapping) -> Result<String, SettingsError> {
    Ok(yaml_serde::to_string(entries)?)
}

/// The YAML form of a value the API or a tool sent, for the entry a write sets.
pub fn yaml_of_json(value: &Json) -> Yaml {
    match value {
        Json::Null => Yaml::Null,
        Json::Bool(value) => Yaml::Bool(*value),
        Json::Number(number) => match (number.as_u64(), number.as_i64(), number.as_f64()) {
            (Some(value), _, _) => Yaml::Number(value.into()),
            (_, Some(value), _) => Yaml::Number(value.into()),
            (_, _, Some(value)) => Yaml::Number(value.into()),
            _ => Yaml::Null,
        },
        Json::String(text) => Yaml::String(text.clone()),
        Json::Array(items) => Yaml::Sequence(items.iter().map(yaml_of_json).collect()),
        Json::Object(fields) => Yaml::Mapping(
            fields
                .iter()
                .map(|(name, value)| (Yaml::String(name.clone()), yaml_of_json(value)))
                .collect(),
        ),
    }
}

/// The JSON form of a value read from a file, so that a value from a file and a
/// value from a request are checked by the same code.
fn json_of_yaml(value: &Yaml) -> Json {
    match value {
        Yaml::Null => Json::Null,
        Yaml::Bool(value) => Json::from(*value),
        Yaml::Number(number) => match (number.as_u64(), number.as_i64(), number.as_f64()) {
            (Some(value), _, _) => Json::from(value),
            (_, Some(value), _) => Json::from(value),
            (_, _, Some(value)) => Json::from(value),
            _ => Json::Null,
        },
        Yaml::String(text) => Json::from(text.clone()),
        Yaml::Sequence(items) => Json::Array(items.iter().map(json_of_yaml).collect()),
        Yaml::Mapping(fields) => Json::Object(
            fields
                .iter()
                .map(|(name, value)| (key_text(name), json_of_yaml(value)))
                .collect(),
        ),
        // A `!Tag` is not one of the forms any setting takes; it is carried
        // through as its value so that the type check reports what is there.
        Yaml::Tagged(tagged) => json_of_yaml(&tagged.value),
    }
}

/// The text of a mapping key, which is the setting's name when the file is
/// written as settings files are.
fn key_text(key: &Yaml) -> String {
    match key.as_str() {
        Some(text) => text.to_string(),
        None => serde_json::to_string(&json_of_yaml(key)).unwrap_or_else(|_| "?".to_string()),
    }
}

/// The strings of a JSON list, or `None` when it is not a list of strings.
fn strings(value: &Json) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// What a value is, named as a refusal names it.
fn describe_json(value: &Json) -> String {
    match value {
        Json::Null => "null".to_string(),
        Json::Bool(_) => "a bool".to_string(),
        Json::Number(_) => "a number".to_string(),
        Json::String(_) => "a string".to_string(),
        Json::Array(_) => "a list".to_string(),
        Json::Object(_) => "a mapping".to_string(),
    }
}

fn describe_yaml(value: &Yaml) -> &'static str {
    match value {
        Yaml::Null => "null",
        Yaml::Bool(_) => "a bool",
        Yaml::Number(_) => "a number",
        Yaml::String(_) => "a string",
        Yaml::Sequence(_) => "a list",
        Yaml::Mapping(_) => "a mapping",
        Yaml::Tagged(_) => "a tagged value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a file whose keys do not reach the settings, and defaults that
    /// move: every key a store may carry has to take effect, and a key the file
    /// leaves out has to keep the documented default.
    #[test]
    fn every_key_of_a_file_takes_effect_and_the_rest_keep_their_defaults() {
        let (file, problems) =
            parse(b"reminder_tokens: 120000\ninterrupt_exempt_tools:\n- Read\n- Glob\n")
                .expect("a mapping of settings parses");

        assert_eq!(problems, Vec::new(), "the file is well formed");
        assert_eq!(file.settings.reminder_tokens, Some(120_000));
        assert_eq!(
            file.settings.interrupt_exempt_tools,
            vec!["Read".to_string(), "Glob".to_string()]
        );
        assert_eq!(
            file.settings.interrupt_on_critical,
            Settings::default().interrupt_on_critical,
            "a key the file does not carry must keep its default"
        );
    }

    /// Detects a missing file read as anything other than the defaults, which
    /// is the store that has never been configured.
    #[test]
    fn a_file_with_no_keys_is_the_defaults() {
        let (file, problems) = parse(b"# nothing set here\n").expect("an empty file parses");
        assert_eq!(problems, Vec::new());
        assert_eq!(file.settings, Settings::default());
        assert_eq!(
            Settings::default().reminder_tokens,
            None,
            "reminders are off unless the store asks for them"
        );
    }

    /// Detects a misspelled key read as "the default": the person who wrote it
    /// asked for a behaviour and has to be told it is not happening.
    #[test]
    fn a_key_the_schema_does_not_define_is_a_problem() {
        let (_, problems) = parse(b"remind_tokens: 1000\n").expect("the file parses");
        assert_eq!(
            problems,
            vec![SettingProblem::UnknownKey {
                key: "remind_tokens".to_string()
            }]
        );
    }

    /// Detects a value of the wrong type being coerced instead of refused: a
    /// negative or textual token count is not a token count, and a bool written
    /// as a string is not a bool.
    #[test]
    fn a_value_of_the_wrong_type_is_a_problem_naming_the_type_the_key_takes() {
        for (text, key, expected) in [
            (
                "reminder_tokens: -5\n",
                "reminder_tokens",
                SettingType::IntegerOrNull,
            ),
            (
                "interrupt_on_critical: 'no'\n",
                "interrupt_on_critical",
                SettingType::Boolean,
            ),
            (
                "interrupt_exempt_tools: Read\n",
                "interrupt_exempt_tools",
                SettingType::StringList,
            ),
            (
                "interrupt_exempt_tools:\n- 7\n",
                "interrupt_exempt_tools",
                SettingType::StringList,
            ),
            (
                "tool_result_match_limit: null\n",
                "tool_result_match_limit",
                SettingType::Integer,
            ),
            (
                "characters_per_token: 0\n",
                "characters_per_token",
                SettingType::Number,
            ),
            (
                "characters_per_token: -3.5\n",
                "characters_per_token",
                SettingType::Number,
            ),
            (
                "characters_per_token: '3.5'\n",
                "characters_per_token",
                SettingType::Number,
            ),
        ] {
            let (file, problems) = parse(text.as_bytes()).expect("the file parses");
            assert!(
                matches!(
                    problems.as_slice(),
                    [SettingProblem::WrongType { key: named, mismatch }]
                        if named == key && mismatch.expected == expected
                ),
                "{text:?} must be refused as {expected}, got {problems:?}"
            );
            assert_eq!(
                file.settings,
                Settings::default(),
                "{text:?} sets nothing, so the defaults must stand"
            );
        }
    }

    /// Detects a write that drops the keys it was not asked to change, which
    /// would silently reset every other setting of the store.
    #[test]
    fn setting_one_key_keeps_the_other_entries_of_the_file() {
        let (file, _) =
            parse(b"reminder_tokens: 1000\ndeliver_knowledge_index: false\n").expect("it parses");
        let mut entries = file.entries.clone();
        entries.insert(
            Yaml::String("interrupt_on_critical".to_string()),
            yaml_of_json(&Json::from(false)),
        );

        let text = render(&entries).expect("the entries render");
        let (written, problems) = parse(text.as_bytes()).expect("the rendered file parses");

        assert_eq!(problems, Vec::new(), "the rendered file must be valid");
        assert_eq!(written.settings.reminder_tokens, Some(1_000));
        assert!(!written.settings.deliver_knowledge_index);
        assert!(!written.settings.interrupt_on_critical);
    }

    /// Detects a fractional divisor truncated to a whole number, or refused for
    /// not being one: a token is a fraction of a text, and the documented
    /// default is itself fractional.
    ///
    /// Source: the setting's documented meaning, characters of delivered text
    /// per token, whose default is 3.5.
    #[test]
    fn the_characters_per_token_a_file_sets_is_kept_as_the_fraction_it_was_written_as() {
        let (file, problems) = parse(b"characters_per_token: 3.25\n").expect("the file parses");

        assert_eq!(problems, Vec::new(), "a fractional divisor is valid");
        assert_eq!(file.settings.characters_per_token, 3.25);
        assert_eq!(
            Settings::default().characters_per_token,
            3.5,
            "a store that sets nothing divides by Anthropic's published figure"
        );
        let written = render(&file.entries).expect("the entries render");
        let (again, _) = parse(written.as_bytes()).expect("the rendered file parses");
        assert_eq!(
            again.settings.characters_per_token, 3.25,
            "the fraction must survive being written back to the file"
        );
    }

    /// Detects an exemption list that is not matched exactly: a prefix or a
    /// case-insensitive match would hold or release the wrong tools.
    #[test]
    fn only_a_tool_named_exactly_in_the_exemptions_is_released() {
        let settings = Settings {
            interrupt_exempt_tools: vec!["Read".to_string()],
            ..Settings::default()
        };
        assert!(!settings.interrupts(Some("Read")));
        assert!(settings.interrupts(Some("ReadFile")));
        assert!(settings.interrupts(Some("read")));
        assert!(settings.interrupts(Some("Bash")));
        assert!(!settings.interrupts(None));
    }
}
