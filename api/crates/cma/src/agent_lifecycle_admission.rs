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

pub(super) fn sandbox_admission_error(
    error: SandboxServiceError,
    profile: &SandboxProfileRef,
) -> AgentLifecycleProvisioningError {
    let services = match error {
        SandboxServiceError::ConnectionsRequired { services } => services
            .into_iter()
            .map(|service| service.to_string())
            .collect(),
        SandboxServiceError::CredentialSetupRequired => vec!["cma-credentials".into()],
        error => return sandbox_error(error),
    };
    AgentLifecycleProvisioningError::ConnectionRequired {
        services,
        connection_target: connection_profile_target(profile),
    }
}

impl InlineAgentLifecycle {
    pub(super) async fn admit_operation(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        delegated_credential: Option<&str>,
        request: AgentLifecycleCreateRequest,
        idempotency_key: &str,
    ) -> Result<CreateResult, AgentLifecycleProvisioningError> {
        use crate::store::LifecycleCreateError;
        use layer2_core::released_runtime::ReleasedLane;

        // Explicit Image Manager selections hold their own authority.
        let explicit = match &request.image {
            Some(image) => Some(
                self.image_resolver
                    .resolve(delegated_credential, image)
                    .await?,
            ),
            None => None,
        };
        let lane = crate::store::managed_lifecycle_release_lane(&request);
        for attempt in 0..4 {
            let captured = lane
                .and_then(|_| self.released.get())
                .map(|source| source.snapshot());
            let image = match (lane, &captured) {
                (Some(ReleasedLane::Machine), snapshot) => snapshot
                    .as_ref()
                    .and_then(|release| release.machine.clone())
                    .or_else(|| {
                        if snapshot.is_none() {
                            self.machine_sandbox_image.clone()
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| {
                        AgentLifecycleProvisioningError::Invalid(
                            "Machine Included is not available in this deployment".into(),
                        )
                    })
                    .map(Some)?,
                (Some(ReleasedLane::Agent), Some(release)) => Some(release.agent.clone()),
                _ => explicit.clone(),
            };
            let resolved_image = self
                .resolve_desktop_image(caller, context, &request, image)
                .await?;
            let input = LifecycleInput {
                behavior: LifecycleBehavior::PromptRepository,
                managed_agent_instructions: Some(guidance::managed_v3().into()),
                setup_message: Some(validation::setup_message(&request)),
                request: request.clone(),
                resolved_image,
                capacity: Some(self.capacity),
                sandbox_availability: layer2_core::profiles::SandboxProfileAvailability {
                    always_keep_alive: true,
                    ..Default::default()
                },
                capabilities: caller.authorization.iter().collect(),
                actor: context.map(|context| context.actor.clone()),
            };
            match self
                .operations
                .create_against_release(
                    &caller.subject,
                    idempotency_key,
                    input,
                    captured.as_ref().zip(lane),
                )
                .await
            {
                Err(LifecycleCreateError::StaleReleasedRuntime) if attempt < 3 => {
                    if let Some(source) = self.released.get() {
                        source.refresh_after_stale().await;
                    }
                    tokio::task::yield_now().await;
                }
                result => return result.map_err(validation::creation_error),
            }
        }
        Err(AgentLifecycleProvisioningError::Unavailable)
    }
}

pub(super) fn connection_profile_target(profile: &SandboxProfileRef) -> serde_json::Value {
    json!({"type":"sandbox_profile", "profile":profile})
}
