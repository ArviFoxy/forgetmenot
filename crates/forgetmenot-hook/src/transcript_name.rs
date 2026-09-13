//! Reading a session's name and first prompt out of a Claude Code transcript.
//!
//! Three kinds of line carry a name. Naming a session with `/rename` appends
//! `{"type":"custom-title","customTitle":"…","sessionId":"…"}`; Claude Code
//! names a session itself with `{"type":"ai-title","aiTitle":"…",…}`; and a
//! compaction writes `{"type":"summary","summary":"…",…}`. Each is appended and
//! re-emitted as the file grows, so the last line of a kind is the one in force,
//! and a session may carry any combination of them or none at all. Claude Code's
//! own session picker prefers them in that order, which is the order
//! [`session_title`] answers in.
//!
//! The first prompt is in the first line of type `user` that carries the user's
//! own words, which is not always the first user line of the file: the harness
//! writes its own turns as user lines too.
//!
//! Both reads are shaped by the same constraint as [`crate::transcript_tail`]:
//! a transcript grows for the whole life of a session and reaches tens of
//! megabytes, and this runs on every hook event. So the file is streamed in
//! chunks rather than read whole, and only the lines that already look like the
//! answer are parsed as JSON. Every failure gives `None`; nothing here returns
//! an error or panics, because Claude Code waits for this process.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Bytes read from the end of the file when only the tail is scanned for a
/// title. Same size as the first window [`crate::transcript_tail`] reads, and
/// for the same reason: one transcript line with a full tool result in it can
/// be large. It does not grow when no title is found, because "no title" is the
/// ordinary answer for a session nobody named.
const TAIL_WINDOW_BYTES: u64 = 256 * 1024;

/// How far into the file the search for the first prompt may read before giving
/// up. The search stops as soon as it has one, so this bounds only the case
/// where the start of the transcript is all harness turns and huge tool
/// results; a transcript that has said nothing of the user's in its first
/// megabyte is not going to be identified by its first prompt.
const FIRST_PROMPT_CAP_BYTES: u64 = 1024 * 1024;

/// Bytes read from the file at a time. Both scans keep no more than the line
/// they are on, so this only trades read syscalls against memory.
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// One kind of line that carries a name for the session.
///
/// Claude Code writes all three objects with `type` first, so each kind is
/// recognised from the line's first bytes and every other line is skipped
/// without being parsed.
struct TitleKind {
    /// What a line of this kind starts with.
    prefix: &'static [u8],
    /// The field of that object holding the name.
    field: &'static str,
}

/// The kinds of name a transcript carries, in the order Claude Code's own
/// session picker prefers them: the name the user typed, then the one Claude
/// Code generated, then a compaction summary.
const TITLE_KINDS: [TitleKind; 3] = [
    TitleKind {
        prefix: b"{\"type\":\"custom-title\"",
        field: "customTitle",
    },
    TitleKind {
        prefix: b"{\"type\":\"ai-title\"",
        field: "aiTitle",
    },
    TitleKind {
        prefix: b"{\"type\":\"summary\"",
        field: "summary",
    },
];

/// A title line is about a hundred bytes. A line that starts like one and runs
/// past this is not one, and is dropped rather than buffered: the scanner's
/// memory must not depend on what is in the transcript.
const MAXIMUM_TITLE_LINE_BYTES: usize = 64 * 1024;

/// What marks the line carrying the first prompt. A real user line begins with
/// `parentUuid`, `isSidechain` and `promptId`, so `type` is found inside the
/// line and not at its start.
const USER_LINE_MARKER: &[u8] = b"\"type\":\"user\"";

/// How much of the first prompt is kept: enough to tell two sessions apart in a
/// list, short enough that a pasted file does not travel with every hook event.
const PROMPT_CHARACTERS: usize = 200;

/// How much of the transcript a title scan covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleScan {
    /// Every line of the file. This is what `SessionStart` does: it happens
    /// once per session, and it is the only scan that sees the title of a
    /// session whose name was set long ago and whose transcript has grown past
    /// the tail window since.
    WholeFile,
    /// Only the last [`TAIL_WINDOW_BYTES`] of the file. Claude Code re-emits
    /// the title line as the session goes on, so a rename in a running session
    /// is picked up at one of the next events.
    TailWindow,
}

/// What the session whose transcript is at `path` is called, or `None` when the
/// transcript names it in none of the three ways.
///
/// The name the user typed wins over the one Claude Code generated, which wins
/// over a compaction summary, and within each kind the last line of that kind
/// is the one in force. A line that is not valid JSON, or that carries no name
/// field, counts as absent, so a damaged line of one kind does not hide a name
/// of another.
///
/// `None` also covers every way the read can fail: a missing or unreadable
/// file, and a name line the writer had not finished when the file was read.
pub fn session_title(path: impl AsRef<Path>, scan: TitleScan) -> Option<String> {
    let mut file = File::open(path.as_ref()).ok()?;
    let start = match scan {
        TitleScan::WholeFile => 0,
        TitleScan::TailWindow => file
            .metadata()
            .ok()?
            .len()
            .saturating_sub(TAIL_WINDOW_BYTES),
    };
    if start > 0 {
        file.seek(SeekFrom::Start(start)).ok()?;
    }

    // A window that does not start at the beginning of the file starts in the
    // middle of a line, and that fragment is not a line of this file's own.
    let mut scanner = TitleLineScanner::new(start > 0);
    let mut chunk = vec![0u8; READ_CHUNK_BYTES];
    loop {
        let read = file.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        scanner.consume(&chunk[..read]);
    }

    scanner
        .last_line_of_each_kind
        .iter()
        .zip(TITLE_KINDS.iter())
        .find_map(|(line, kind)| {
            let parsed: serde_json::Value = serde_json::from_slice(line.as_ref()?).ok()?;
            Some(parsed.get(kind.field)?.as_str()?.to_string())
        })
}

/// The first thing the user said in the session whose transcript is at `path`,
/// cut to [`PROMPT_CHARACTERS`] characters, or `None` when the transcript
/// carries none.
///
/// The file is read from the start, whatever the event: the first prompt cannot
/// move, and the client keeps no state between events to remember it by. The
/// read stops at the first prompt it accepts, and gives up after
/// [`FIRST_PROMPT_CAP_BYTES`], so a first user line of a few hundred kilobytes
/// still yields its opening characters.
pub fn first_prompt(path: impl AsRef<Path>) -> Option<String> {
    let mut file = File::open(path.as_ref()).ok()?;
    let mut chunk = vec![0u8; READ_CHUNK_BYTES];
    let mut line: Vec<u8> = Vec::new();
    let mut bytes_read: u64 = 0;

    loop {
        let read = file.read(&mut chunk).ok()?;
        if read == 0 {
            // The file ended in the middle of a line, so that line is whatever
            // the writer has got to so far and is not a prompt yet.
            return None;
        }
        bytes_read += read as u64;

        let mut rest = &chunk[..read];
        while let Some(newline) = next_newline(rest) {
            line.extend_from_slice(&rest[..newline]);
            if let Some(prompt) = prompt_in_user_line(&line) {
                return Some(prompt);
            }
            line.clear();
            rest = &rest[newline + 1..];
        }
        line.extend_from_slice(rest);

        if bytes_read >= FIRST_PROMPT_CAP_BYTES {
            return None;
        }
    }
}

/// The user's own words in one transcript line, or `None` if the line is not a
/// user line, is not the user's turn, or carries no text.
///
/// Two kinds of line are not the user's turn even though Claude Code writes
/// them as `user`, and both have to be excluded because neither test catches
/// the other. Measured over the 26 transcripts on this machine on 2026-09-13:
/// the first user line is an `isMeta` line in 17 of them, and in one the user's
/// own first prompt is the fourth user line, behind an `isMeta`
/// `<local-command-caveat>` block and then `<command-name>/model</command-name>`
/// and `<local-command-stdout>Set model to claude-opus-5</local-command-stdout>`,
/// neither of which carries `isMeta`. In the other direction, the `isMeta`
/// lines that carry a skill's instructions open with `Base directory for this
/// skill:` and no tag at all.
///
/// So: an `isMeta` line is skipped, and so is a line whose text opens with `<`.
/// The trade-off that buys is a genuine prompt that opens with `<` being
/// skipped; that is accepted, because every one of these harness blocks opens
/// with one.
fn prompt_in_user_line(line: &[u8]) -> Option<String> {
    if !contains(line, USER_LINE_MARKER) {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_slice(line).ok()?;
    if parsed.get("type")?.as_str()? != "user" {
        return None;
    }
    if parsed.get("isMeta").and_then(serde_json::Value::as_bool) == Some(true) {
        return None;
    }
    let content = parsed.get("message")?.get("content")?;
    let prompt = match content {
        // Claude Code writes a plain prompt as a string and a prompt that
        // carries anything else, an image or a pasted attachment, as blocks.
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    // A user line with no text of its own is a tool result on its way back to
    // the model, not something the user said.
    let opening = prompt.trim_start();
    if opening.is_empty() || opening.starts_with('<') {
        return None;
    }
    Some(cut_to_characters(&prompt, PROMPT_CHARACTERS))
}

/// The first `limit` characters of `text`. Cutting by character rather than by
/// byte is what keeps a prompt whose 200th character is not ASCII from being
/// split down the middle of it.
pub(crate) fn cut_to_characters(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end_of_the_kept_part, _)) => text[..end_of_the_kept_part].to_string(),
        None => text.to_string(),
    }
}

/// Whether `needle` occurs anywhere in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The last line of each [`TITLE_KINDS`] kind in a byte stream fed to it in
/// arbitrary pieces.
///
/// It holds the current line only while that line can still turn out to be a
/// line of some kind, so a transcript of any size costs it a few hundred bytes.
struct TitleLineScanner {
    /// The current line so far.
    line: Vec<u8>,
    /// The current line cannot be a line of any kind, so its bytes are dropped.
    skipping: bool,
    /// The last complete line seen of each kind, kept unparsed, in the order of
    /// [`TITLE_KINDS`].
    last_line_of_each_kind: [Option<Vec<u8>>; TITLE_KINDS.len()],
}

impl TitleLineScanner {
    /// `starts_mid_line` says the first bytes fed in belong to a line that
    /// started before the scan did, so they are dropped up to the first newline.
    fn new(starts_mid_line: bool) -> Self {
        TitleLineScanner {
            line: Vec::new(),
            skipping: starts_mid_line,
            last_line_of_each_kind: [const { None }; TITLE_KINDS.len()],
        }
    }

    /// Take the next piece of the stream. Pieces may split a line anywhere.
    fn consume(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some(newline) = next_newline(rest) {
            self.extend(&rest[..newline]);
            self.end_line();
            rest = &rest[newline + 1..];
        }
        self.extend(rest);
    }

    /// Add a piece of the current line, unless this line is already out.
    fn extend(&mut self, part: &[u8]) {
        if self.skipping {
            return;
        }
        self.line.extend_from_slice(part);
        if !can_still_be_a_title_line(&self.line) {
            self.skipping = true;
            self.line.clear();
        }
    }

    /// A newline arrived. A line the stream ends in the middle of never gets
    /// here, which is how an unfinished last line is discarded.
    fn end_line(&mut self) {
        if !self.skipping {
            for (index, kind) in TITLE_KINDS.iter().enumerate() {
                if self.line.starts_with(kind.prefix) {
                    self.last_line_of_each_kind[index] = Some(std::mem::take(&mut self.line));
                    break;
                }
            }
        }
        self.skipping = false;
        self.line.clear();
    }
}

/// Whether a line that begins with these bytes could still be a line of any
/// kind that carries a name.
fn can_still_be_a_title_line(line: &[u8]) -> bool {
    if line.len() > MAXIMUM_TITLE_LINE_BYTES {
        return false;
    }
    TITLE_KINDS.iter().any(|kind| {
        if line.len() < kind.prefix.len() {
            kind.prefix.starts_with(line)
        } else {
            line.starts_with(kind.prefix)
        }
    })
}

/// Offset of the next newline in `haystack`.
///
/// It reads eight bytes at a time because the `SessionStart` scan covers the
/// whole transcript: over 40 MB, a comparison per byte costs 113 ms in a debug
/// build on this machine against a whole-run budget of 200 ms, and eight bytes
/// in one word costs 46 ms.
fn next_newline(haystack: &[u8]) -> Option<usize> {
    const NEWLINES: u64 = u64::from_ne_bytes([b'\n'; 8]);
    const LOW_BITS: u64 = 0x0101_0101_0101_0101;
    const HIGH_BITS: u64 = 0x8080_8080_8080_8080;

    let mut offset = 0;
    while offset + 8 <= haystack.len() {
        let word = u64::from_ne_bytes(
            haystack[offset..offset + 8]
                .try_into()
                .expect("the slice is eight bytes long"),
        );
        // A byte of `difference` is zero exactly where `haystack` holds a
        // newline, and this expression is non-zero exactly when one of the
        // eight bytes is zero.
        let difference = word ^ NEWLINES;
        if difference.wrapping_sub(LOW_BITS) & !difference & HIGH_BITS != 0
            && let Some(found) = position_of_newline(&haystack[offset..offset + 8])
        {
            return Some(offset + found);
        }
        offset += 8;
    }
    position_of_newline(&haystack[offset..]).map(|found| offset + found)
}

/// Offset of the first newline, byte by byte.
fn position_of_newline(haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|byte| *byte == b'\n')
}
