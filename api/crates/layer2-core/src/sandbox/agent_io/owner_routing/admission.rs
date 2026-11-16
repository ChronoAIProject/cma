use super::*;

impl AgentCommandService {
    /// Admission returns durable facts; observation and execution are separate.
    pub async fn admit(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: CommandSubmission,
    ) -> Result<CommandStatusView, CommandAdmissionError> {
        self.admit_routed(context, credential, source, submission, None)
            .await
    }

    /// Retain the caller's exact three-state input-policy observation.
    pub async fn admit_at_binding_revision(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: CommandSubmission,
        binding_revision: Option<u64>,
    ) -> Result<CommandStatusView, CommandAdmissionError> {
        self.admit_routed(
            context,
            credential,
            source,
            submission,
            Some(binding_revision),
        )
        .await
    }

    async fn admit_routed(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        submission: CommandSubmission,
        binding_policy: Option<Option<u64>>,
    ) -> Result<CommandStatusView, CommandAdmissionError> {
        let mutation = OwnerCommandMutation::Admit {
            submission: submission.clone(),
            binding_policy,
        };
        // The unused local path must not enlarge every routed caller's future.
        match Box::pin(self.route_mutation(context, credential, source, mutation)).await? {
            Some(OwnerCommandReply::Admit(result)) => result,
            Some(_) => Err(CommandLedgerError::Unavailable.into()),
            None => {
                Box::pin(self.admit_inner(context, credential, source, submission, binding_policy))
                    .await
            }
        }
    }
}
