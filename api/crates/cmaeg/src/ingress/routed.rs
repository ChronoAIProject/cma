#[path = "routed_compatibility.rs"]
mod compatibility;
#[path = "routed_custom.rs"]
mod custom;
#[path = "routed_github.rs"]
pub(crate) mod github;
#[path = "routed_launch.rs"]
mod launch;
#[path = "routed_observe.rs"]
mod observe;
#[path = "routed_problem.rs"]
mod problem;
use problem::{admission_problem, routing_problem};
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
    binding_preparation_enabled: bool,
    binding_replacement_bindings: BTreeSet<String>,
    pub(crate) quoted_context_enabled: bool,
    sync_delivery_enabled: bool,
    stream_monitor: Option<crate::turns::routed::StreamMonitor>,
    custom_media_providers: BTreeSet<String>,
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
            binding_preparation_enabled: false,
            binding_replacement_bindings: BTreeSet::new(),
            quoted_context_enabled: false,
            sync_delivery_enabled: false,
            stream_monitor: None,
            custom_media_providers: BTreeSet::new(),
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
    pub fn with_binding_preparation(
        mut self,
        enabled: bool,
        replacements: BTreeSet<String>,
    ) -> Self {
        self.binding_preparation_enabled = enabled;
        self.binding_replacement_bindings = replacements;
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
    pub fn with_sync_delivery(mut self, enabled: bool) -> Self {
        self.sync_delivery_enabled = enabled;
        self
    }
    pub(crate) fn with_stream_monitor(
        mut self,
        monitor: Option<crate::turns::routed::StreamMonitor>,
    ) -> Self {
        self.stream_monitor = monitor;
        self
    }
    pub fn with_custom_media_providers(mut self, providers: BTreeSet<String>) -> Self {
        self.custom_media_providers = providers;
        self
    }
    #[allow(clippy::too_many_arguments)]
    async fn admit_once(
        &self,
        request: &LaunchRequest,
        source: VerifiedIngress,
        observer: &dyn ActivityObserver,
        native_interaction: bool,
        message_plan: Option<super::routing_plan::RoutedPlan>,
        quote_projection: Option<u8>,
        github_fence: Option<&github::GithubAuthorityFence>,
    ) -> Result<Option<LaunchOutcome>, Problem> {
        if source.oversized {
            return Err(admission_problem(AdmissionError::TooLarge));
        }
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
                .map_err(routing_problem)?;
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
                custom::validate_media(request, &plan, &self.custom_media_providers)?;
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
                    Candidate::prepare_message_event_with_generation(
                        request,
                        &plan,
                        &source.body,
                        &self.keys,
                        (self.clock)(),
                        self.binding_preparation_enabled,
                    )
                } else if native_interaction {
                    Candidate::prepare_native_interaction_with_generation(
                        request,
                        &plan,
                        &source.body,
                        &self.keys,
                        (self.clock)(),
                        self.binding_preparation_enabled,
                    )
                } else if self.binding_preparation_enabled {
                    Candidate::prepare_with_binding_preparation(
                        request,
                        &plan,
                        &source.body,
                        &self.keys,
                        (self.clock)(),
                        quote_projection,
                        &self.binding_replacement_bindings,
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
                let committed = if let Some(github_fence) = github_fence {
                    let authority_guards = vec![
                        crate::records::AuthorityGuard {
                            owner: github_fence.owner.clone(),
                            kind: "app_integration",
                            id: github_fence.integration_id.clone(),
                            version: github_fence.integration_version,
                        },
                        crate::records::AuthorityGuard {
                            owner: github_fence.owner.clone(),
                            kind: "binding",
                            id: github_fence.binding_id.clone(),
                            version: github_fence.binding_version,
                        },
                    ];
                    self.plans
                        .admit_github(
                            &candidate,
                            request.channel.pointer.record_id.as_str(),
                            request.channel.version,
                            &authority_guards,
                        )
                        .await
                } else if matches!(
                    request.activity.source,
                    cmaeg_core::activity::SourceKind::Github { .. }
                ) {
                    Err(AdmissionError::Unavailable)
                } else {
                    self.plans.admit(&candidate).await
                }
                .map_err(admission_problem)?;
                // Only a durable winning snapshot is reported or scheduled.
                // winner, so do not report this candidate's trace as its trace.
                committed
            }
            Err(error) => return Err(admission_problem(error)),
        };
        self.report_admitted(request, &source, observer, committed)
            .await
    }
}

fn ordinary(request: &LaunchRequest) -> bool {
    request.consumer.is_none()
        && request.activity.routing.is_none()
        && !matches!(request.activity.kind, ActivityKind::Interaction(_))
}

fn validate_legacy_command(request: &LaunchRequest) -> Result<(), Problem> {
    if ordinary(request) {
        let definition = &request.channel.definition;
        cmaeg_core::routing::addressing::validate_legacy_command(
            definition,
            &request.activity,
            definition.source_of(&request.activity.source),
        )
        .map_err(routing_plan::PlanError::Routing)
        .map_err(routing_problem)?;
    }
    Ok(())
}
#[cfg(test)]
#[path = "routed_tests.rs"]
mod tests;
