//! `POST /hook`: the endpoint every Claude Code hook event reaches.
//!
//! One event is one pass: take a catalog snapshot, enter the context's critical
//! section, turn on the scopes its triggers fire, work out what the context is
//! owed, decide whether to stop the tool call, and record what was delivered.
//! Everything that touches the store happens before the context lock is taken
//! and the statistics are written after it is released.

pub mod events;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use forgetmenot_types::hook::{HookEvent, HookRequest, HookResponse};
use git2::Oid;

use crate::app::AppState;
use crate::context::registry::Creation;
use crate::context::{ContextState, Needs, PreviousTexts, compute_needs, record_delivery};
use crate::render::{self, Delivery, Rendered};
use crate::service::Store;
use crate::stats::{Decision, ForgottenScope, HookEventRecord, SHRUNK_REASON, TriggerFire};
use crate::store::catalog::Catalog;
use crate::store::memory::MemoryKind;
use crate::store::scope::TriggerField;
use crate::store::settings::Settings;
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
    let Some(plan) = events::plan(
        &event,
        &request.machine,
        request.task.as_deref(),
        request.parent_agent_id.as_deref(),
        settings,
    ) else {
        return axum::Json(serde_json::json!({})).into_response();
    };

    let now = state.clock.now();
    let tokens_now = request.context_tokens;
    // The size an event reports belongs to the context whose transcript it was
    // read from, which is this event's own context except at a `SubagentStart`:
    // see [`EventPlan::tokens_are_the_parents`]. Where it is not the context's
    // own, nothing that counts tokens reads it, and the statistics row below
    // still records it as it arrived.
    let context_tokens = match plan.tokens_are_the_parents {
        true => None,
        false => tokens_now,
    };
    let named = SessionName {
        title: request.session_title.as_deref(),
        first_prompt: request.first_prompt.as_deref(),
    };
    let previous = previous_texts_of_event(&state, &plan, &catalog, now).await;
    let outcome = state
        .contexts
        .with_context(&plan.key, now, creation_of(&plan, settings), |context| {
            apply(
                context,
                &plan,
                &catalog,
                context_tokens,
                now,
                named,
                &previous,
            )
        })
        .await;

    let deny = plan.may_deny
        && settings.interrupts(plan.tool_name.as_deref())
        && outcome.needs.has_critical_arrival(&catalog);
    let delivery = Delivery {
        key: &plan.key,
        catalog: &catalog,
        needs: &outcome.needs,
        activated: &outcome.activated,
        announce_empty_scopes: settings.announce_empty_scopes,
        session_start: plan.session_start,
        answer_file_threshold: settings.answer_file_threshold,
    };
    // A scope named because it delivered nothing is text and nothing else, so it
    // is a reason to answer at all but not a delivery: nothing is recorded
    // against the context and no call is held for it.
    let answered = !outcome.needs.is_empty() || !delivery.announced_scopes().is_empty();
    // Rendered once: the text the model is sent and the accounting the
    // statistics record are the same pass, so what a scope is charged is the
    // text that was actually sent.
    let rendered = answered.then(|| render::render(&delivery));
    let response = match &rendered {
        Some(rendered) => {
            let text = rendered.text.clone();
            if deny {
                HookResponse::deny(plan.event_name, DENY_REASON, text)
            } else {
                HookResponse::with_context(plan.event_name, text)
            }
        }
        None => HookResponse::empty(),
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
            answer_chars: rendered.as_ref().map(Rendered::chars).unwrap_or_default(),
            latency_us: started.elapsed().as_micros() as u64,
            decision,
            triggers: outcome.fires,
            forgotten: outcome.forgotten,
            deliveries: deliveries_of(&outcome.needs, &catalog, rendered.as_ref()),
        })
        .await;

    axum::Json(response).into_response()
}

/// The text an event's context was given for each memory the store now holds
/// another version of.
///
/// A session start is given everything afresh and has no changed memory at all,
/// so it reads nothing. Everything else reads the state as it stands, before the
/// context's own critical section, so the versions it reads are the ones the
/// context held when the event arrived; a memory whose version moves in between
/// is delivered whole by [`compute_needs`], which compares the versions itself.
async fn previous_texts_of_event(
    state: &AppState,
    plan: &EventPlan,
    catalog: &Catalog,
    now: chrono::DateTime<chrono::Utc>,
) -> PreviousTexts {
    if plan.session_start {
        return PreviousTexts::new();
    }
    let held = state
        .contexts
        .with_context(
            &plan.key,
            now,
            creation_of(plan, catalog.settings()),
            |context| context.clone(),
        )
        .await;
    previous_texts(&state.store, &held, catalog).await
}

/// What the context this event acts on is created from, when the event is the
/// first thing seen for it.
///
/// Read at both places an event reaches its context, because either may be the
/// one that creates it: a `SubagentStart` reads what the child holds before it
/// works out what it is owed.
fn creation_of(plan: &EventPlan, settings: &Settings) -> Creation {
    Creation::spawned_by(settings, plan.parent.clone())
}

/// The text `context` was given for each memory the store now holds another
/// version of, read from the version the context holds.
///
/// One blob per memory whose version moved, so a context that meets an unchanged
/// store reads nothing. A version git no longer has, or bytes that no longer
/// parse as that memory, yield no text and the memory is delivered whole.
///
/// The one reader of what a context was told before, so that the hook and a
/// rendering that only looks at a context judge a shrunk memory from the same
/// text.
pub async fn previous_texts(
    store: &Store,
    context: &ContextState,
    catalog: &Catalog,
) -> PreviousTexts {
    let outdated: Vec<(MemoryId, Oid, MemoryKind)> = outdated_versions(context, catalog);
    let mut previous = PreviousTexts::new();
    for (id, version, kind) in outdated {
        let document = match store.memory_at_version(&id, version).await {
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
    /// The scopes this event turned on, the ones its triggers named and the ones
    /// those imply alike.
    activated: Vec<ScopeId>,
    fires: Vec<TriggerFire>,
    /// The scopes this event turned off because their `forget` rule was reached.
    forgotten: Vec<ForgottenScope>,
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

    // The context size this event reports, which is also the baseline of
    // everything this context holds that was recorded without one. It is taken
    // before anything below reads a count, so a delivery and a scope both count
    // from the first event of this context that carried a size.
    if let Some(tokens) = tokens_now {
        context.note_tokens(tokens, catalog);
    }

    // Forgetting comes before the triggers are matched, so a trigger that fires
    // at this very event restarts the count instead of being forgotten in the
    // same pass. What it leaves is read by `compute_needs` below, which
    // withdraws the memories no active scope covers any more.
    let forgotten = forget_scopes(context, catalog, tokens_now);

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
            context.note_activation(&hit.scope, tokens_now, catalog);
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
        activated,
        fires,
        forgotten,
    }
}

/// Turn off every scope of `context` whose `forget` rule is reached at
/// `tokens_now`, and report each one.
///
/// The one place a scope is forgotten. An event that carries no context size
/// forgets nothing: the rule counts tokens, and this event read none.
///
/// What a missing activation means is [`ContextState::note_tokens`]'s to say,
/// and it has given every active scope with a rule a baseline at this event's
/// tokens before this runs, so a scope with none here has met no event carrying
/// a size at all and there is nothing to count.
///
/// The scopes a forgotten scope implies stay on, as they do when a tool call
/// turns a scope off: a scope is a flag, and the state does not record which
/// implication turned it on.
fn forget_scopes(
    context: &mut ContextState,
    catalog: &Catalog,
    tokens_now: Option<u64>,
) -> Vec<ForgottenScope> {
    let Some(tokens_now) = tokens_now else {
        return Vec::new();
    };
    let mut forgotten = Vec::new();
    for scope in context.active.clone() {
        let Some(forget) = catalog.forget_rule(&scope) else {
            continue;
        };
        let Some(activated_at) = context.activated_at.get(&scope).copied() else {
            continue;
        };
        if tokens_now.saturating_sub(activated_at) < forget.tokens_since_trigger {
            continue;
        }
        context.active.remove(&scope);
        context.activated_at.remove(&scope);
        forgotten.push(ForgottenScope {
            scope_id: scope.to_string(),
            tokens_since_trigger: forget.tokens_since_trigger,
            tokens_at: tokens_now,
        });
    }
    forgotten
}

/// One statistics row per memory this event delivered, withdrew, or found had
/// only shrunk.
///
/// The scope and the size of a delivered memory are read out of `rendered`,
/// which is the answer that was sent: a memory's cost is the text printed for
/// it and the scope it is charged to is the section it was printed in, neither
/// of them worked out again here.
///
/// A withdrawal and a memory that only shrank put nothing in the answer, so
/// neither has a section or a size: the row exists so that the context the rule
/// did not cost is visible, and nothing was sent.
fn deliveries_of(
    needs: &Needs,
    catalog: &Catalog,
    rendered: Option<&Rendered>,
) -> Vec<crate::stats::Delivery> {
    let mut printed: BTreeMap<&MemoryId, (&ScopeId, u64, u64)> = BTreeMap::new();
    if let Some(rendered) = rendered {
        for section in &rendered.sections {
            for memory in &section.memories {
                printed.insert(&memory.id, (&section.scope, memory.chars, memory.bytes));
            }
        }
    }

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
            let (scope, chars, bytes) = match printed.get(id) {
                Some((scope, chars, bytes)) => (scope.to_string(), *chars, *bytes),
                None => (String::new(), 0, 0),
            };
            deliveries.push(crate::stats::Delivery {
                memory: id.to_string(),
                kind: kind_name(kind).to_string(),
                form: crate::context::Form::for_kind(kind).as_str().to_string(),
                reason: reason.to_string(),
                scope,
                chars,
                bytes,
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
            // A withdrawal carries no content, so it has no form, no section and
            // no size.
            form: "none".to_string(),
            reason: format!("retracted:{}", why.as_str().replace(' ', "-")),
            scope: String::new(),
            chars: 0,
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::context::{Form, Shown};
    use crate::store::catalog::Catalog;
    use crate::test_support::{TestStore, catalog_of, memory_file, scope_file, settings_file};
    use forgetmenot_types::hook::HookEvent;
    use serde_json::json;

    /// The context growth after which a delivery is stale and after which the
    /// scope below turns itself off. One number for both, so the sizes a test
    /// names are read against a single threshold.
    const THRESHOLD: u64 = 1_000;

    /// The size the first event of these tests reports, so that every later size
    /// is this plus a stated amount.
    const START: u64 = 10_000;

    /// A scope with a `forget` rule, which is what makes a context record when
    /// it was activated at all.
    const PASSING: &str = "passing";

    /// The text a prompt has to carry for the `passing` scope's trigger to
    /// fire, which is the only way a test here activates a scope.
    const RAIL: &str = "the rail";

    /// A store with one critical memory in `global`, one scope that a passing
    /// remark turns on and that turns itself off [`THRESHOLD`] tokens after its
    /// last activation, and reminders at the same threshold.
    fn store_files() -> Vec<(String, Option<Vec<u8>>)> {
        vec![
            settings_file(&format!("reminder_tokens: {THRESHOLD}\n")),
            scope_file(
                PASSING,
                &format!(
                    "forget:\n  tokens_since_trigger: {THRESHOLD}\n\
                     triggers:\n- on: user_message\n  pattern: '{RAIL}'\n"
                ),
            ),
            memory_file(
                "bench-power",
                "critical",
                "global",
                "Cut bench power at the wall",
                "# Cut bench power before rewiring\n\nSwitch the supply off at the wall.\n",
            ),
        ]
    }

    /// The catalog of a store built from [`store_files`], with the store it
    /// lives in, which is removed when the test ends.
    fn catalog() -> (TestStore, Catalog) {
        catalog_of(store_files())
    }

    /// The plan of a prompt saying `text`, from the directory `cwd`.
    fn prompt_plan(catalog: &Catalog, text: &str) -> EventPlan {
        let event: HookEvent = serde_json::from_value(json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "session-1",
            "prompt": text
        }))
        .expect("the payload parses as an event");
        events::plan(&event, "alpha", None, None, catalog.settings()).expect("the event is known")
    }

    /// The plan of an event that fires no trigger and stops nothing, so that
    /// what a test sees comes from the context size alone.
    fn quiet_event(catalog: &Catalog) -> EventPlan {
        prompt_plan(catalog, "carry on where we left off")
    }

    /// Answer one event of `context` at `tokens`, and report what it decided.
    fn at_tokens(context: &mut ContextState, catalog: &Catalog, tokens: u64) -> Outcome {
        answer(context, catalog, &quiet_event(catalog), Some(tokens))
    }

    /// Answer `plan` for `context` at `tokens`, as the handler does.
    fn answer(
        context: &mut ContextState,
        catalog: &Catalog,
        plan: &EventPlan,
        tokens: Option<u64>,
    ) -> Outcome {
        let named = SessionName {
            title: None,
            first_prompt: None,
        };
        apply(
            context,
            plan,
            catalog,
            tokens,
            chrono::Utc::now(),
            named,
            &PreviousTexts::new(),
        )
    }

    /// Detects a delivery recorded without a context size that is never stale
    /// again: a `memory_get` fetch and a write by the context itself report no
    /// size, so a rule read that way would be held for the rest of the session
    /// however far it fell out of the model's reach.
    ///
    /// Source: the rule that a count starts at the first event of the context
    /// that carries that context's own size. The first event reports 10 000, so
    /// that is the baseline: at 10 999 the growth is 999, one short of the
    /// threshold, and at 11 000 it is exactly the threshold and the memory is
    /// owed again.
    #[test]
    fn a_delivery_recorded_without_a_size_goes_stale_from_the_first_event_that_carries_one() {
        let (_directory, catalog) = catalog();
        let id = MemoryId::new("bench-power");
        let version = catalog
            .memory(&id)
            .expect("the store holds the memory")
            .version
            .to_string();
        let mut context = ContextState::fresh(
            BTreeSet::from([ScopeId::global()]),
            None,
            chrono::Utc::now(),
        );
        context.delivered.insert(
            id.clone(),
            Shown {
                version,
                form: Form::Full,
                tokens: None,
            },
        );

        let first = at_tokens(&mut context, &catalog, START);
        assert!(
            first.needs.stale.is_empty(),
            "the event that gives the delivery its baseline cannot also be the threshold past it, \
             got {:?}",
            first.needs.stale
        );

        let short = at_tokens(&mut context, &catalog, START + THRESHOLD - 1);
        assert!(
            short.needs.stale.is_empty(),
            "999 tokens past the baseline is one short of the threshold, got {:?}",
            short.needs.stale
        );

        let reached = at_tokens(&mut context, &catalog, START + THRESHOLD);
        assert_eq!(
            reached.needs.stale,
            vec![id],
            "the threshold past the first event that carried a size, the delivery is stale"
        );
    }

    /// Detects a scope whose activation was never recorded being kept for ever
    /// or dropped at once: a scope a subagent inherited, and one turned on by a
    /// tool call before any event reported a size, have no activation of their
    /// own, and the rule that turns them off counts tokens from somewhere.
    ///
    /// Source: the same rule. The scope's count starts at the first event that
    /// carries a size, 10 000 here, so it survives 10 999 and goes at 11 000.
    #[test]
    fn a_scope_with_no_recorded_activation_is_forgotten_from_the_first_sized_event() {
        let (_directory, catalog) = catalog();
        let scope = ScopeId::new(PASSING);
        let mut context = ContextState::fresh(
            BTreeSet::from([ScopeId::global(), scope.clone()]),
            None,
            chrono::Utc::now(),
        );
        assert!(
            context.activated_at.is_empty(),
            "this context inherited the scope, so nothing recorded when it came on"
        );

        let first = at_tokens(&mut context, &catalog, START);
        assert!(
            first.forgotten.is_empty() && context.active.contains(&scope),
            "the event the count starts at cannot also be the threshold past it"
        );

        let short = at_tokens(&mut context, &catalog, START + THRESHOLD - 1);
        assert!(
            short.forgotten.is_empty() && context.active.contains(&scope),
            "999 tokens past the first sized event is one short of the threshold"
        );

        let reached = at_tokens(&mut context, &catalog, START + THRESHOLD);
        assert_eq!(
            reached
                .forgotten
                .iter()
                .map(|forgotten| forgotten.scope_id.as_str())
                .collect::<Vec<_>>(),
            vec![PASSING],
            "the threshold past the first event that carried a size, the scope turns itself off"
        );
        assert!(
            !context.active.contains(&scope),
            "a forgotten scope is gone from the context's active scopes"
        );
    }

    /// Detects a scope turned on again after a forgetting that is not counted
    /// as an activation of its own: a scope whose subject keeps coming back is
    /// what the scopes report exists to show, and without the second count it
    /// reads as a scope that came on once and stayed on.
    ///
    /// Forgetting runs before the triggers of the same event, so the event at
    /// the threshold both drops the scope and turns it on again; that is the
    /// event the second activation has to be counted at.
    #[test]
    fn a_trigger_that_fires_after_a_forgetting_counts_as_an_activation_of_its_own() {
        let (_store, catalog) = catalog();
        let scope = ScopeId::new(PASSING);
        let mut context = ContextState::fresh(
            BTreeSet::from([ScopeId::global()]),
            None,
            chrono::Utc::now(),
        );
        let remark = prompt_plan(&catalog, &format!("a passing remark about {RAIL}"));

        let first = answer(&mut context, &catalog, &remark, Some(START));
        assert_eq!(
            first
                .fires
                .iter()
                .map(|fire| (fire.scope_id.as_str(), fire.activated_new))
                .collect::<Vec<_>>(),
            vec![(PASSING, true)],
            "the first match turns the scope on, which is one activation"
        );

        let again = answer(&mut context, &catalog, &remark, Some(START + THRESHOLD));

        assert_eq!(
            again
                .forgotten
                .iter()
                .map(|forgotten| forgotten.scope_id.as_str())
                .collect::<Vec<_>>(),
            vec![PASSING],
            "the threshold past the first match the scope turns itself off"
        );
        assert_eq!(
            again
                .fires
                .iter()
                .map(|fire| (fire.scope_id.as_str(), fire.activated_new))
                .collect::<Vec<_>>(),
            vec![(PASSING, true)],
            "the match at the same event turns it on again, which is a second activation"
        );
        assert!(
            context.active.contains(&scope),
            "the scope the same event turned on again must be active"
        );
    }

    /// Detects a `session_directory` trigger matched against the directory the
    /// shell is in at the time of the event, and a context whose first event is
    /// not a session start being left without a session directory at all: the
    /// field names where `claude` was started, which does not move when the
    /// agent runs `cd`, and a session already running when the server came up
    /// must still match directory triggers rather than silently matching none.
    ///
    /// The first event is a prompt, which reports a directory and matches
    /// against none, so the directory the context keeps can only have come from
    /// that event; the tool call is made from somewhere else, so a trigger that
    /// fires there saw the session's directory rather than the shell's.
    #[test]
    fn the_session_directory_a_trigger_matches_is_the_first_events_and_does_not_follow_the_shell() {
        const STARTED_IN: &str = "/start/project";
        const MOVED_TO: &str = "/moved/elsewhere";
        let (_store, catalog) = catalog_of(vec![
            scope_file(
                "bench",
                "triggers:\n- on: session_directory\n  pattern: '/start(/|$)'\n",
            ),
            scope_file(
                "shed",
                "triggers:\n- on: shell_directory\n  pattern: '/start(/|$)'\n",
            ),
        ]);
        let mut context = ContextState::fresh(
            BTreeSet::from([ScopeId::global()]),
            None,
            chrono::Utc::now(),
        );
        let mut prompt = quiet_event(&catalog);
        prompt.cwd = Some(STARTED_IN.to_string());

        answer(&mut context, &catalog, &prompt, Some(START));
        assert_eq!(
            context.session_directory.as_deref(),
            Some(STARTED_IN),
            "the first event a context is seen at says where the session began"
        );

        let call: HookEvent = serde_json::from_value(json!({
            "hook_event_name": "PreToolUse",
            "session_id": "session-1",
            "cwd": MOVED_TO,
            "tool_name": "Read",
            "tool_input": { "file_path": "/home/dev/notes/README.md" }
        }))
        .expect("the payload parses as an event");
        let call =
            events::plan(&call, "alpha", None, None, catalog.settings()).expect("PreToolUse");
        let moved = answer(&mut context, &catalog, &call, Some(START));

        assert!(
            moved.activated.contains(&ScopeId::new("bench")),
            "a session_directory trigger on {STARTED_IN} must fire at a call made from \
             {MOVED_TO}, got {:?}",
            moved.activated
        );
        assert!(
            !moved.activated.contains(&ScopeId::new("shed")),
            "a shell_directory trigger on {STARTED_IN} must not fire while the shell is in \
             {MOVED_TO}, got {:?}",
            moved.activated
        );
    }
}
