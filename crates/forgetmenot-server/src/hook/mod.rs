//! `POST /hook`: the endpoint every Claude Code hook event reaches.
//!
//! One event is one pass: take a catalog snapshot, enter the context's critical
//! section, turn on the scopes its triggers fire, work out what the context is
//! owed, decide whether to stop the tool call, and record what was delivered.
//! Everything that touches the store happens before the context lock is taken
//! and the statistics are written after it is released.

pub mod events;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use forgetmenot_types::hook::{HookEvent, HookRequest, HookResponse};
use git2::Oid;

use crate::app::AppState;
use crate::context::registry::Inheritance;
use crate::context::{ContextState, Needs, PreviousTexts, compute_needs, record_delivery};
use crate::render::{self, Delivery};
use crate::stats::{Decision, HookEventRecord, SHRUNK_REASON, TriggerFire};
use crate::store::catalog::Catalog;
use crate::store::memory::MemoryKind;
use crate::store::scope::TriggerField;
use crate::store::{MemoryId, ScopeId};
use events::EventPlan;

/// Why a tool call was stopped. Fixed text: the model has to be able to tell
/// this apart from a human refusing the call.
pub const DENY_REASON: &str = "This tool call was not executed. Its input matched a memory trigger and a critical memory is new or changed for this context. This is an automatic keyword match, not a review, approval or denial of the call. Read the memory in the additional context. Issue the call again if it is still what you intend.";

/// Answer one hook event.
pub async fn handle(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let started = Instant::now();

    let Ok(request) = serde_json::from_slice::<HookRequest>(&body) else {
        return (StatusCode::BAD_REQUEST, "the body is not a hook request").into_response();
    };
    let Ok(event) = serde_json::from_value::<HookEvent>(request.hook) else {
        return (StatusCode::BAD_REQUEST, "the hook event could not be read").into_response();
    };

    // The catalog is read before the event is planned, because the store's own
    // settings decide how the event is read and what it may do.
    let catalog = match state.store.snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::error!("the store could not be read: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the store could not be read",
            )
                .into_response();
        }
    };
    let settings = catalog.settings();

    // An event name this server does not know is acknowledged with an object
    // Claude Code reads as "nothing to do", not with an error.
    let Some(plan) = events::plan(&event, &request.machine, request.task.as_deref(), settings)
    else {
        return axum::Json(serde_json::json!({})).into_response();
    };

    let now = state.clock.now();
    let tokens_now = request.context_tokens;
    let named = SessionName {
        title: request.session_title.as_deref(),
        first_prompt: request.first_prompt.as_deref(),
    };
    let previous = previous_texts(&state, &plan, &catalog, now).await;
    let outcome = state
        .contexts
        .with_context(&plan.key, now, Inheritance::of(settings), |context| {
            apply(context, &plan, &catalog, tokens_now, now, named, &previous)
        })
        .await;

    let deny = plan.may_deny
        && settings.interrupts(plan.tool_name.as_deref())
        && outcome.needs.has_critical_arrival(&catalog);
    let delivery = Delivery {
        key: &plan.key,
        catalog: &catalog,
        needs: &outcome.needs,
        active: &outcome.active,
        activated: &outcome.activated,
        announce_empty_scopes: settings.announce_empty_scopes,
        session_start: plan.session_start,
        answer_file_threshold: settings.answer_file_threshold,
    };
    // A scope named because it delivered nothing is text and nothing else, so it
    // is a reason to answer at all but not a delivery: nothing is recorded
    // against the context and no call is held for it.
    let answered = !outcome.needs.is_empty() || !delivery.announced_scopes().is_empty();
    let response = if answered {
        let text = render::render(&delivery);
        if deny {
            HookResponse::deny(plan.event_name, DENY_REASON, text)
        } else {
            HookResponse::with_context(plan.event_name, text)
        }
    } else {
        HookResponse::empty()
    };

    let decision = match (deny, answered) {
        (true, _) => Decision::Deny,
        (false, true) => Decision::Context,
        (false, false) => Decision::None,
    };
    state
        .stats
        .record_hook_event(HookEventRecord {
            ts: now,
            machine: plan.key.machine.clone(),
            session_id: plan.key.session_id.clone(),
            agent: plan.key.agent.clone(),
            event: plan.event_name.to_string(),
            context_tokens: tokens_now,
            latency_us: started.elapsed().as_micros() as u64,
            decision,
            triggers: outcome.fires,
            deliveries: deliveries_of(&outcome.needs, &catalog),
        })
        .await;

    axum::Json(response).into_response()
}

/// The text this context was given for each memory the store now holds another
/// version of, read from the version the context holds.
///
/// One blob per memory whose version moved, so an event that meets an unchanged
/// store reads nothing. A session start is given everything afresh and has no
/// changed memory at all, so it reads nothing either. A version git no longer
/// has, or bytes that no longer parse as that memory, yield no text and the
/// memory is delivered whole.
///
/// This runs before the context's own critical section, so the versions it reads
/// are the ones the context held when the event arrived; a memory whose version
/// moves in between is delivered whole by [`compute_needs`], which compares the
/// versions itself.
async fn previous_texts(
    state: &AppState,
    plan: &EventPlan,
    catalog: &Catalog,
    now: chrono::DateTime<chrono::Utc>,
) -> PreviousTexts {
    if plan.session_start {
        return PreviousTexts::new();
    }
    let inheritance = Inheritance::of(catalog.settings());
    let outdated: Vec<(MemoryId, Oid, MemoryKind)> = state
        .contexts
        .with_context(&plan.key, now, inheritance, |context| {
            outdated_versions(context, catalog)
        })
        .await;

    let mut previous = PreviousTexts::new();
    for (id, version, kind) in outdated {
        let document = match state.store.memory_at_version(&id, version).await {
            Ok(Some(document)) => document,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!("version {version} of {id} could not be read: {error}");
                continue;
            }
        };
        // A memory whose kind changed is delivered in another form, so its two
        // texts are of different things and neither one holds the other's lines.
        if document.kind() != kind {
            continue;
        }
        previous.insert(id, document.delivered_text().to_string());
    }
    previous
}

/// Every memory this context holds at a version the catalog no longer has, with
/// the version it holds and the kind the catalog gives it now.
fn outdated_versions(
    context: &ContextState,
    catalog: &Catalog,
) -> Vec<(MemoryId, Oid, MemoryKind)> {
    context
        .delivered
        .iter()
        .filter_map(|(id, shown)| {
            let current = catalog.memory(id)?;
            if shown.version == current.version.to_string() {
                return None;
            }
            let version = Oid::from_str(&shown.version).ok()?;
            Some((id.clone(), version, current.kind()))
        })
        .collect()
}

/// What one event decided for its context.
struct Outcome {
    needs: Needs,
    /// The context's scopes after this event's triggers were applied.
    active: BTreeSet<ScopeId>,
    /// The scopes this event added to that set, the ones its triggers named and
    /// the ones those imply alike.
    activated: Vec<ScopeId>,
    fires: Vec<TriggerFire>,
}

/// What this event's client read from the session's transcript about what the
/// session is.
///
/// Both are absent when the client could not read the transcript, which is not
/// the same as the session having no name: see [`apply`].
#[derive(Clone, Copy, Debug)]
struct SessionName<'event> {
    title: Option<&'event str>,
    first_prompt: Option<&'event str>,
}

/// The whole critical section: begin a session's state, activate, compute,
/// record.
///
/// Pure, so that the decision a context makes depends on the catalog snapshot,
/// the state and the event alone.
fn apply(
    context: &mut ContextState,
    plan: &EventPlan,
    catalog: &Catalog,
    tokens_now: Option<u64>,
    now: chrono::DateTime<chrono::Utc>,
    named: SessionName<'_>,
    previous: &PreviousTexts,
) -> Outcome {
    // A session start makes the session's state anew: it is the one event that
    // says a context begins here, so nothing counts as delivered into it and
    // the scopes it carries are whatever the context already holds. For a
    // session id first seen at this event those are the implicit three the
    // registry created it with, and for one that is already known, such as a
    // session resumed or rebuilt from a summary, they are the scopes it was
    // working in.
    if plan.session_start {
        *context = ContextState::fresh(context.active.clone(), context.parent.clone(), now);
    }

    // The first event this context is seen at says where the session started.
    // A session start has just cleared it above, so it fills the directory in
    // from its own event in this same pass.
    if context.session_directory.is_none() {
        context.session_directory = plan.cwd.clone();
    }

    // What the session is, as the last event that could say anything said it.
    // A `None` is the client having read no transcript, not the user having
    // taken the name away, so it leaves what the context holds alone; the read
    // fails on every event whose transcript is missing, and the name must not
    // blink out of the contexts list when one does. This is after the state is
    // made anew above, so a session start fills the name back in from its own
    // event.
    if let Some(title) = named.title {
        context.session_title = Some(title.to_string());
    }
    if let Some(first_prompt) = named.first_prompt {
        context.first_prompt = Some(first_prompt.to_string());
    }
    // The task and the kind of subagent are kept the same way and for the same
    // reason: the event that carries one is not always the event that named the
    // context, and an event that carries neither says nothing about either.
    if let Some(task) = &plan.task {
        context.task = Some(task.clone());
    }
    if let Some(agent_type) = &plan.agent_type {
        context.agent_type = Some(agent_type.clone());
    }

    // The session directory is not in the plan's texts, because only the
    // context knows it; it joins them here for the events that match on a
    // directory at all.
    let mut texts = plan.texts.clone();
    if plan.matches_directories
        && let Some(started_in) = &context.session_directory
    {
        texts.push((TriggerField::SessionDirectory, started_in.clone()));
    }

    // The scopes this event finds the context in, against which the scopes it
    // turns on are counted below.
    let active_before = context.active.clone();

    // `fire` rather than `fire_closed` because the statistics record which
    // pattern fired; the implies closure is applied to the whole active set
    // afterwards, which has the same effect.
    let mut fires = Vec::new();
    for (field, text) in &texts {
        for hit in catalog.triggers().fire(*field, text, &plan.key.machine) {
            let activated_new = !context.active.contains(&hit.scope);
            context.active.insert(hit.scope.clone());
            fires.push(TriggerFire {
                scope_id: hit.scope.to_string(),
                field: hit.field.to_string(),
                pattern: hit.pattern,
                activated_new,
            });
        }
    }
    context.active = catalog.closure(&context.active);
    let activated: Vec<ScopeId> = context.active.difference(&active_before).cloned().collect();

    let needs = compute_needs(catalog, context, tokens_now, previous);
    record_delivery(context, &needs, catalog, tokens_now);
    context.last_seen = now;

    Outcome {
        needs,
        active: context.active.clone(),
        activated,
        fires,
    }
}

/// One statistics row per memory this event delivered, withdrew, or found had
/// only shrunk.
///
/// A shrunk memory is recorded with no bytes: the row exists so that the context
/// the rule did not cost is visible, and nothing was sent.
fn deliveries_of(needs: &Needs, catalog: &Catalog) -> Vec<crate::stats::Delivery> {
    let mut deliveries = Vec::new();
    for (ids, reason) in [
        (&needs.new, "new"),
        (&needs.changed, "changed"),
        (&needs.stale, "stale"),
        (&needs.shrunk, SHRUNK_REASON),
    ] {
        for id in ids {
            let kind = catalog
                .memory(id)
                .map(|memory| memory.kind())
                .unwrap_or(MemoryKind::Knowledge);
            deliveries.push(crate::stats::Delivery {
                memory: id.to_string(),
                kind: kind_name(kind).to_string(),
                form: crate::context::Form::for_kind(kind).as_str().to_string(),
                reason: reason.to_string(),
                bytes: match reason == SHRUNK_REASON {
                    true => 0,
                    false => render::delivery_bytes(catalog, id),
                },
            });
        }
    }
    for (id, why) in &needs.retracted {
        let kind = catalog
            .memory(id)
            .map(|memory| memory.kind())
            .unwrap_or(MemoryKind::Knowledge);
        deliveries.push(crate::stats::Delivery {
            memory: id.to_string(),
            kind: kind_name(kind).to_string(),
            // A withdrawal carries no content, so it has no form.
            form: "none".to_string(),
            reason: format!("retracted:{}", why.as_str().replace(' ', "-")),
            bytes: 0,
        });
    }
    deliveries
}

fn kind_name(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Critical => "critical",
        MemoryKind::Knowledge => "knowledge",
    }
}
