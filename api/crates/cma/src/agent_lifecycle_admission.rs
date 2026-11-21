use super::*;

impl InlineAgentLifecycle {
    pub(super) fn authority(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
    ) -> Result<BotOwnerAuthority, AgentLifecycleProvisioningError> {
        let context = context.ok_or(AgentLifecycleProvisioningError::Forbidden)?;
        if context.caller.subject != caller.subject || !context.validate_principal() {
            return Err(AgentLifecycleProvisioningError::Forbidden);
        }
        BotOwnerAuthority::new(
            context.clone(),
            ResourceOwner::Person {
                subject: caller.subject.clone(),
            },
            None,
        )
        .map_err(|_| AgentLifecycleProvisioningError::Forbidden)
    }

    pub(crate) fn with_failure_metadata(self: Arc<Self>, enabled: bool) -> Arc<Self> {
        self.failure_metadata_enabled
            .set(enabled)
            .expect("lifecycle failure writer is configured once before use");
        self
    }

    pub(super) async fn replay_receipt(
        &self,
        caller: &Caller,
        request: &AgentLifecycleCreateRequest,
        key: &str,
    ) -> Result<Option<AgentLifecycleCreateReceipt>, AgentLifecycleProvisioningError> {
        let Some(record) = self
            .operations
            .replay(&caller.subject, key, request)
            .await
            .map_err(validation::operation_error)?
        else {
            return Ok(None);
        };
        self.operations
            .resume_connection(&caller.subject, &record.id)
            .await
            .map_err(validation::operation_error)?;
        self.spawn_worker(record.id.clone());
        Ok(Some(AgentLifecycleCreateReceipt {
            id: record.id,
            status: "provisioning".into(),
        }))
    }

    pub(super) async fn check_capacity(
        &self,
        caller: &Caller,
        request: &AgentLifecycleCreateRequest,
        key: &str,
    ) -> Result<Option<AgentLifecycleCreateReceipt>, AgentLifecycleProvisioningError> {
        if request.sandbox_id.is_some() || request.workspace_id.is_some() {
            return Ok(None);
        }
        let capacity = self.sandbox.capacity(caller).await.map_err(sandbox_error)?;
        if capacity.available > 0 {
            return Ok(None);
        }
        // Another caller with this exact intent may have committed during the read.
        if let Some(receipt) = self.replay_receipt(caller, request, key).await? {
            return Ok(Some(receipt));
        }
        Err(AgentLifecycleProvisioningError::SandboxLimitReached {
            limit: capacity.limit,
            live: capacity.live,
        })
    }
}
