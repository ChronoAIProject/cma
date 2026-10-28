//! Durable admission for explicitly selected channel records. Legacy traffic
//! never performs a routing-store read or retains its raw body here.
#[path = "routed_native_lark.rs"]
mod native_lark;
#[path = "routed_native_lark_control.rs"]
mod native_lark_control;
#[path = "routed_quote.rs"]
mod quoted;
#[path = "routed_whatsapp.rs"]
mod whatsapp;
#[path = "routed_whatsapp_flow.rs"]
mod whatsapp_flow;

use super::{
    launch::{LaunchOutcome, LaunchRequest},
    routing_plan::{self, OrdinaryPlan, RoutingOptions},
    routing_store::{AdmissionError, Candidate, PlanStore, MAX_SOURCE_BYTES},
    IngressState,
};
use crate::turns::{
    observer::{ActivityObserver, LaunchClass},
    routed::{scheduler::ScheduleHandle, validate_routed_delivery_with_sync},
};
use cmaeg_core::{
    activity::ActivityKind,
    ids::RecordId,
    problem::{Problem, ProblemCode},
    seal::MasterKeyRing,
};
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::OwnedSemaphorePermit;

/// Constructed only after ingress authenticates the original request. Neither
/// its body nor its authentication facts come from a normalized event field.
#[derive(Clone)]
pub struct VerifiedIngress {
    quote: super::quoted_context::Intake,
    body: Vec<u8>,
    oversized: bool,
    attributes_signed: bool,
    received_at: Option<i64>,
}

#[path = "routed_message_event.rs"]
mod message_event;
#[path = "routed_telegram.rs"]
pub(crate) mod telegram;

pub struct RoutedIngress {
    plans: PlanStore,
    schedule: ScheduleHandle,
    keys: Arc<MasterKeyRing>,
    records: BTreeSet<String>,
    native_discovery: crate::discord::selection::Selection,
    non_human_providers: BTreeSet<String>,
    replay_providers: BTreeSet<String>,
    admission_enabled: bool,
    pub(crate) quoted_context_enabled: bool,
    sync_delivery_enabled: bool,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl RoutedIngress {
    pub fn new(
        plans: PlanStore,
        schedule: ScheduleHandle,
        keys: Arc<MasterKeyRing>,
        records: BTreeSet<String>,
        non_human_providers: BTreeSet<String>,
        replay_providers: BTreeSet<String>,
    ) -> Self {
        Self {
            plans,
            schedule,
            keys,
            records,
            native_discovery: Default::default(),
            non_human_providers,
            replay_providers,
            admission_enabled: true,
            quoted_context_enabled: false,
            sync_delivery_enabled: false,
            clock: Arc::new(|| time::OffsetDateTime::now_utc().unix_timestamp()),
        }
    }

    pub fn selected(&self, record: &RecordId) -> bool {
        self.records.contains(record.as_str())
    }

    pub fn with_native_discovery(
        mut self,
        selection: crate::discord::selection::Selection,
    ) -> Self {
        self.native_discovery = selection;
        self
    }

    pub(crate) fn native_eligible(
        &self,
        definition: &cmaeg_core::channel::ChannelDefinition,
    ) -> bool {
        definition.routing.is_some()
            && definition
                .consumers()
                .filter(|consumer| !consumer.pending)
                .all(|consumer| {
                    self.replay_providers
                        .contains(consumer.provider.slug.as_str())
                })
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_quoted_context(mut self, enabled: bool) -> Self {
        self.quoted_context_enabled = enabled;
        self
    }

    pub fn with_admissions_enabled(mut self, enabled: bool) -> Self {
        self.admission_enabled = enabled;
        self
    }
    /// Enable only when execution shares the ingress SyncRegistry through
    /// DeliveryCoordinator::with_custom_stream.
    pub fn with_sync_delivery(mut self, enabled: bool) -> Self {
        self.sync_delivery_enabled = enabled;
        self
    }

    async fn admit_once(
        &self,
        request: &LaunchRequest,
        source: VerifiedIngress,
        observer: &dyn ActivityObserver,
        native_interaction: bool,
        message_plan: Option<super::routing_plan::RoutedPlan>,
        quote_projection: Option<u8>,
    ) -> Result<Option<LaunchOutcome>, Problem> {
        if source.oversized {
            return Err(admission_problem(AdmissionError::TooLarge));
        }
        // Restore committed selection before looking at mutable routing policy.
        let replay = if message_plan.is_some() {
            self.plans.replay_message_event(request, &source.body).await
        } else if native_interaction {
            self.plans
                .replay_native_interaction(request, &source.body)
                .await
        } else {
            self.plans.replay(request, &source.body).await
        };
        let committed = match replay {
            Ok(Some(plan)) => plan,
            Ok(None) => {
                let decision = if let Some(plan) = message_plan.as_ref() {
                    Ok(OrdinaryPlan::Routed(Box::new(plan.clone())))
                } else if native_interaction {
                    super::native_interaction::plan(request, &self.keys, (self.clock)())
                        .map(|plan| OrdinaryPlan::Routed(Box::new(plan)))
                } else {
                    routing_plan::plan(
                        request,
                        &self.keys,
                        (self.clock)(),
                        RoutingOptions {
                            enabled: true,
                            attributes_signed: source.attributes_signed,
                            non_human_providers: &self.non_human_providers,
                        },
                    )
                }
                .map_err(|_| {
                    Problem::new(
                        ProblemCode::AdmissionRefused,
                        403,
                        "routing_admission_refused",
                    )
                })?;
                let OrdinaryPlan::Routed(plan) = decision else {
                    return Ok(None);
                };
                if !self.admission_enabled || !self.schedule.accepting() {
                    return Err(Problem::new(
                        ProblemCode::ProviderUnavailable,
                        503,
                        "routed_admission_paused",
                    ));
                }
                if plan.targets.iter().any(|target| {
                    !self
                        .replay_providers
                        .contains(target.provider.slug.as_str())
                }) {
                    return Err(Problem::new(
                        ProblemCode::ProviderUnavailable,
                        503,
                        "routed_provider_replay_unavailable",
                    ));
                }
                validate_routed_delivery_with_sync(
                    &request.channel,
                    &plan,
                    self.sync_delivery_enabled,
                )
                .map_err(|_| {
                    Problem::new(
                        ProblemCode::AdmissionRefused,
                        422,
                        "routed_delivery_unsupported",
                    )
                })?;
                let candidate = if message_plan.is_some() {
                    Candidate::prepare_message_event(
                        request,
                        &plan,
                        &source.body,
                        &self.keys,
                        (self.clock)(),
                    )
                } else if native_interaction {
                    Candidate::prepare_native_interaction(
                        request,
                        &plan,
                        &source.body,
                        &self.keys,
                        (self.clock)(),
                    )
                } else {
                    Candidate::prepare_projected(
                        request,
                        &plan,
                        &source.body,
                        &self.keys,
                        (self.clock)(),
                        quote_projection,
                    )
                }
                .map_err(admission_problem)?;
                let committed = self
                    .plans
                    .admit(&candidate)
                    .await
                    .map_err(admission_problem)?;
                // Only a durable winning snapshot is reported or scheduled.
                // Concurrent configuration changes can select a different
                // winner, so do not report this candidate's trace as its trace.
                committed
            }
            Err(error) => return Err(admission_problem(error)),
        };
        if !matches!(source.quote, super::quoted_context::Intake::Legacy) {
            self.observe_quote_receipt(request, &source, observer, Some(&committed.activity))
                .await?;
        }
        let primary = committed.targets.first().ok_or_else(|| {
            Problem::new(ProblemCode::Internal, 503, "routed_admission_unavailable")
        })?;
        // Admission can restore a previous winner, including a concurrent
        // request with another local waiter. Never promise an unattached SSE
        // stream or rewrite the winning plan to the retry's ephemeral waiter.
        let sync_waiter_attached = request.event_ref.open(&self.keys, (self.clock)())
            .ok().is_some_and(|opened| matches!(
                (&opened.payload.reply, &primary.launch.event.reply),
                (cmaeg_core::seal::ReplyAddress::Sync { waiter: incoming },
                 cmaeg_core::seal::ReplyAddress::Sync { waiter: retained }) if incoming == retained
            ));
        observer.admitted(&committed.identity.record_id, &committed.activity);
        for target in &committed.targets {
            let mut change = crate::activity::Change::state(
                target.launch.consumer.as_str(),
                crate::activity::ConsumerState::Queued,
                None,
            );
            change.provider = Some(target.launch.provider.slug.to_string());
            change.conversation_id = Some(target.launch.event.conversation_id.to_string());
            change.display_name = request
                .channel
                .definition
                .consumer(Some(&target.launch.consumer))
                .map(|c| c.label.to_owned());
            observer
                .checkpoint(
                    &committed.identity.record_id,
                    &committed.identity.event_id,
                    change,
                )
                .await;
            observer.launched(
                &committed.identity.record_id,
                &committed.identity.event_id,
                &target.launch.consumer,
                LaunchClass::Deferred,
                request.received_at.elapsed(),
            );
        }
        // A full hint queue leaves the manifest authoritative for recovery.
        self.schedule.notify(&committed);
        Ok(Some(LaunchOutcome::Admitted {
            admission_id: committed.admission_id,
            conversation_id: primary.launch.event.conversation_id.clone(),
            sync_waiter_attached,
        }))
    }
}

impl IngressState {
    /// Native inbox work is acknowledged only by durable routed admission.
    /// In particular, missing/disabled routing must never reach the legacy launcher.
    pub(crate) async fn launch_native_verified(
        &self,
        verified: crate::discord::VerifiedInboxLaunch,
        _hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        let request = verified.request;
        let native_interaction = super::native_interaction::shape(&request.activity);
        if !matches!(
            request.activity.source,
            cmaeg_core::activity::SourceKind::Discord { .. }
        ) {
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "native_discord_source_required",
            ));
        }
        let Some(routing) = self.routed.as_ref().filter(|routing| {
            (routing.selected(&request.channel.definition.record_id)
                || (routing.native_discovery.discovery_enabled()
                    && routing
                        .native_discovery
                        .contains(request.channel.definition.record_id.as_str())))
                && (ordinary(&request) || native_interaction)
        }) else {
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "native_discord_durable_admission_unavailable",
            ));
        };
        let source = VerifiedIngress {
            quote: super::quoted_context::Intake::Pinned(verified.quote_projection),
            oversized: verified.source_bytes.len() > MAX_SOURCE_BYTES,
            body: verified.source_bytes,
            attributes_signed: true,
            received_at: Some(verified.received_at),
        };
        match routing
            .admit(
                &request,
                source,
                self.activity.as_ref(),
                native_interaction,
                None,
            )
            .await
        {
            Ok(Some(outcome @ LaunchOutcome::Admitted { .. })) => outcome,
            Ok(_) => LaunchOutcome::Refused(Problem::new(
                ProblemCode::AdmissionRefused,
                422,
                "native_discord_routing_required",
            )),
            Err(problem) => LaunchOutcome::Refused(problem),
        }
    }

    pub fn capture_routed(
        &self,
        request: &LaunchRequest,
        body: &[u8],
        attributes_signed: bool,
    ) -> Option<VerifiedIngress> {
        self.routed
            .as_ref()
            .filter(|routing| routing.selected_request(request))
            .filter(|_| ordinary(request))
            .map(|_| VerifiedIngress {
                quote: if matches!(
                    request.activity.source,
                    cmaeg_core::activity::SourceKind::Telegram { .. }
                ) {
                    super::quoted_context::Intake::Candidate(
                        serde_json::from_slice(body).ok().and_then(|raw| {
                            super::quoted_context::telegram(&raw, &request.activity)
                        }),
                    )
                } else {
                    super::quoted_context::Intake::Legacy
                },
                body: if body.len() <= MAX_SOURCE_BYTES {
                    body.to_vec()
                } else {
                    Vec::new()
                },
                oversized: body.len() > MAX_SOURCE_BYTES,
                attributes_signed,
                received_at: None,
            })
    }

    /// Holds bounded ingress capacity until durable admission is confirmed.
    /// It never acknowledges a partial upload or starts a detached provider call.
    pub async fn launch_verified(
        &self,
        request: LaunchRequest,
        source: Option<VerifiedIngress>,
        hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        if let Some(routing) = self
            .routed
            .as_ref()
            .filter(|routing| routing.selected_request(&request) && ordinary(&request))
        {
            let Some(source) = source else {
                return LaunchOutcome::Refused(Problem::new(
                    ProblemCode::Internal,
                    503,
                    "verified_ingress_bytes_unavailable",
                ));
            };
            match routing
                .admit(&request, source, self.activity.as_ref(), false, None)
                .await
            {
                Ok(Some(outcome)) => return outcome,
                Ok(None) => {}
                Err(error) => return LaunchOutcome::Refused(error),
            }
        }
        if ordinary(&request) && telegram::opted_in(&request) {
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "telegram_message_events_durable_admission_unavailable",
            ));
        }
        self.launcher.launch_holding(request, hold).await
    }
}

fn ordinary(request: &LaunchRequest) -> bool {
    request.consumer.is_none()
        && request.activity.routing.is_none()
        && !matches!(request.activity.kind, ActivityKind::Interaction(_))
}

fn admission_problem(error: AdmissionError) -> Problem {
    let (code, status, detail) = match error {
        AdmissionError::Conflict => (ProblemCode::Conflict, 409, "routed_event_conflict"),
        AdmissionError::TooLarge => (ProblemCode::PayloadTooLarge, 413, "routed_event_too_large"),
        AdmissionError::Invalid => (ProblemCode::InvalidRequest, 400, "routed_event_invalid"),
        AdmissionError::Expired => (ProblemCode::EventRefExpired, 410, "routed_event_expired"),
        _ => (
            ProblemCode::ProviderUnavailable,
            503,
            "routed_admission_unavailable",
        ),
    };
    Problem::new(code, status, detail)
}

#[cfg(test)]
#[path = "routed_tests.rs"]
mod tests;
