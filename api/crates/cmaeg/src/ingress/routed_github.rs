use super::*;

impl RoutedIngress {
    pub(crate) async fn launch_github_verified(
        &self,
        request: LaunchRequest,
        body: Vec<u8>,
        hold: OwnedSemaphorePermit,
        observer: &dyn ActivityObserver,
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
