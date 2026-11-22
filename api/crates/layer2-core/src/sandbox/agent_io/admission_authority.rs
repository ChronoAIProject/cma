use super::*;

impl AgentCommandService {
    /// Revalidate operation visibility before returning an older adapter's
    /// exact receipt. Input validation and execution custody are deliberately
    /// separate: a historical validation/refusal receipt is still replayable.
    pub async fn authorize_intent(
        &self,
        context: &CallerContext,
        credential: &str,
        agent_id: &AgentId,
        command: &AgentCommand,
    ) -> Result<(), CommandAdmissionError> {
        if !context.validate_principal() || context.actor.ordinary_subject().is_none() {
            return Err(ResourceAccessError::Access(AccessError::NotFound).into());
        }
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let capability = command_capability(command);
        let authorized = bounded(self.access.authorize(
            context,
            Some(credential),
            &target,
            capability,
            AccessPurpose::Mutation {
                started_at: SystemTime::now(),
            },
        ))
        .await??;
        self.fence(&authorized, &target, capability).await
    }

    pub(in crate::sandbox::agent_io) async fn fence(
        &self,
        authorized: &AuthorizedResource,
        target: &ResourceTarget,
        capability: ResourceCapability,
    ) -> Result<(), CommandAdmissionError> {
        self.with_authority_bound(
            authorized,
            capability,
            self.access
                .validate_read_result(authorized, target, capability),
        )
        .await??;
        Ok(())
    }

    pub(in crate::sandbox::agent_io) async fn with_authority_bound<T>(
        &self,
        authorized: &AuthorizedResource,
        capability: ResourceCapability,
        work: impl std::future::Future<Output = T>,
    ) -> Result<T, CommandAdmissionError> {
        authorized
            .decision
            .require(capability)
            .map_err(ResourceAccessError::from)?;
        let deadline = authorized
            .decision
            .revalidation_deadline(capability)
            .map_or(tokio::time::Instant::now() + IO_BOUND, |deadline| {
                deadline.min(tokio::time::Instant::now() + IO_BOUND)
            });
        let result = tokio::time::timeout_at(deadline, work).await;
        authorized
            .decision
            .require(capability)
            .map_err(ResourceAccessError::from)?;
        result.map_err(|_| CommandLedgerError::Unavailable.into())
    }
}
