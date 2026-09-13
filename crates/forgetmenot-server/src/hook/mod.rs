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
    let Some(plan) = events::plan(&event, &request.machine, settings) else {
        return axum::Json(serde_json::json!({})).into_response();
    };

    let now = state.clock.now();
    let tokens_now = request.context_tokens;
    let outcome = state
        .contexts
        .with_context(&plan.key, now, Inheritance::of(settings), |context| {
            apply(context, &plan, &catalog, tokens_now, now)
        })
        .await;

    let deny = plan.may_deny
        && settings.interrupts(plan.tool_name.as_deref())
        && outcome.needs.has_critical_arrival(&catalog);
    let response = if outcome.needs.is_empty() {
        HookResponse::empty(plan.event_name)
    } else {
        let text = render::render(&Delivery {
            key: &plan.key,
            catalog: &catalog,
            needs: &outcome.needs,
            active: &outcome.active,
            session_start: plan.session_start,
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

    // `fire` rather than `fire_closed` because the statistics record which
    // pattern fired; the implies closure is applied to the whole active set
    // afterwards, which has the same effect.
    let mut fires = Vec::new();
    for (field, text) in &plan.texts {
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
