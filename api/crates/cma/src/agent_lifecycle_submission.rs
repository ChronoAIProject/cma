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
        if idempotency_key.starts_with(layer1_public::rest::creation_outcomes::KEY_PREFIX)
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
        if request.sandbox_id.is_some() {
            self.contextual_sandbox(caller, context.map(|context| &context.actor), &request)
                .await?;
        }
        if request.workspace_id.is_some() {
            self.contextual_workspace(caller, context.map(|context| &context.actor), &request)
                .await?;
        }
        let profile = request.profile.clone().unwrap_or_default();
        agent_content(&guidance::profile_with_instructions(
            profile.clone(),
            Some(guidance::managed_v3()),
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
        // Resolve the delegated Image Manager reference before persisting the
        // operation. The durable worker then needs no secret after admission.
        let resolved_image = match &request.image {
            Some(image) => Some(
                self.image_resolver
                    .resolve(delegated_credential, image)
                    .await?,
            ),
            None if request.machine_included => self
                .machine_sandbox_image
                .clone()
                .ok_or_else(|| {
                    AgentLifecycleProvisioningError::Invalid(
                        "Machine Included is not available in this deployment".into(),
                    )
                })
                .map(Some)?,
            None => None,
        };
        let resolved_image = self
            .resolve_desktop_image(caller, context, &request, resolved_image)
            .await?;
        let input = LifecycleInput {
            behavior: LifecycleBehavior::PromptRepository,
            managed_agent_instructions: Some(guidance::managed_v3().into()),
            setup_message: Some(validation::setup_message(&request)),
            request,
            resolved_image,
            capacity: Some(self.capacity),
            sandbox_availability: layer2_core::profiles::SandboxProfileAvailability {
                always_keep_alive: true,
                ..Default::default()
            },
            capabilities: caller.authorization.iter().collect(),
            actor: context.map(|context| context.actor.clone()),
        };
        let operation = self
            .operations
            .create_with_submission(&caller.subject, idempotency_key, input, submission)
            .await
            .map_err(validation::creation_error)?;
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
