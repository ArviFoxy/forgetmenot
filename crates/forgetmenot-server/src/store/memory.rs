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
//! `kind`, `scopes` and `source` are optional because a file Claude Code wrote
//! has none of them. Each is read through an accessor that applies its
//! documented default, and an absent key is never written back, so reading a
//! file and writing it again changes nothing.

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
pub const DEFAULT_SOURCE: MemorySource = MemorySource::Assistant;

/// The scopes of a memory with no `metadata.scopes`.
static DEFAULT_SCOPES: LazyLock<[ScopeId; 1]> = LazyLock::new(|| [ScopeId::global()]);

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

/// Who wrote the memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    User,
    Assistant,
}

/// The `metadata` block of a memory file.
///
/// Every field is optional and is written back only if the file had it, so a
/// file Claude Code wrote does not grow keys it never carried.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MemoryKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<ScopeId>>,
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

impl MemoryMetadata {
    /// Whether the block holds nothing, in which case it is not written at all.
    pub fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.scopes.is_none()
            && self.source.is_none()
            && self.created.is_none()
            && self.author.is_none()
            && self.extra.is_empty()
    }
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
    pub fn parse(id: MemoryId, bytes: &[u8]) -> Result<Self, FrontmatterError> {
        let (mut frontmatter, body): (MemoryFrontmatter, String) = frontmatter::parse(bytes)?;
        frontmatter.metadata.extra.shift_remove(LEGACY_ARCHIVED_KEY);
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

    pub fn scopes(&self) -> &[ScopeId] {
        match &self.frontmatter.metadata.scopes {
            Some(scopes) => scopes,
            None => &*DEFAULT_SCOPES,
        }
    }

    pub fn source(&self) -> MemorySource {
        self.frontmatter.metadata.source.unwrap_or(DEFAULT_SOURCE)
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
    let mut open_fence: Option<(u8, usize)> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        match open_fence {
            Some((fence_character, fence_length)) => {
                if closes_fence(trimmed, fence_character, fence_length) {
                    open_fence = None;
                }
            }
            None => match opening_fence(trimmed) {
                Some(fence) => open_fence = Some(fence),
                None => collect_links_in_line(line, &mut links),
            },
        }
    }
    links
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

/// Append the links of one line, skipping inline code spans.
fn collect_links_in_line(line: &str, links: &mut Vec<String>) {
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
                let target = line[content_start..content_start + relative_end].trim();
                if !target.is_empty() && !target.contains('[') {
                    links.push(target.to_string());
                }
                position = content_start + relative_end + 2;
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
            document.scopes(),
            [ScopeId::global()],
            "the default scopes are wrong"
        );
        assert_eq!(
            document.source(),
            MemorySource::Assistant,
            "the default source is wrong"
        );
        assert_eq!(document.description(), "Releases are cut from main only");
    }

    /// Detects a write that adds the keys the defaults stand for: an existing
    /// Claude Code memory directory must survive being used as a store without
    /// every file gaining `kind`, `scopes` and `source` lines.
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
        let document = parse(concat!(
            "---\n",
            "name: widget-release\n",
            "description: Releases are cut from main only\n",
            "metadata:\n",
            "  kind: critical\n",
            "  scopes:\n",
            "  - widgets\n",
            "  source: user\n",
            "---\n",
            "Body.\n",
        ));
        assert_eq!(document.kind(), MemoryKind::Critical);
        assert_eq!(document.scopes(), [ScopeId::new("widgets")]);
        assert_eq!(document.source(), MemorySource::User);
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
