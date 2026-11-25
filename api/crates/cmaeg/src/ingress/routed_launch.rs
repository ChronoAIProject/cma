//! Verified transport dispatch and bounded source capture.
use super::*;

impl IngressState {
    /// Native inbox work is acknowledged only by durable routed admission.
    /// In particular, missing/disabled routing must never reach the legacy launcher.
    pub(crate) async fn launch_native_verified(
        &self,
        verified: crate::discord::VerifiedInboxLaunch,
        _hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        let request = verified.request;
        let native_interaction = crate::ingress::native_interaction::shape(&request.activity);
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
            quote: crate::ingress::quoted_context::Intake::Pinned(verified.quote_projection),
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
            .filter(|routing| {
                crate::channels::profile_assignment::requires_guard(
                    &request.channel.definition,
                    self.profile_revisions_enabled,
                ) || routing.plans.replicas_enabled()
                    || (routing.selected_request(request) && ordinary(request))
            })
            .map(|_| VerifiedIngress {
                quote: if matches!(
                    request.activity.source,
                    cmaeg_core::activity::SourceKind::Telegram { .. }
                ) {
                    crate::ingress::quoted_context::Intake::Candidate(
                        serde_json::from_slice(body).ok().and_then(|raw| {
                            crate::ingress::quoted_context::telegram(&raw, &request.activity)
                        }),
                    )
                } else {
                    crate::ingress::quoted_context::Intake::Legacy
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
        mut source: Option<VerifiedIngress>,
        hold: OwnedSemaphorePermit,
    ) -> LaunchOutcome {
        if self.profile_revisions_enabled {
            let current = match self
                .resolver
                .resolve_current(request.channel.id.as_str())
                .await
            {
                Ok(current) => current,
                Err(error) => return LaunchOutcome::Refused(error.problem()),
            };
            if current.version != request.channel.version || current.definition.mutation.is_some() {
                self.resolver.invalidate(request.channel.id.as_str());
                return LaunchOutcome::Refused(Problem::new(
                    ProblemCode::Conflict,
                    409,
                    "channel changed before event admission",
                ));
            }
        }
        if let Some(routing) = self
            .routed
            .as_ref()
            .filter(|routing| routing.selected_request(&request) && ordinary(&request))
        {
            let selected_source = if routing.plans.replicas_enabled()
                || crate::channels::profile_assignment::requires_guard(
                    &request.channel.definition,
                    self.profile_revisions_enabled,
                ) {
                source.clone()
            } else {
                source.take()
            };
            let Some(source) = selected_source else {
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
        if crate::ingress::telegram::member_joins::is_member_join(&request.activity) {
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "telegram_member_join_durable_admission_unavailable",
            ));
        }
        if let Some(routing) = self.routed.as_ref().filter(|r| {
            r.plans.replicas_enabled()
                || crate::channels::profile_assignment::requires_guard(
                    &request.channel.definition,
                    self.profile_revisions_enabled,
                )
        }) {
            let Some(source) = source else {
                return LaunchOutcome::Refused(Problem::new(
                    ProblemCode::Internal,
                    503,
                    "verified_ingress_bytes_unavailable",
                ));
            };
            return routing
                .admit_compatibility(&request, source, self.activity.as_ref())
                .await
                .unwrap_or_else(LaunchOutcome::Refused);
        }
        if ordinary(&request) && telegram::opted_in(&request) {
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "telegram_message_events_durable_admission_unavailable",
            ));
        }
        if let Some(refusal) = custom::refusal(&request) {
            return refusal;
        }
        if let Err(problem) = validate_legacy_command(&request) {
            return LaunchOutcome::Refused(problem);
        }
        self.launcher.launch_holding(request, hold).await
    }
}
