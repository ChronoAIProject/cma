use super::*;

pub(crate) struct GithubAuthorityFence {
    pub(crate) owner: String,
    pub(crate) integration_id: String,
    pub(crate) integration_version: u64,
    pub(crate) binding_id: String,
    pub(crate) binding_version: u64,
}

impl RoutedIngress {
    pub(crate) async fn launch_github_verified(
        &self,
        mut request: LaunchRequest,
        body: Vec<u8>,
        hold: OwnedSemaphorePermit,
        observer: &dyn ActivityObserver,
        resolver: &crate::channels::resolver::ChannelResolver,
        authority: GithubAuthorityFence,
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
        let current = match resolver.resolve_current(request.channel.id.as_str()).await {
            Ok(channel) => channel,
            Err(error) => {
                drop(hold);
                return LaunchOutcome::Refused(channel_restore_problem(&error));
            }
        };
        if current
            .definition
            .source_of(&request.activity.source)
            .is_none()
        {
            drop(hold);
            return LaunchOutcome::Refused(Problem::new(
                ProblemCode::AdmissionRefused,
                422,
                "github_channel_changed_before_admission",
            ));
        }
        request.channel = current;
        let verified = VerifiedIngress {
            oversized: body.len() > MAX_SOURCE_BYTES,
            body,
            attributes_signed: true,
            received_at: None,
            quote: super::super::quoted_context::Intake::Legacy,
        };
        match self
            .admit_github(&request, verified, observer, authority)
            .await
        {
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

    async fn admit_github(
        &self,
        request: &LaunchRequest,
        source: VerifiedIngress,
        observer: &dyn ActivityObserver,
        authority: GithubAuthorityFence,
    ) -> Result<Option<LaunchOutcome>, Problem> {
        self.admit_once(
            request,
            source,
            observer,
            false,
            None,
            None,
            Some(&authority),
        )
        .await
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

fn channel_restore_problem(error: &crate::channels::resolver::ResolveError) -> Problem {
    if matches!(
        error,
        crate::channels::resolver::ResolveError::NotFound
            | crate::channels::resolver::ResolveError::Tampered
    ) {
        Problem::new(
            ProblemCode::AdmissionRefused,
            422,
            "github_channel_removed_before_admission",
        )
    } else {
        Problem::new(
            ProblemCode::ProviderUnavailable,
            503,
            "github_channel_restore_unavailable",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::channel_restore_problem;
    use crate::channels::resolver::ResolveError;

    #[test]
    fn transient_channel_restore_errors_are_retryable() {
        assert_eq!(channel_restore_problem(&ResolveError::NotFound).status, 422);
        assert_eq!(
            channel_restore_problem(&ResolveError::Unavailable).status,
            503
        );
        assert_eq!(
            channel_restore_problem(&ResolveError::RateLimited).status,
            503
        );
    }
}
