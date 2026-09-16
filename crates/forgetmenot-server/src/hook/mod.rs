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

use crate::app::AppState;
use crate::context::registry::Inheritance;
use crate::context::{ContextState, Needs, compute_needs, initial_active, record_delivery};
use crate::render::{self, Delivery};
use crate::stats::{Decision, HookEventRecord, TriggerFire};
use crate::store::ScopeId;
use crate::store::catalog::Catalog;
use crate::store::memory::MemoryKind;
use crate::store::scope::TriggerField;
use events::{EventPlan, Reset};

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
    let outcome = state
        .contexts
        .with_context(&plan.key, now, Inheritance::of(settings), |context| {
            apply(context, &plan, &catalog, tokens_now, now, named)
        })
        .await;

    let deny = plan.may_deny
        && settings.interrupts(plan.tool_name.as_deref())
        && outcome.needs.has_critical_arrival(&catalog);
    let response = if outcome.needs.is_empty() {
        HookResponse::empty()
    } else {
        let text = render::render(&Delivery {
            key: &plan.key,
            catalog: &catalog,
            needs: &outcome.needs,
            active: &outcome.active,
            session_start: plan.session_start,
            answer_file_threshold: settings.answer_file_threshold,
        });
        if deny {
            HookResponse::deny(plan.event_name, DENY_REASON, text)
        } else {
            HookResponse::with_context(plan.event_name, text)
        }
    };

    let decision = match (deny, outcome.needs.is_empty()) {
        (true, _) => Decision::Deny,
        (false, false) => Decision::Context,
        (false, true) => Decision::None,
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

/// What one event decided for its context.
struct Outcome {
    needs: Needs,
    /// The context's scopes after this event's triggers were applied.
    active: BTreeSet<ScopeId>,
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

/// The whole critical section: reset, activate, compute, record.
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
) -> Outcome {
    match plan.reset {
        Reset::Keep => {}
        Reset::Fresh => {
            *context = ContextState::fresh(
                initial_active(&plan.key.machine, &plan.key.session_id),
                context.parent.clone(),
                now,
            );
        }
        Reset::ClearDelivered => context.delivered.clear(),
    }

    // The first event this context is seen at says where the session started.
    // A `Reset::Fresh` above has just cleared it, so a session start fills it
    // in from its own event in this same pass.
    if context.session_directory.is_none() {
        context.session_directory = plan.cwd.clone();
    }

    // What the session is, as the last event that could say anything said it.
    // A `None` is the client having read no transcript, not the user having
    // taken the name away, so it leaves what the context holds alone; the read
    // fails on every event whose transcript is missing, and the name must not
    // blink out of the contexts list when one does. This is after the reset
    // above, so a session start that begins the record afresh fills the name
    // back in from its own event.
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

    let needs = if plan.deliver {
        let needs = compute_needs(catalog, context, tokens_now);
        record_delivery(context, &needs, catalog, tokens_now);
        needs
    } else {
        Needs::default()
    };
    context.last_seen = now;

    Outcome {
        needs,
        active: context.active.clone(),
        fires,
    }
}

/// One statistics row per memory this event delivered or withdrew.
fn deliveries_of(needs: &Needs, catalog: &Catalog) -> Vec<crate::stats::Delivery> {
    let mut deliveries = Vec::new();
    for (ids, reason) in [
        (&needs.new, "new"),
        (&needs.changed, "changed"),
        (&needs.stale, "stale"),
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
                bytes: render::delivery_bytes(catalog, id),
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
