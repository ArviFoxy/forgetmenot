//! Reading the context size out of a Claude Code transcript.
//!
//! A transcript is a JSONL file that grows for the whole life of a session and
//! reaches tens of megabytes. Only its last assistant line matters here, so
//! the file is read backwards in windows instead of being read whole: a hook
//! handler runs on every event and its whole budget is milliseconds.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Size of the first window read from the end of the file. A transcript line
/// with a full tool result can be large, so the window is generous; it doubles
/// when no assistant usage record is found inside it.
const INITIAL_WINDOW_BYTES: u64 = 256 * 1024;

/// Context size in tokens at the last assistant message of the transcript at
/// `path`, or `None` if there is no such record.
///
/// The context size is the sum of the last assistant message's
/// `input_tokens`, `cache_read_input_tokens` and `cache_creation_input_tokens`:
/// the cached parts of the prompt are in context but are not counted in
/// `input_tokens`. A counter Claude Code did not write counts as zero.
///
/// `None` means "nothing to compare against", not zero: a missing or
/// unreadable file, a transcript with no assistant line, and a transcript
/// whose assistant lines carry no usage record all give `None`.
pub fn last_assistant_context_tokens(path: impl AsRef<Path>) -> Option<u64> {
    let mut file = File::open(path.as_ref()).ok()?;
    let file_length = file.metadata().ok()?.len();

    let mut window = INITIAL_WINDOW_BYTES;
    loop {
        let start = file_length.saturating_sub(window);
        let mut buffer = vec![0u8; (file_length - start) as usize];
        file.seek(SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut buffer).ok()?;

        // A window that does not start at the beginning of the file starts in
        // the middle of a line; that fragment belongs to the next window.
        let complete = if start == 0 {
            &buffer[..]
        } else {
            match buffer.iter().position(|byte| *byte == b'\n') {
                Some(newline) => &buffer[newline + 1..],
                None => &[],
            }
        };

        if let Some(tokens) = last_assistant_context_tokens_in(complete) {
            return Some(tokens);
        }
        if start == 0 {
            return None;
        }
        window = window.saturating_mul(2);
    }
}

/// The same sum, over a byte range that contains whole lines. A line that is
/// not valid JSON is ignored, which is also what skips the partial line a
/// writer may have left at the end of the file.
fn last_assistant_context_tokens_in(lines: &[u8]) -> Option<u64> {
    lines
        .split(|byte| *byte == b'\n')
        .rev()
        .filter_map(assistant_context_tokens_in_line)
        .next()
}

/// The sum for one transcript line, or `None` if the line is not an assistant
/// line carrying a usage record.
fn assistant_context_tokens_in_line(line: &[u8]) -> Option<u64> {
    let parsed: serde_json::Value = serde_json::from_slice(line).ok()?;
    if parsed.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let usage = parsed.get("message")?.get("usage")?.as_object()?;
    let counter = |name: &str| {
        usage
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Some(
        counter("input_tokens")
            + counter("cache_read_input_tokens")
            + counter("cache_creation_input_tokens"),
    )
}
