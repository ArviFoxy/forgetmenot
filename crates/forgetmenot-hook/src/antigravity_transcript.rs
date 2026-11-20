//! Reading the context size and the model's last message out of an
//! Antigravity transcript.
//!
//! Antigravity writes one JSON object per step of a conversation to the file
//! its hook events name in `transcriptPath`. A step of the model is
//! `{"source":"MODEL","type":"PLANNER_RESPONSE","content":…,"input_tokens":…,
//! "cache_read_tokens":…,"output_tokens":…,…}`; a step that only calls tools
//! has an empty `content`. These are the lines Antigravity 1.5.12 wrote on
//! 2026-11-20.
//!
//! Like a Claude Code transcript the file grows for the whole life of a
//! conversation, so it is read backwards in windows: see
//! [`crate::transcript_tail`]. Every failure gives `None`, because Antigravity
//! waits for this process.

use std::path::Path;

use crate::transcript_tail::last_line_with;

/// The `type` of a step in which the model answered.
const MODEL_STEP: &str = "PLANNER_RESPONSE";

/// The bytes of the model's last message that are sent to the server. The text
/// is matched against triggers and never shown, so a cut one still does its
/// work, and a hook event must stay small.
const MESSAGE_BYTES: usize = 64 * 1024;

/// Context size in tokens at the last model step of the transcript at `path`,
/// or `None` if there is no such step.
///
/// The context size is the sum of the step's `input_tokens` and
/// `cache_read_tokens`: the cached part of the prompt is in context and is not
/// counted in `input_tokens`. A counter Antigravity did not write counts as
/// zero, and a model step with neither counter is not an answer.
pub fn last_model_context_tokens(path: impl AsRef<Path>) -> Option<u64> {
    last_line_with(path, |line| {
        let step = model_step(line)?;
        let counter = |name: &str| step.get(name).and_then(serde_json::Value::as_u64);
        match (counter("input_tokens"), counter("cache_read_tokens")) {
            (None, None) => None,
            (input, cached) => Some(input.unwrap_or(0) + cached.unwrap_or(0)),
        }
    })
}

/// The text of the last model step that said anything, cut to
/// [`MESSAGE_BYTES`] on a character boundary, or `None` if no step did.
pub fn last_model_message(path: impl AsRef<Path>) -> Option<String> {
    last_line_with(path, |line| {
        let step = model_step(line)?;
        let content = step.get("content")?.as_str()?;
        if content.trim().is_empty() {
            return None;
        }
        let mut end = content.len().min(MESSAGE_BYTES);
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        Some(content[..end].to_string())
    })
}

/// The parsed line, if it is a step in which the model answered.
fn model_step(line: &[u8]) -> Option<serde_json::Value> {
    let step: serde_json::Value = serde_json::from_slice(line).ok()?;
    (step.get("type")?.as_str()? == MODEL_STEP).then_some(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A transcript holding `lines`, one per line, and the file that keeps it.
    fn transcript(lines: &[serde_json::Value]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("a temporary file");
        for line in lines {
            writeln!(file, "{line}").expect("the line is written");
        }
        file
    }

    fn user(text: &str) -> serde_json::Value {
        serde_json::json!({
            "step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT",
            "status": "DONE", "content": format!("<USER_REQUEST>\n{text}\n</USER_REQUEST>")
        })
    }

    fn model(content: &str, input: u64, cached: u64, output: u64) -> serde_json::Value {
        serde_json::json!({
            "step_index": 1, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE",
            "model": "gemini-example", "input_tokens": input, "cache_read_tokens": cached,
            "output_tokens": output, "content": content, "thinking": ""
        })
    }

    // Detects a context size that leaves out the cached part of the prompt, or
    // that counts the model's own output: with a warm cache almost the whole
    // prompt is cached, so the first would report a context a tenth of its
    // size and no delivery would ever go stale.
    // Expectation source: 4,302 uncached and 31,074 cached tokens are a prompt
    // of 35,376, written out here.
    #[test]
    fn the_context_size_is_the_uncached_and_the_cached_prompt_of_the_last_model_step() {
        let file = transcript(&[
            user("first"),
            model("an answer", 30_735, 0, 196),
            user("second"),
            model("another answer", 4_302, 31_074, 493),
            serde_json::json!({ "source": "SYSTEM_SDK", "type": "SYSTEM_MESSAGE", "content": "a note" }),
        ]);

        assert_eq!(last_model_context_tokens(file.path()), Some(35_376));
    }

    // Detects a missing transcript, or one the model has not answered in yet,
    // being reported as a context of zero, which the server would read as a
    // size and count every delivery from.
    #[test]
    fn a_transcript_without_a_model_step_has_no_context_size_and_no_message() {
        let file = transcript(&[user("first")]);
        assert_eq!(last_model_context_tokens(file.path()), None);
        assert_eq!(last_model_message(file.path()), None);
        assert_eq!(
            last_model_context_tokens("/nonexistent/transcript.jsonl"),
            None
        );
    }

    // Detects the last message taken from a step that only called a tool: such
    // a step has an empty `content`, and the turn's words are in the step
    // before it.
    #[test]
    fn the_last_message_is_the_last_model_step_that_said_something() {
        let file = transcript(&[
            user("first"),
            model("I will look at the widgets.", 100, 0, 10),
            model("", 120, 0, 5),
        ]);

        assert_eq!(
            last_model_message(file.path()).as_deref(),
            Some("I will look at the widgets.")
        );
    }

    // Detects a cut in the middle of a character, which would make the text
    // invalid and the request unsendable, and a message sent whole whatever its
    // size.
    #[test]
    fn a_long_last_message_is_cut_on_a_character_boundary() {
        let long = "\u{00e9}".repeat(MESSAGE_BYTES);
        let file = transcript(&[model(&long, 100, 0, 10)]);

        let message = last_model_message(file.path()).expect("the step said something");

        assert!(
            message.len() <= MESSAGE_BYTES,
            "got {} bytes",
            message.len()
        );
        assert!(long.starts_with(&message));
        assert!(!message.is_empty());
    }
}
