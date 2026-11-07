use super::*;

impl RoutedIngress {
    pub(crate) async fn launch_github_verified(
        &self,
        request: LaunchRequest,
        body: Vec<u8>,
        hold: OwnedSemaphorePermit,
        observer: &dyn ActivityObserver,
        resolver: &crate::channels::resolver::ChannelResolver,
    ) -> LaunchOutcome {
        if !matches!(
            request.activity.source,
            cmaeg_core::activity::SourceKind::Github { .. }
        ) {
            drop(hold);
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "github_source_required",
            ));
        }
        if !self.has_routed_source(&request) {
            drop(hold);
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::AdmissionRefused,
                422,
                "github_source_not_configured",
            ));
        }
        let channel_url_matches = resolver
            .record_client()
            .record_url(request.channel.pointer.record_id.as_str())
            .is_ok_and(|url| url.as_str() == request.channel.pointer.record_url);
        if channel_url_matches {
            let current = match resolver.resolve_current(request.channel.id.as_str()).await {
                Ok(channel) => channel,
                Err(_) => {
                    drop(hold);
                    return LaunchOutcome::Refused(Problem::new(
                        ProblemCode::AdmissionRefused,
                        422,
                        "github_channel_removed_before_admission",
                    ));
                }
            };
            if !github_fence_allows(
                request.channel.version,
                current.version,
                current
                    .definition
                    .source_of(&request.activity.source)
                    .is_some(),
            ) {
                drop(hold);
                return LaunchOutcome::Refused(Problem::new(
                    ProblemCode::AdmissionRefused,
                    422,
                    "github_channel_changed_before_admission",
                ));
            }
        }
        let verified = VerifiedIngress {
            oversized: body.len() > MAX_SOURCE_BYTES,
            body,
            attributes_signed: true,
            received_at: None,
            quote: super::super::quoted_context::Intake::Legacy,
        };
        match self.admit(&request, verified, observer, false, None).await {
            Ok(Some(outcome)) => {
                drop(hold);
                outcome
            }
            Ok(None) => {
                drop(hold);
                LaunchOutcome::Refused(Problem::new(
                    ProblemCode::AdmissionRefused,
                    422,
                    "github_routing_required",
                ))
            }
            Err(problem) => {
                drop(hold);
                LaunchOutcome::Refused(problem)
            }
        }
    }

    fn has_routed_source(&self, request: &LaunchRequest) -> bool {
        request
            .channel
            .definition
            .source_of(&request.activity.source)
            .is_some()
    }

    pub(crate) async fn task(
        &self,
        owner: &str,
        task_id: &str,
    ) -> Result<crate::github::Task, crate::github::Error> {
        let Some(plan) = self
            .plans
            .lookup_task(owner, task_id)
            .await
            .map_err(|_| crate::github::Error::Unavailable)?
        else {
            return Err(crate::github::Error::NotFound);
        };
        Ok(crate::github::Task {
            task_id: plan.admission_id,
            status: "accepted".into(),
            response_id: None,
            channel_id: plan.identity.channel_id.to_string(),
            record_id: plan.identity.record_id.to_string(),
            event_id: plan.identity.event_id.to_string(),
            target_count: plan.targets.len(),
            accepted_at: plan.admitted_at,
            retained_until: plan.retained_until,
            result: None,
        })
    }
}

fn github_fence_allows(snapshot_version: u64, current_version: u64, source_present: bool) -> bool {
    snapshot_version == current_version && source_present
}

#[cfg(test)]
mod tests {
    use super::github_fence_allows;

    #[test]
    fn route_removed_between_receipt_and_admission_is_refused() {
        assert!(!github_fence_allows(4, 5, true));
        assert!(!github_fence_allows(4, 4, false));
        assert!(github_fence_allows(4, 4, true));
    }
}
