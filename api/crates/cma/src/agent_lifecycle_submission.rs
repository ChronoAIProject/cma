use super::*;

impl InlineAgentLifecycle {
    pub(super) async fn submit(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        delegated_credential: Option<&str>,
        request: AgentLifecycleCreateRequest,
        idempotency_key: &str,
        submission: Option<layer1_public::rest::creation_outcomes::Submission>,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError> {
        let replay_operation = submission
            .as_ref()
            .and_then(|submission| submission.replay_operation_id.as_deref());
        if layer1_public::rest::creation_outcomes::is_covered_key(idempotency_key)
            && replay_operation.is_none()
            && submission
                .as_ref()
                .is_none_or(|submission| submission.fence.is_none())
        {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        if self.tasks.is_closed() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        if crate::store::canary_key::is_reserved(idempotency_key) {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "The idempotency key is reserved.".into(),
            ));
        }
        validate_request(&request)?;
        caller
            .authorization
            .allows(Capability::UseOwnSandboxes)
            .then_some(())
            .ok_or(AgentLifecycleProvisioningError::Forbidden)?;
        if request.workspace_id.is_none()
            && request.sandbox_id.is_none()
            && !caller.authorization.allows(Capability::CreateSandbox)
        {
            return Err(AgentLifecycleProvisioningError::Forbidden);
        }
        let _authority = self.authority(caller, context)?;
        // Machine allocation remains cohort-gated when an old operation is replayed.
        // Recheck before resume_connection/spawn_worker can resume provisioning.
        if request.machine_included {
            self.require_task_experience(caller, &request)?;
        }
        if let Some(receipt) = self
            .replay_receipt(caller, &request, idempotency_key, replay_operation)
            .await?
        {
            return Ok(receipt);
        }
        if replay_operation.is_some() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        self.require_task_experience(caller, &request)?;
        if request.repository.as_ref().is_some_and(|repository| {
            repository.operation.is_some() || repository.connection_id.is_some()
        }) && (self.managed_repository_preflight.get() != Some(&true)
            || request.sandbox_id.is_some()
            || request.workspace_id.is_some())
        {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "A selected GitHub connection or operation requires managed repository setup in a new Sandbox.".into(),
            ));
        }
        if request.sandbox_id.is_some() {
            self.contextual_sandbox(caller, context.map(|context| &context.actor), &request)
                .await?;
        }
        if request.workspace_id.is_some() {
            self.contextual_workspace(caller, context.map(|context| &context.actor), &request)
                .await?;
        }
        let profile = request.profile.clone().unwrap_or_default();
        let instructions = self
            .session_guidance
            .get()
            .copied()
            .unwrap_or_else(guidance::managed_v3);
        agent_content(&guidance::profile_with_instructions(
            profile.clone(),
            Some(instructions),
        ))?;
        services(&profile.permissions)?;
        if request.machine_included && request.image.is_some() {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "Machine Included selects the deployment desktop image and cannot be combined with an explicit image".into(),
            ));
        }
        if let Some(receipt) = self
            .check_capacity(caller, &request, idempotency_key)
            .await?
        {
            return Ok(receipt);
        }
        let operation = self
            .admit_operation(
                caller,
                context,
                delegated_credential,
                request,
                (idempotency_key, submission),
                instructions,
            )
            .await?;
        let operation_id = match operation {
            CreateResult::Created(id) => id,
            CreateResult::Existing(record) => record.id.clone(),
        };
        self.spawn_worker(operation_id.clone());
        Ok(AgentLifecycleCreateReceipt {
            id: operation_id,
            status: "provisioning".into(),
        })
    }
}
