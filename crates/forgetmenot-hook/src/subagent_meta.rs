//! Reading what a subagent was asked to do, and which agent asked it, out of
//! the metadata file Claude Code writes beside its transcript.
//!
//! A `SubagentStart` event names the subagent and its type and says nothing
//! about its task or about the agent that spawned it, so the only place either
//! can be read from is the file Claude Code writes for each subagent next to
//! the session's transcript:
//!
//! ```text
//! <dir>/<session_id>.jsonl                                  the session
//! <dir>/<session_id>/subagents/agent-<agent_id>.jsonl       the subagent
//! <dir>/<session_id>/subagents/agent-<agent_id>.meta.json   its metadata
//! ```
//!
//! The metadata file is a single JSON object,
//! `{"agentType":…,"description":…,"toolUseId":…,"spawnDepth":…,…}`, whose
//! `description` is the short label the parent gave the Agent tool. A subagent
//! spawned by another subagent carries `parentAgentId` as well, naming that
//! subagent; a subagent the session spawned carries no such key. The file is a
//! few hundred bytes, so it is read whole, once per event, and both values come
//! out of the one read.
//!
//! Which of the two transcripts a hook event inside a subagent reports is not
//! known, so both are handled: a `transcript_path` that is already a subagent's
//! own transcript has the metadata file beside it, and a session transcript has
//! it under the directory named after the session.
//!
//! Like [`crate::transcript_name`], every failure gives `None`: this runs on
//! Claude Code's critical path and must not fail a hook over a file it did not
//! write.

use std::path::{Path, PathBuf};

use crate::transcript_name::cut_to_characters;

/// How much of the task is kept. Same width as a first prompt, for the same
/// reason: enough to tell two subagents apart in a list, short enough that a
/// task with a pasted file in it does not travel with every hook event.
const TASK_CHARACTERS: usize = 200;

/// The directory Claude Code puts a session's subagent files in, under the
/// directory named after the session.
const SUBAGENTS_DIRECTORY: &str = "subagents";

/// What one subagent's metadata file says about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubagentMeta {
    /// The task the spawning agent gave it, cut to [`TASK_CHARACTERS`]
    /// characters. `None` when the file says none.
    pub task: Option<String>,
    /// The subagent that spawned it. `None` when the session spawned it, which
    /// is what a file carrying no such agent says.
    pub parent_agent_id: Option<String>,
}

/// What the metadata file of `agent_id` says, or an empty [`SubagentMeta`] when
/// there is no file to read it from.
///
/// `transcript_path` is the event's own, whether that is the session's
/// transcript or the subagent's.
pub fn read(transcript_path: impl AsRef<Path>, agent_id: &str) -> SubagentMeta {
    let Some(parsed) = parse(transcript_path.as_ref(), agent_id) else {
        return SubagentMeta::default();
    };
    SubagentMeta {
        task: text(&parsed, "description").map(|task| cut_to_characters(task, TASK_CHARACTERS)),
        parent_agent_id: text(&parsed, "parentAgentId").map(str::to_string),
    }
}

/// The metadata file of `agent_id` as JSON, or `None` when there is none to
/// read at either of the two places it can sit.
fn parse(transcript_path: &Path, agent_id: &str) -> Option<serde_json::Value> {
    let path = meta_path(transcript_path, agent_id)?;
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// One string field of the metadata, or `None` when it is absent, is not a
/// string, or is empty: a file with an empty description says no more about the
/// subagent than no file at all, and an empty id names no agent.
fn text<'json>(parsed: &'json serde_json::Value, field: &str) -> Option<&'json str> {
    parsed
        .get(field)?
        .as_str()
        .filter(|value| !value.is_empty())
}

/// Where the metadata file of `agent_id` is, given the transcript path an event
/// reported, or `None` when that path is not one a transcript can sit at.
///
/// The file is beside `transcript_path` when that is already inside a
/// `subagents` directory, and under `<session_id>/subagents/` beside it
/// otherwise.
fn meta_path(transcript_path: &Path, agent_id: &str) -> Option<PathBuf> {
    let file_name = format!("agent-{agent_id}.meta.json");
    let directory = transcript_path.parent()?;
    if directory
        .file_name()
        .is_some_and(|name| name == SUBAGENTS_DIRECTORY)
    {
        return Some(directory.join(file_name));
    }
    let session_id = transcript_path.file_stem()?;
    Some(
        directory
            .join(session_id)
            .join(SUBAGENTS_DIRECTORY)
            .join(file_name),
    )
}
