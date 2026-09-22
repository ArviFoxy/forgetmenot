//! Memory files: Claude Code's own auto-memory frontmatter, then a markdown
//! body.
//!
//! The format is Claude Code's so that a directory of memories Claude Code
//! wrote is already a valid store: `name` and `description` at the top level,
//! everything this server adds inside `metadata`, and every key either tool
//! does not know preserved untouched at whichever level it appeared.
//!
//! The one key this server drops rather than preserves is the legacy
//! `metadata.archived`, which a memory used to be retired with before retiring
//! one meant deleting its file.
//!
//! `kind`, `scope` and `source` are optional because a file Claude Code wrote
//! has none of them. Each is read through an accessor that applies its
//! documented default, and an absent key is never written back, so reading a
//! file and writing it again changes nothing. The one exception is the legacy
//! `metadata.scopes` list, which is read as the single scope its first entry
//! names and written back as `scope`, so a file in the list form reshapes itself
//! the first time anything writes it.

use std::ops::Range;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::frontmatter::{self, FrontmatterError};
use super::{MemoryId, ScopeId};

/// The kind of a memory with no `metadata.kind`: an index line, not a rule
/// pushed in full, because a file that never declared itself critical must not
/// start interrupting tool calls.
pub const DEFAULT_KIND: MemoryKind = MemoryKind::Knowledge;

/// The source of a memory with no `metadata.source`.
pub const DEFAULT_SOURCE: &str = "assistant";

/// The `metadata` keys this server owns. Each has a field of its own on every
/// write, so a write that carries one inside `metadata` is refused rather than
/// setting it twice from two places. `scopes` is here because the reader still
/// takes a memory's scope from it: a write that put it back inside `metadata`
/// would leave a file carrying two spellings of the same field.
pub const OWNED_METADATA_KEYS: [&str; 6] =
    ["kind", "scope", "scopes", "source", "created", "author"];

/// The scope of a memory whose file names none.
static DEFAULT_SCOPE: LazyLock<ScopeId> = LazyLock::new(ScopeId::global);

/// A `metadata` key earlier versions of this server maintained, when a memory
/// could be retired by a flag instead of being deleted. It means nothing now:
/// it is dropped when a file is read, and never written.
const LEGACY_ARCHIVED_KEY: &str = "archived";

/// How a memory is delivered to a context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Delivered in full whenever it becomes due.
    Critical,
    /// Delivered as one index line; the model fetches the body if it wants it.
    Knowledge,
}

/// Who wrote the memory: `user` or `assistant` by convention, and whatever
/// else a person or another tool wrote there.
///
/// Not an enum, because the value belongs to whoever keeps the file: a memory
/// directory in use carries sources this server never invented, `derived`
/// among them, and a value it could not hold would be lost on the first write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemorySource(String);

impl MemorySource {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MemorySource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The `metadata` block of a memory file.
///
/// Every field is optional and is written back only if the file had it, so a
/// file Claude Code wrote does not grow keys it never carried.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MemoryKind>,
    /// The one scope this memory is delivered in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeId>,
    /// The list earlier versions of this server wrote, when a memory could name
    /// several scopes. Read so that a file in that shape is still delivered, and
    /// never written: [`MemoryDocument::parse`] takes the scope from its first
    /// entry into `scope`, which is the key every write emits.
    #[serde(default, skip_serializing, rename = "scopes")]
    pub legacy_scopes: Option<Vec<ScopeId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MemorySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Keys inside `metadata` that this server does not interpret, Claude
    /// Code's own `type` among them.
    ///
    /// An order-preserving map, so that writing a file back does not reshuffle
    /// the keys its author wrote and turn every save into a large diff.
    #[serde(flatten)]
    pub extra: yaml_serde::Mapping,
}

/// Why a write's `metadata` could not be taken.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MetadataError {
    #[error(
        "`{0}` is forgetmenot's own field: send it as the `{0}` field of the write, \
         not inside `metadata`"
    )]
    OwnedKey(String),
    /// A value the write carried that cannot be written into a YAML file. JSON
    /// values all can, so this is the failure of a caller sending something
    /// else, not of a memory file.
    #[error("the `metadata` value for `{key}` cannot be written to a memory file: {message}")]
    Unwritable { key: String, message: String },
}

impl MemoryMetadata {
    /// The extra keys as JSON, which is the form the API and the MCP tools
    /// report them in.
    ///
    /// A key or a value that YAML holds and JSON does not, a mapping key that
    /// is not a string among them, is reported as the YAML text the file
    /// carries, so a reader is shown what the file says rather than nothing.
    pub fn extra_as_json(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut reported = serde_json::Map::new();
        for (key, value) in self.extra.iter() {
            let name = match key {
                yaml_serde::Value::String(name) => name.clone(),
                other => yaml_text(other),
            };
            let value = serde_json::to_value(value)
                .unwrap_or_else(|_| serde_json::Value::String(yaml_text(value)));
            reported.insert(name, value);
        }
        reported
    }

    /// Replace every extra key with the ones given, in the order given: what a
    /// write of the whole memory does, so the file ends up carrying exactly the
    /// keys the write named.
    pub fn replace_extra(
        &mut self,
        given: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), MetadataError> {
        let mut replacement = yaml_serde::Mapping::new();
        for (key, value) in given {
            check_owned(key)?;
            replacement.insert(
                yaml_serde::Value::String(key.clone()),
                yaml_value(key, value)?,
            );
        }
        self.extra = replacement;
        Ok(())
    }

    /// Merge the keys given into the extra keys: a key given is added or
    /// replaced, a key given as `null` is removed, and a key not given keeps
    /// the value it has. A replaced key keeps its place in the file, so setting
    /// one key does not reshuffle the others.
    pub fn merge_extra(
        &mut self,
        given: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), MetadataError> {
        for key in given.keys() {
            check_owned(key)?;
        }
        for (key, value) in given {
            if value.is_null() {
                self.extra.shift_remove(key.as_str());
                continue;
            }
            self.extra.insert(
                yaml_serde::Value::String(key.clone()),
                yaml_value(key, value)?,
            );
        }
        Ok(())
    }

    /// Whether the block holds nothing, in which case it is not written at all.
    ///
    /// The legacy list is not counted: nothing writes it, so a block that holds
    /// only that holds nothing a write would put in the file.
    pub fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.scope.is_none()
            && self.source.is_none()
            && self.created.is_none()
            && self.author.is_none()
            && self.extra.is_empty()
    }
}

/// Refuse a key this server writes from a field of its own.
fn check_owned(key: &str) -> Result<(), MetadataError> {
    if OWNED_METADATA_KEYS.contains(&key) {
        return Err(MetadataError::OwnedKey(key.to_string()));
    }
    Ok(())
}

/// One `metadata` value as a memory file holds it.
fn yaml_value(key: &str, value: &serde_json::Value) -> Result<yaml_serde::Value, MetadataError> {
    yaml_serde::to_value(value).map_err(|error| MetadataError::Unwritable {
        key: key.to_string(),
        message: error.to_string(),
    })
}

/// One YAML value as the text a file would carry for it, for the values JSON
/// cannot hold.
fn yaml_text(value: &yaml_serde::Value) -> String {
    yaml_serde::to_string(value)
        .map(|text| text.trim_end().to_string())
        .unwrap_or_default()
}

/// The frontmatter of a memory file.
///
/// Field order is the serialization order: `name`, `description`, `modified`,
/// the top-level keys this server does not interpret, then `metadata`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemoryFrontmatter {
    /// Must equal the last segment of the memory id.
    pub name: String,
    /// The index entry. Claude Code's format requires it; a file without one
    /// loads with an empty index entry and a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Top level, because that is where Claude Code stamps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
    /// Top-level keys this server does not interpret, in the order they
    /// appeared in the file.
    #[serde(flatten)]
    pub extra: yaml_serde::Mapping,
    #[serde(default, skip_serializing_if = "MemoryMetadata::is_empty")]
    pub metadata: MemoryMetadata,
}

/// A parsed memory file.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryDocument {
    /// The path under `memories/` without `.md`; not stored in the file.
    pub id: MemoryId,
    pub frontmatter: MemoryFrontmatter,
    /// The markdown body, exactly as it appeared in the file.
    pub body: String,
}

impl MemoryDocument {
    /// Parse the memory stored at the path `id` names.
    ///
    /// The legacy `metadata.archived` key is dropped here rather than kept as
    /// an unknown key, so that a file that carries it is read like any other
    /// and writing the file back does not put the key in again.
    ///
    /// A file that names its scope in the legacy `scopes` list is read as
    /// carrying the first entry of that list, so everything downstream sees one
    /// scope and the next write of the file emits `scope`. The list itself stays
    /// on the metadata, unwritten, for [`legacy_scope_list`] to report.
    pub fn parse(id: MemoryId, bytes: &[u8]) -> Result<Self, FrontmatterError> {
        let (mut frontmatter, body): (MemoryFrontmatter, String) = frontmatter::parse(bytes)?;
        frontmatter.metadata.extra.shift_remove(LEGACY_ARCHIVED_KEY);
        if frontmatter.metadata.scope.is_none()
            && let Some(first) = frontmatter
                .metadata
                .legacy_scopes
                .as_ref()
                .and_then(|scopes| scopes.first())
        {
            frontmatter.metadata.scope = Some(first.clone());
        }
        Ok(Self {
            id,
            frontmatter,
            body,
        })
    }

    /// Render the memory back to file text.
    pub fn render(&self) -> Result<String, FrontmatterError> {
        frontmatter::render(&self.frontmatter, &self.body)
    }

    /// The declared name, which must equal the last segment of the id.
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }

    /// What to show as the memory's heading: the first level-1 heading of the
    /// body, or the name when the body has none.
    pub fn title(&self) -> &str {
        first_level_one_heading(&self.body).unwrap_or(&self.frontmatter.name)
    }

    /// The index entry, empty when the file has no `description`.
    pub fn description(&self) -> &str {
        self.frontmatter.description.as_deref().unwrap_or_default()
    }

    /// Whether the file carries a `description` at all.
    pub fn has_description(&self) -> bool {
        self.frontmatter
            .description
            .as_ref()
            .is_some_and(|description| !description.trim().is_empty())
    }

    pub fn kind(&self) -> MemoryKind {
        self.frontmatter.metadata.kind.unwrap_or(DEFAULT_KIND)
    }

    /// The text this memory puts in an agent's context: the whole body for a
    /// critical memory, without the trailing blank space the rendered text drops,
    /// and the description for a knowledge memory, which is all its index line
    /// carries of it.
    ///
    /// Two versions of a memory are compared through this, so it has to be the
    /// text that is actually delivered rather than the file's.
    pub fn delivered_text(&self) -> &str {
        match self.kind() {
            MemoryKind::Critical => self.body.trim_end(),
            MemoryKind::Knowledge => self.description(),
        }
    }

    /// The one scope this memory is delivered in.
    pub fn scope(&self) -> &ScopeId {
        self.frontmatter
            .metadata
            .scope
            .as_ref()
            .unwrap_or(&DEFAULT_SCOPE)
    }

    /// The scopes of the legacy `scopes` list when the file names more than one,
    /// which is the shape the store reports so that nothing quietly stops being
    /// delivered: the memory is in the first of them and the rest are dropped by
    /// the next write.
    pub fn legacy_scope_list(&self) -> Option<&[ScopeId]> {
        self.frontmatter
            .metadata
            .legacy_scopes
            .as_deref()
            .filter(|scopes| scopes.len() > 1)
    }

    pub fn source(&self) -> MemorySource {
        self.frontmatter
            .metadata
            .source
            .clone()
            .unwrap_or_else(|| MemorySource::new(DEFAULT_SOURCE))
    }

    pub fn created(&self) -> Option<DateTime<Utc>> {
        self.frontmatter.metadata.created
    }

    pub fn modified(&self) -> Option<DateTime<Utc>> {
        self.frontmatter.modified
    }

    pub fn author(&self) -> Option<&str> {
        self.frontmatter.metadata.author.as_deref()
    }

    /// The `[[target]]` links in the body, in the order they appear.
    pub fn links(&self) -> Vec<String> {
        extract_links(&self.body)
    }
}

/// The text of the body's first level-1 ATX heading.
///
/// Fenced code blocks are skipped so that a `#` comment in an example is not
/// mistaken for the title. Only ATX headings (`# Title`) count; a setext
/// underline is not recognised.
fn first_level_one_heading(body: &str) -> Option<&str> {
    let mut open_fence: Option<(u8, usize)> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some((fence_character, fence_length)) = open_fence {
            if closes_fence(trimmed, fence_character, fence_length) {
                open_fence = None;
            }
            continue;
        }
        if let Some(fence) = opening_fence(trimmed) {
            open_fence = Some(fence);
            continue;
        }
        if let Some(text) = level_one_heading(line) {
            return Some(text);
        }
    }
    None
}

/// The text of `line` if it is a level-1 ATX heading.
fn level_one_heading(line: &str) -> Option<&str> {
    let without_indent = line.trim_start();
    // Four spaces of indentation make an indented code block, not a heading.
    if line.len() - without_indent.len() > 3 {
        return None;
    }
    let rest = without_indent.strip_prefix('#')?;
    if rest.starts_with('#') {
        return None;
    }
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    Some(rest.trim().trim_end_matches('#').trim())
}

/// The `[[target]]` links in a markdown body, in document order.
///
/// Fenced code blocks and inline code spans are skipped: documentation that
/// shows the link syntax must not create links, and a memory about markdown
/// would otherwise link to whatever its examples name.
pub fn extract_links(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    visit_links(body, |_, target| links.push(target.to_string()));
    links
}

/// The body with every link to `from` rewritten to name `to`.
///
/// Only real links are rewritten: the same text inside a code span or a fenced
/// block is not a link and is left exactly as it was, which is the rule
/// [`extract_links`] reads by.
pub fn rewrite_links(body: &str, from: &str, to: &str) -> String {
    let mut rewritten = String::with_capacity(body.len());
    let mut copied = 0;
    visit_links(body, |range, target| {
        if target == from {
            rewritten.push_str(&body[copied..range.start]);
            rewritten.push_str("[[");
            rewritten.push_str(to);
            rewritten.push_str("]]");
            copied = range.end;
        }
    });
    rewritten.push_str(&body[copied..]);
    rewritten
}

/// Call `visit` for every `[[target]]` link of `body`, with the byte range of
/// the whole link and the target it names.
///
/// The one scan both reading and rewriting links use, so a link the store
/// resolves and a link a rename rewrites are always the same set.
fn visit_links(body: &str, mut visit: impl FnMut(Range<usize>, &str)) {
    let mut open_fence: Option<(u8, usize)> = None;
    let mut offset = 0;
    // Split so that each piece keeps its newline, because the ranges are
    // offsets into the body and a line's length has to include what follows it.
    for chunk in body.split_inclusive('\n') {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim_start();
        match open_fence {
            Some((fence_character, fence_length)) => {
                if closes_fence(trimmed, fence_character, fence_length) {
                    open_fence = None;
                }
            }
            None => match opening_fence(trimmed) {
                Some(fence) => open_fence = Some(fence),
                None => visit_links_in_line(line, offset, &mut visit),
            },
        }
        offset += chunk.len();
    }
}

/// The character and length of the fence this line opens, if it opens one.
fn opening_fence(trimmed: &str) -> Option<(u8, usize)> {
    let fence_character = *trimmed.as_bytes().first()?;
    if fence_character != b'`' && fence_character != b'~' {
        return None;
    }
    let length = trimmed
        .bytes()
        .take_while(|byte| *byte == fence_character)
        .count();
    if length < 3 {
        None
    } else {
        Some((fence_character, length))
    }
}

/// Whether this line closes a fence of the given character and length.
fn closes_fence(trimmed: &str, fence_character: u8, fence_length: usize) -> bool {
    match opening_fence(trimmed) {
        Some((character, length)) => {
            character == fence_character
                && length >= fence_length
                && trimmed[length..].trim().is_empty()
        }
        None => false,
    }
}

/// Visit the links of one line, skipping inline code spans. `base` is the
/// line's offset in the body, so the ranges name the body.
fn visit_links_in_line(line: &str, base: usize, visit: &mut impl FnMut(Range<usize>, &str)) {
    let bytes = line.as_bytes();
    let mut position = 0;
    while position < bytes.len() {
        if bytes[position] == b'`' {
            let run_start = position;
            while position < bytes.len() && bytes[position] == b'`' {
                position += 1;
            }
            // An unmatched run of backticks is literal text, so scanning simply
            // continues after it rather than skipping the rest of the line.
            if let Some(after_closing) = find_backtick_run(bytes, position, position - run_start) {
                position = after_closing;
            }
            continue;
        }
        if bytes[position] == b'[' && bytes.get(position + 1) == Some(&b'[') {
            let content_start = position + 2;
            if let Some(relative_end) = line[content_start..].find("]]") {
                let end = content_start + relative_end + 2;
                let target = line[content_start..content_start + relative_end].trim();
                if !target.is_empty() && !target.contains('[') {
                    visit(base + position..base + end, target);
                }
                position = end;
                continue;
            }
        }
        position += 1;
    }
}

/// The offset just past the next run of exactly `length` backticks at or after
/// `from`, or `None` if the line holds no such run.
fn find_backtick_run(bytes: &[u8], from: usize, length: usize) -> Option<usize> {
    let mut position = from;
    while position < bytes.len() {
        if bytes[position] != b'`' {
            position += 1;
            continue;
        }
        let run_start = position;
        while position < bytes.len() && bytes[position] == b'`' {
            position += 1;
        }
        if position - run_start == length {
            return Some(position);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> MemoryDocument {
        MemoryDocument::parse(MemoryId::new("widget-release"), text.as_bytes())
            .unwrap_or_else(|error| panic!("the fixture parses: {error}"))
    }

    /// A file in exactly the shape Claude Code writes: the two required keys
    /// and one `metadata` key of its own, nothing this server added.
    const CLAUDE_PLAIN: &str = concat!(
        "---\n",
        "name: widget-release\n",
        "description: Releases are cut from main only\n",
        "metadata:\n",
        "  type: feedback\n",
        "---\n",
        "Free markdown body, no heading.\n",
    );

    /// Detects defaults that differ from the documented ones. A file Claude
    /// Code wrote declares none of these, and reading it as critical would
    /// start interrupting tool calls over a memory nobody marked critical.
    #[test]
    fn a_file_in_claudes_plain_format_gets_the_documented_defaults() {
        let document = parse(CLAUDE_PLAIN);
        assert_eq!(
            document.kind(),
            MemoryKind::Knowledge,
            "the default kind is wrong"
        );
        assert_eq!(
            document.scope(),
            &ScopeId::global(),
            "the default scope is wrong"
        );
        assert_eq!(
            document.source(),
            MemorySource::new("assistant"),
            "the default source is wrong"
        );
        assert_eq!(document.description(), "Releases are cut from main only");
    }

    /// Detects a write that adds the keys the defaults stand for: an existing
    /// Claude Code memory directory must survive being used as a store without
    /// every file gaining `kind`, `scope` and `source` lines.
    #[test]
    fn a_file_in_claudes_plain_format_round_trips_byte_for_byte() {
        let rendered = parse(CLAUDE_PLAIN).render().expect("the memory renders");
        assert_eq!(
            rendered, CLAUDE_PLAIN,
            "the file changed when it was written back"
        );
    }

    /// Detects metadata that is parsed but ignored, which would deliver every
    /// memory as a global knowledge line whatever its file said.
    #[test]
    fn metadata_keys_override_the_defaults() {
        let text = concat!(
            "---\n",
            "name: widget-release\n",
            "description: Releases are cut from main only\n",
            "metadata:\n",
            "  kind: critical\n",
            "  scope: widgets\n",
            "  source: user\n",
            "---\n",
            "Body.\n",
        );
        let document = parse(text);
        assert_eq!(document.kind(), MemoryKind::Critical);
        assert_eq!(document.scope(), &ScopeId::new("widgets"));
        assert_eq!(document.source(), MemorySource::new("user"));
        assert_eq!(
            document.render().expect("the memory renders"),
            text,
            "the scope must be written back as the file had it"
        );
    }

    /// Detects a reader that ignores the legacy `scopes` list, which would
    /// deliver every memory whose file is in the list form in `global` instead
    /// of its own scope, and a write that leaves the list in the file, which
    /// would leave the two spellings of the field to drift apart.
    ///
    /// Source: the documented reading rule, `scope` when present and else the
    /// first entry of the legacy list.
    #[test]
    fn a_legacy_one_entry_scopes_list_is_read_as_that_scope_and_written_back_as_scope() {
        let document = parse(concat!(
            "---\n",
            "name: widget-release\n",
            "description: Releases are cut from main only\n",
            "metadata:\n",
            "  kind: critical\n",
            "  scopes:\n",
            "  - widgets\n",
            "---\n",
            "Body.\n",
        ));

        assert_eq!(document.scope(), &ScopeId::new("widgets"));
        let rendered = document.render().expect("the memory renders");
        assert!(
            rendered.contains("scope: widgets") && !rendered.contains("scopes:"),
            "the file must be written back with the single-scope key alone, got {rendered}"
        );
    }

    /// Detects a legacy list read as any entry but its first, which would
    /// deliver a memory somewhere its author never put it, and a list of several
    /// entries that is read without being reported, which would silently stop
    /// delivering the memory in every scope but one.
    ///
    /// Source: the documented reading rule, the first entry of the list.
    #[test]
    fn a_legacy_scopes_list_of_several_entries_is_read_as_its_first_and_reported_whole() {
        let document = parse(concat!(
            "---\n",
            "name: widget-release\n",
            "description: Releases are cut from main only\n",
            "metadata:\n",
            "  kind: critical\n",
            "  scopes:\n",
            "  - widgets\n",
            "  - rocketry\n",
            "---\n",
            "Body.\n",
        ));

        assert_eq!(document.scope(), &ScopeId::new("widgets"));
        assert_eq!(
            document.legacy_scope_list(),
            Some(&[ScopeId::new("widgets"), ScopeId::new("rocketry")][..]),
            "a file naming several scopes must be reported with all of them"
        );
    }

    /// Detects a one-entry list reported as a file that names several, which
    /// would put a warning on every memory whose file is in the list form,
    /// although such a file names exactly the one scope it is delivered in.
    #[test]
    fn a_file_naming_one_scope_is_not_reported_as_naming_several() {
        let one_entry = concat!(
            "---\n",
            "name: widget-release\n",
            "description: Releases are cut from main only\n",
            "metadata:\n",
            "  scopes:\n",
            "  - widgets\n",
            "---\n",
            "Body.\n",
        );
        for text in [CLAUDE_PLAIN, one_entry] {
            assert_eq!(
                parse(text).legacy_scope_list(),
                None,
                "a file naming at most one scope has nothing to report, got {text}"
            );
        }
    }

    /// Detects a `source` that only accepts the two conventional values: a
    /// memory directory in use carries others, and a reader that refuses them
    /// or rewrites them makes the file unreadable or changes what it says.
    #[test]
    fn a_source_that_is_neither_user_nor_assistant_is_read_and_written_back_as_it_was() {
        let text = concat!(
            "---\n",
            "name: widget-release\n",
            "description: Releases are cut from main only\n",
            "metadata:\n",
            "  source: derived\n",
            "---\n",
            "Body.\n",
        );
        let document = parse(text);
        assert_eq!(document.source(), MemorySource::new("derived"));
        assert_eq!(
            document.render().expect("the memory renders"),
            text,
            "the file changed when it was written back"
        );
    }

    /// Detects a replace that keeps keys the write did not name, which would
    /// leave a memory carrying metadata its author had just taken out, and one
    /// that reorders the keys it was given.
    #[test]
    fn replacing_the_extra_metadata_writes_exactly_the_keys_given_in_the_order_given() {
        let mut document = parse(concat!(
            "---\n",
            "name: widget-release\n",
            "description: d\n",
            "metadata:\n",
            "  kind: critical\n",
            "  legacy: keep me out\n",
            "---\n",
            "Body.\n",
        ));
        let given = serde_json::json!({ "type": "feedback", "strength": "hard" });
        document
            .frontmatter
            .metadata
            .replace_extra(given.as_object().expect("the fixture is an object"))
            .expect("neither key is forgetmenot's own");

        let rendered = document.render().expect("the memory renders");
        assert!(
            !rendered.contains("legacy"),
            "a key the write did not name was kept:\n{rendered}"
        );
        assert_eq!(
            document.frontmatter.metadata.kind,
            Some(MemoryKind::Critical),
            "replacing the extra keys must not touch forgetmenot's own"
        );
        let type_at = rendered
            .find("type:")
            .expect("the given key is in the file");
        let strength_at = rendered
            .find("strength:")
            .expect("the given key is in the file");
        assert!(
            type_at < strength_at,
            "the keys must be written in the order they were given:\n{rendered}"
        );
        assert!(
            rendered.find("kind:").expect("kind is in the file") < type_at,
            "forgetmenot's own keys come first:\n{rendered}"
        );
    }

    /// Detects a merge that drops the keys it was not given, one that moves a
    /// replaced key to the end of the file, and a `null` that writes a null
    /// value instead of taking the key out.
    #[test]
    fn merging_extra_metadata_adds_replaces_and_removes_only_the_keys_given() {
        let mut document = parse(concat!(
            "---\n",
            "name: widget-release\n",
            "description: d\n",
            "metadata:\n",
            "  type: feedback\n",
            "  strength: hard\n",
            "  node_type: rule\n",
            "---\n",
            "Body.\n",
        ));
        let given =
            serde_json::json!({ "type": "reference", "node_type": null, "originSessionId": "s-1" });
        document
            .frontmatter
            .metadata
            .merge_extra(given.as_object().expect("the fixture is an object"))
            .expect("none of the keys is forgetmenot's own");

        let rendered = document.render().expect("the memory renders");
        assert!(
            rendered.contains("strength: hard"),
            "a key the merge did not name was lost:\n{rendered}"
        );
        assert!(
            rendered.contains("type: reference"),
            "the given key was not replaced:\n{rendered}"
        );
        assert!(
            !rendered.contains("node_type"),
            "the key given as null was not removed:\n{rendered}"
        );
        assert!(
            rendered.contains("originSessionId: s-1"),
            "the new key was not added:\n{rendered}"
        );
        assert!(
            rendered.find("type:").expect("type is in the file")
                < rendered.find("strength:").expect("strength is in the file"),
            "a replaced key must keep its place in the file:\n{rendered}"
        );
    }

    /// Detects a write that takes one of forgetmenot's own fields from inside
    /// `metadata`, which would set it from two places at once and silently
    /// overrule the field the caller sent.
    #[test]
    fn a_write_that_puts_one_of_forgetmenots_own_keys_inside_metadata_is_refused_naming_it() {
        for key in OWNED_METADATA_KEYS {
            let mut metadata = MemoryMetadata::default();
            let given = serde_json::json!({ key: "whatever" });
            let given = given.as_object().expect("the fixture is an object");
            for refusal in [metadata.replace_extra(given), metadata.merge_extra(given)] {
                let error = refusal.expect_err(&format!("`{key}` is forgetmenot's own field"));
                assert!(
                    error.to_string().contains(key),
                    "the refusal must name the key, got {error}"
                );
            }
            assert!(
                metadata.extra.is_empty(),
                "a refused write must leave the metadata alone"
            );
        }
    }

    /// Detects a nested value flattened or dropped on the way in or out, which
    /// would rewrite a mapping somebody keeps in the frontmatter.
    #[test]
    fn a_nested_metadata_value_survives_the_way_in_and_the_way_out() {
        let mut metadata = MemoryMetadata::default();
        let given = serde_json::json!({ "review": { "by": "2026-12-01", "every": 90 } });
        metadata
            .replace_extra(given.as_object().expect("the fixture is an object"))
            .expect("`review` is not forgetmenot's own field");

        assert_eq!(
            serde_json::Value::Object(metadata.extra_as_json()),
            given,
            "the nested value read back differently from the way it was written"
        );
    }

    /// Detects unknown keys being dropped, at the top level or inside
    /// `metadata`, which would delete parts of a file its author never touched.
    #[test]
    fn unknown_keys_at_both_levels_survive_a_round_trip() {
        let text = concat!(
            "---\n",
            "name: widget-release\n",
            "description: Releases are cut from main only\n",
            "modified: 2026-09-12T10:00:00Z\n",
            "review-by: 2026-12-01\n",
            "metadata:\n",
            "  kind: critical\n",
            "  type: feedback\n",
            "  strength: hard\n",
            "---\n",
            "Body.\n",
        );
        let document = parse(text);
        assert!(
            document.frontmatter.extra.contains_key("review-by"),
            "a top-level key was dropped"
        );
        assert!(
            document.frontmatter.metadata.extra.contains_key("type"),
            "a metadata key was dropped"
        );
        assert_eq!(
            document.render().expect("the memory renders"),
            text,
            "the file changed"
        );
    }

    /// Detects a title read from the frontmatter or left empty, which would
    /// leave a memory with no heading unnamed in the frontend.
    #[test]
    fn the_title_falls_back_to_the_name_when_the_body_has_no_heading() {
        assert_eq!(parse(CLAUDE_PLAIN).title(), "widget-release");
    }

    /// Detects a title that ignores the body's heading.
    #[test]
    fn the_title_is_the_bodys_first_level_one_heading() {
        let document = parse(concat!(
            "---\n",
            "name: widget-release\n",
            "description: d\n",
            "---\n",
            "# Widgets release rule\n",
            "\n",
            "Body.\n",
        ));
        assert_eq!(document.title(), "Widgets release rule");
    }

    /// Detects a heading scan that reads inside code fences, which would take a
    /// shell comment in an example as the memory's title.
    #[test]
    fn a_heading_inside_a_code_fence_is_not_the_title() {
        let document = parse(concat!(
            "---\n",
            "name: widget-release\n",
            "description: d\n",
            "---\n",
            "```sh\n",
            "# not a title\n",
            "```\n",
            "\n",
            "# Real title\n",
        ));
        assert_eq!(document.title(), "Real title");
    }

    /// Detects a legacy `archived` key kept as an unknown key, which would put
    /// it back into the file on the next write and leave a memory carrying a
    /// flag nothing acts on; and a reader that drops the neighbouring keys with
    /// it, which would delete parts of a file its author wrote.
    #[test]
    fn a_legacy_archived_key_is_dropped_and_its_neighbours_are_kept() {
        let text = concat!(
            "---\n",
            "name: widget-release\n",
            "description: Releases are cut from main only\n",
            "metadata:\n",
            "  kind: critical\n",
            "  archived: true\n",
            "  type: feedback\n",
            "---\n",
            "Body.\n",
        );
        let document = parse(text);

        let rendered = document.render().expect("the memory renders");
        assert!(
            !rendered.contains("archived"),
            "the legacy key was written back:\n{rendered}"
        );
        assert_eq!(
            document.kind(),
            MemoryKind::Critical,
            "the key next to the legacy one was lost"
        );
        assert!(
            document.frontmatter.metadata.extra.contains_key("type"),
            "an unknown key next to the legacy one was dropped:\n{rendered}"
        );
    }

    /// Detects a missing `description` being reported as present, which would
    /// put an empty index entry in front of the model with nothing to say it is
    /// empty.
    #[test]
    fn a_file_without_a_description_reports_that_it_has_none() {
        let document = parse("---\nname: widget-release\n---\nBody.\n");
        assert!(
            !document.has_description(),
            "an absent description was reported as present"
        );
        assert_eq!(
            document.description(),
            "",
            "an absent description is not empty"
        );
    }

    /// Detects a link scan that ignores fences, which would turn the example
    /// links in a memory about this syntax into real links and then into
    /// missing-target validation errors.
    #[test]
    fn links_inside_a_fenced_code_block_are_not_extracted() {
        let body = "before [[real]]\n\n```markdown\n[[fenced]]\n```\n\nafter [[second]]\n";
        assert_eq!(
            extract_links(body),
            vec!["real".to_string(), "second".to_string()]
        );
    }

    /// Detects a link scan that ignores inline code spans.
    #[test]
    fn links_inside_an_inline_code_span_are_not_extracted() {
        let body = "write `[[spanned]]` to link, as in [[real]].\n";
        assert_eq!(extract_links(body), vec!["real".to_string()]);
    }

    /// Detects a scan that stops at the first link or reorders them, since the
    /// validation report names targets in document order.
    #[test]
    fn every_prose_link_is_extracted_in_order() {
        let body = "see [[one]] and [[two]], then [[three]]\n";
        assert_eq!(
            extract_links(body),
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
    }

    /// Detects an unmatched backtick swallowing the rest of the line, which
    /// would hide links after a stray backtick.
    #[test]
    fn an_unmatched_backtick_does_not_hide_later_links() {
        assert_eq!(
            extract_links("a ` stray tick and [[real]]\n"),
            vec!["real".to_string()]
        );
    }

    /// Detects an opening `[[` with no closing `]]` consuming the line, or
    /// producing an empty target that can never resolve.
    #[test]
    fn an_unclosed_or_empty_link_yields_no_target() {
        assert_eq!(
            extract_links("a [[unclosed and [[]] here\n"),
            Vec::<String>::new()
        );
    }
}
