//! Reading the scope activation log: which scopes came on in which context, and
//! why.
//!
//! The log is written by the hook, the session tools and the context registry,
//! through the statistics writer; see [`crate::stats::activations`]. Both reads
//! here flush that writer first, so an answer covers every step answered before
//! it.

use std::collections::BTreeMap;

use crate::app::AppState;
use crate::context::ContextKey;
use crate::context::registry::AgentDescription;
use crate::stats::activations::{
    ActivationDetail, ActivationEntry, ActivationFilter, ActivationPage,
};

use super::OperationError;

/// One page of the log, newest or oldest first as `filter` asks, with the
/// cursor of the next page.
///
/// Each entry of a context the server still holds names the kind of subagent it
/// is and its task, where the context records them.
pub async fn scope_activations(
    state: &AppState,
    filter: ActivationFilter,
) -> Result<ActivationPage, OperationError> {
    if !state.activations.is_on() {
        return Err(OperationError::ActivationLogOff);
    }
    let mut page = crate::stats::read(&state.stats, &state.config.stats_path, move |reader| {
        reader.activations(&filter)
    })
    .await?;
    describe_agents(state, &mut page.entries).await;
    Ok(page)
}

/// One entry of the log with the whole text its trigger matched.
pub async fn scope_activation_get(
    state: &AppState,
    id: i64,
) -> Result<ActivationDetail, OperationError> {
    if !state.activations.is_on() {
        return Err(OperationError::ActivationLogOff);
    }
    let mut detail = crate::stats::read(&state.stats, &state.config.stats_path, move |reader| {
        reader.activation(id)
    })
    .await?
    .ok_or_else(|| OperationError::NotFound(format!("the activation log entry {id}")))?;
    describe_agents(state, std::slice::from_mut(&mut detail.entry)).await;
    Ok(detail)
}

/// Fill in the kind and the task of every entry's context that is a subagent
/// the registry still holds, reading each context once.
async fn describe_agents(state: &AppState, entries: &mut [ActivationEntry]) {
    let mut described: BTreeMap<String, Option<AgentDescription>> = BTreeMap::new();
    for entry in entries {
        if !described.contains_key(&entry.context) {
            let agent = match ContextKey::parse(&entry.context) {
                Ok(key) if key.is_subagent() => state.contexts.agent_of(&key).await,
                _ => None,
            };
            described.insert(entry.context.clone(), agent);
        }
        if let Some(Some(agent)) = described.get(&entry.context) {
            entry.agent_type = agent.agent_type.clone();
            entry.task = agent.task.clone();
        }
    }
}
