use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{SandboxLifecycleAgentScope, SandboxStoreError};
use layer2_core::sandbox::{
    Agent, AgentStatus, RuntimeBinding, Sandbox, SandboxEventKind, SandboxLifecycleOperation,
    SandboxStatus,
};
use layer3_adapter::{
    CredentialFrame, RestoreBootstrapRequest, RuntimeCredential, RuntimeProvisionRequest,
};

use super::super::support::{
    adapter_failure, credential_failure, credential_rotation_at, material_matches, runtime,
    WorkerFailure,
};
use super::super::SandboxWorker;
use super::checkpoint::{pinned_checkpoint, require_quiet_agent};
use super::support::{lifecycle_lease, model_error, operation_id};
use crate::sandbox::runtime_mapping::sandbox_spec;

impl SandboxWorker {
    pub(super) async fn verify_lifecycle_checkpoint(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let checkpoint = match pinned_checkpoint(&sandbox) {
            Ok(checkpoint) => checkpoint.clone(),
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_checkpoint_verification(
            &checkpoint,
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        self.complete_sandbox_only_phase(sandbox, next, stamp).await
    }

    pub(super) async fn provision_lifecycle_runtime(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let checkpoint = match pinned_checkpoint(&sandbox) {
            Ok(checkpoint) => checkpoint.clone(),
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let generation = match sandbox.lifecycle.runtime_provision.as_ref() {
            Some(reservation) => reservation.generation,
            None => {
                let agents = self.lifecycle_agents(&sandbox).await?;
                let minimum = match replacement_runtime_generation(&sandbox, &agents) {
                    Ok(generation) => generation,
                    Err(failure) => {
                        return self.handle_lifecycle_failure(sandbox, failure, now).await
                    }
                };
                let stamp = rfc3339_at(now);
                let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
                let mut next = sandbox.clone();
                next.reserve_lifecycle_runtime_generation(
                    minimum,
                    &lease.holder,
                    lease.fence,
                    stamp.clone(),
                )
                .map_err(model_error)?;
                return self.complete_sandbox_only_phase(sandbox, next, stamp).await;
            }
        };
        let request = RuntimeProvisionRequest::new(
            generation,
            sandbox_spec(&sandbox, &self.config.deployment_id),
        );
        let handle = match self.dependencies.adapter.provision_runtime(&request).await {
            Ok(handle) => handle,
            Err(error) => {
                return self
                    .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                    .await
            }
        };
        let restore = checkpoint.restore_request().map_err(model_error)?;
        let bootstrap = RestoreBootstrapRequest::new(
            operation_id(&sandbox, "bootstrap_restore")?,
            sandbox.id.to_string(),
            restore.clone(),
        );
        let receipt = match self
            .dependencies
            .adapter
            .bootstrap_restore(&handle, &bootstrap)
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                return self
                    .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                    .await
            }
        };
        if receipt.operation_id != bootstrap.operation_id
            || receipt.request_sha256 != bootstrap.request_sha256
            || receipt.restore_request_sha256 != restore.computed_sha256()
            || !receipt.installation_inherited
        {
            if let Err(error) = self.dependencies.adapter.teardown(&handle).await {
                tracing::warn!(sandbox_id = %sandbox.id, runtime_id = %handle.0, error = %error, "invalid restore bootstrap left replacement cleanup unconfirmed");
            }
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security(
                        "runtime returned invalid restore bootstrap evidence",
                    ),
                    now,
                )
                .await;
        }

        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.bind_lifecycle_runtime(
            RuntimeBinding {
                provision_generation: Some(generation),
                runtime_id: handle.0,
                runtime_version: replacement_runtime_version(
                    &sandbox,
                    &self.config.runtime_version,
                ),
                bound_at: stamp.clone(),
            },
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        let committed = self
            .complete_sandbox_only_phase(sandbox, next.clone(), stamp)
            .await;
        if committed.is_ok() || self.lifecycle_state_matches(&next).await? {
            Ok(())
        } else {
            committed
        }
    }

    pub(super) async fn deliver_lifecycle_credential(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(handle) = runtime(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("replacement runtime binding is absent"),
                    now,
                )
                .await;
        };
        let Some(authority) = sandbox.credential.current.clone() else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("current credential authority is absent"),
                    now,
                )
                .await;
        };
        let material = match self
            .dependencies
            .credentials
            .material(&sandbox.id, authority.generation)
            .await
        {
            Ok(Some(material)) if material_matches(&sandbox, &authority, &material) => material,
            Ok(_) => {
                return self
                    .handle_lifecycle_failure(
                        sandbox,
                        WorkerFailure::terminal_security(
                            "durable current credential does not match its authority",
                        ),
                        now,
                    )
                    .await
            }
            Err(error) => {
                return self
                    .handle_lifecycle_failure(sandbox, credential_failure(error), now)
                    .await
            }
        };
        let frame = CredentialFrame {
            operation_id: operation_id(&sandbox, "deliver_credential")?,
            value: RuntimeCredential::new(material.value.expose()),
            custody_generation: Some(authority.generation),
        };
        if let Err(error) = self
            .dependencies
            .adapter
            .deliver_credential(&handle, frame)
            .await
        {
            return self
                .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                .await;
        }
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_credential_delivery(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        self.complete_sandbox_only_phase(sandbox, next, stamp).await
    }

    pub(super) async fn restore_lifecycle_home(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(handle) = runtime(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("replacement runtime binding is absent"),
                    now,
                )
                .await;
        };
        let checkpoint = match pinned_checkpoint(&sandbox) {
            Ok(checkpoint) => checkpoint,
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let request = checkpoint.restore_request().map_err(model_error)?;
        if let Err(error) = self.dependencies.adapter.restore(&handle, &request).await {
            return self
                .handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                .await;
        }
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_home_restore(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        self.complete_sandbox_only_phase(sandbox, next, stamp).await
    }

    pub(super) async fn publish_restored_active(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let agents = self.lifecycle_agents(&sandbox).await?;
        if !agents
            .iter()
            .filter(|agent| agent.status.is_live())
            .all(|agent| {
                agent.status == AgentStatus::Ready
                    && require_quiet_agent(agent).is_ok()
                    && agent.conversation.queued_messages.is_empty()
            })
        {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::retryable("restored agents are not settled"),
                    now,
                )
                .await;
        }
        let Some(rotation_at) = credential_rotation_at(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("current credential rotation time is invalid"),
                    now,
                )
                .await;
        };
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let operation = sandbox.lifecycle.operation.clone();
        let mut next = sandbox.clone();
        next.publish_lifecycle_active(
            rfc3339_at(rotation_at),
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        let mut events = vec![SandboxEventKind::StatusChanged {
            from: sandbox.status,
            to: SandboxStatus::Active,
        }];
        match operation {
            Some(SandboxLifecycleOperation::Revive { .. }) => {
                events.push(SandboxEventKind::Revived)
            }
            Some(SandboxLifecycleOperation::ImageRotation {
                target_runtime_version,
                ..
            }) => events.push(SandboxEventKind::Rotated {
                runtime_version: target_runtime_version,
            }),
            _ => {}
        }
        self.commit_lifecycle_phase(
            &sandbox,
            next,
            Vec::new(),
            SandboxLifecycleAgentScope::Touched,
            stamp,
            events,
        )
        .await
    }
}

pub(super) fn replacement_runtime_generation(
    sandbox: &Sandbox,
    agents: &[Agent],
) -> Result<u64, WorkerFailure> {
    let next_agent_generation = agents
        .iter()
        .filter(|agent| agent.status.is_live())
        .map(|agent| {
            agent
                .conversation
                .runtime_generation
                .ok_or_else(|| {
                    WorkerFailure::terminal_security(
                        "live agent runtime generation is absent during restore",
                    )
                })?
                .checked_add(1)
                .ok_or_else(|| {
                    WorkerFailure::terminal_validation("agent runtime generation is exhausted")
                })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .unwrap_or(1);
    let next_authority_generation = sandbox
        .lifecycle
        .operation
        .as_ref()
        .ok_or_else(|| WorkerFailure::terminal_security("lifecycle authority is absent"))?
        .authority()
        .generation
        .checked_add(1)
        .ok_or_else(|| WorkerFailure::terminal_validation("lifecycle authority is exhausted"))?;
    Ok(next_agent_generation.max(next_authority_generation))
}

fn replacement_runtime_version(sandbox: &Sandbox, deployed: &str) -> String {
    match sandbox.lifecycle.operation.as_ref() {
        Some(SandboxLifecycleOperation::ImageRotation {
            target_runtime_version,
            ..
        }) => target_runtime_version.clone(),
        _ => deployed.to_owned(),
    }
}
