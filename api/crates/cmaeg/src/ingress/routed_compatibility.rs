//! Replica-mode compatibility admission retains the original single launch.
use super::*;
use crate::ingress::launch::TurnLauncher;
use futures_util::future::BoxFuture;

impl RoutedIngress {
    pub(super) async fn admit_compatibility(
        &self,
        request: &LaunchRequest,
        source: VerifiedIngress,
        observer: &dyn ActivityObserver,
    ) -> Result<LaunchOutcome, Problem> {
        crate::ingress::admission_trace::attempt(
            request,
            self.admit_compatibility_observed(request, source, observer),
        )
        .await
    }

    async fn admit_compatibility_observed(
        &self,
        request: &LaunchRequest,
        mut source: VerifiedIngress,
        observer: &dyn ActivityObserver,
    ) -> Result<LaunchOutcome, Problem> {
        if source.oversized {
            return Err(admission_problem(AdmissionError::TooLarge));
        }
        // Compatibility never applies ordinary routing/quote projection to an
        // already authenticated callback or the original legacy activity.
        source.quote = crate::ingress::quoted_context::Intake::Legacy;
        self.observe_quote_receipt(request, &source, observer, None)
            .await?;
        let committed = match self
            .plans
            .replay_compatibility(
                request,
                &source.body,
                crate::channels::profile_assignment::requires_guard(
                    &request.channel.definition,
                    self.profile_writes_enabled,
                ),
            )
            .await
            .map_err(admission_problem)?
        {
            Some(plan) => plan,
            None => {
                validate_legacy_command(request)?;
                if !self.admission_enabled || !self.schedule.accepting() {
                    return Err(Problem::new(
                        ProblemCode::ProviderUnavailable,
                        503,
                        "routed_admission_paused",
                    ));
                }
                let (assigned, assignment) = self.assign_profile(request).await?;
                let request = &assigned;
                let mut candidate = Candidate::prepare_compatibility(
                    request,
                    &source.body,
                    &self.keys,
                    (self.clock)(),
                )
                .map_err(admission_problem)?;
                let preview = candidate
                    .compatibility_delivery_plan()
                    .map_err(admission_problem)?;
                if preview.targets.iter().any(|target| {
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
                custom::validate_media(request, &preview, &self.custom_media_providers)?;
                validate_routed_delivery_with_sync(
                    &request.channel,
                    &preview,
                    self.sync_delivery_enabled,
                )
                .map_err(|_| {
                    Problem::new(
                        ProblemCode::AdmissionRefused,
                        422,
                        "routed_delivery_unsupported",
                    )
                })?;
                candidate.profile_admission = assignment;
                self.plans
                    .admit_compatibility(&candidate, request)
                    .await
                    .map_err(admission_problem)?
            }
        };
        self.report_admitted(request, &source, observer, committed)
            .await?
            .ok_or_else(|| admission_problem(AdmissionError::Corrupt))
    }

    /// Inject only into ConnectionService's internal completion outbox. Its
    /// sealed reference attests the canonical Activity; no external body exists.
    pub fn replica_completion_launcher(
        self: &Arc<Self>,
        observer: Arc<dyn ActivityObserver>,
    ) -> Arc<dyn TurnLauncher> {
        Arc::new(CompletionLauncher {
            routed: self.clone(),
            observer,
        })
    }
}
struct CompletionLauncher {
    routed: Arc<RoutedIngress>,
    observer: Arc<dyn ActivityObserver>,
}
impl TurnLauncher for CompletionLauncher {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        Box::pin(async move {
            if !self.routed.plans.replicas_enabled()
                || !matches!(&request.activity.kind, ActivityKind::Event { ce, .. }
                    if ce.r#type == "gateway.connection.ready")
                || request.activity.actor.kind != cmaeg_core::activity::ActorKind::System
            {
                return LaunchOutcome::Refused(admission_problem(AdmissionError::Invalid));
            }
            let body = match serde_json::to_value(&request.activity) {
                Ok(value) => cmaeg_core::profile::canonical_json(value).into_bytes(),
                Err(_) => {
                    return LaunchOutcome::Refused(admission_problem(AdmissionError::Invalid))
                }
            };
            let source = VerifiedIngress {
                oversized: body.len() > MAX_SOURCE_BYTES,
                body,
                quote: crate::ingress::quoted_context::Intake::Legacy,
                attributes_signed: true,
                received_at: None,
            };
            self.routed
                .admit_compatibility(&request, source, self.observer.as_ref())
                .await
                .unwrap_or_else(LaunchOutcome::Refused)
        })
    }
}
