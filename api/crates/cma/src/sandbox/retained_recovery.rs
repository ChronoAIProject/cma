//! Fresh same-image observations, separated from update selection and admission.
use std::{collections::BTreeSet, sync::Arc};

use layer2_core::sandbox::{
    RetainedRecoveryDisposition as D, RetainedRecoveryInspector, RetainedRecoverySnapshot,
    RetainedReopenProof, RetainedReopenSnapshot, RetainedReopenVerifier, SandboxStoreError,
};
use layer3_adapter::{
    AdapterError, RecoveryConversation, RuntimeHandle, RuntimeInspection,
    RuntimeRecoveryInspectionRequest, RuntimeStatusRequest, SandboxAdapter,
    RUNTIME_CONTRACT_VERSION,
};

pub(crate) struct RetainedRuntimeInspector {
    adapter: Arc<dyn SandboxAdapter>,
    deployment: String,
}

impl RetainedRuntimeInspector {
    pub(crate) fn new(adapter: Arc<dyn SandboxAdapter>, deployment: String) -> Self {
        Self {
            adapter,
            deployment,
        }
    }

    async fn inspect_source(
        &self,
        snapshot: &RetainedRecoverySnapshot,
        reopening: bool,
    ) -> Result<(D, Option<RetainedReopenProof>), D> {
        let source = snapshot.source()?;
        let home = snapshot
            .sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .ok_or(D::AuthorityChanged)?;
        let volumes = self
            .adapter
            .home_volumes()
            .ok_or(D::UnsupportedImageAuthority)?;
        volumes.verify(&home).await.map_err(classify)?;
        let request = super::runtime_attachment::retained_runtime_request(
            &snapshot.sandbox,
            source.binding,
            &self.deployment,
        )
        .map_err(classify)?;
        let before = self
            .adapter
            .inspect_runtime(&request)
            .await
            .map_err(classify)?;
        if before
            .as_ref()
            .is_none_or(|inspection| !inspection.observation.lifecycle.is_running())
        {
            // A missing/running-state observation alone is not loss proof. Only
            // the exact retained native graph in a verified suspended state
            // proves process absence. With no process to read history, even a
            // terminal child keeps this inspection-only branch closed.
            if reopening || !snapshot.agents.is_empty() {
                return Err(D::EvidenceUnavailable);
            }
            self.adapter
                .verify_suspended_runtime(&request)
                .await
                .map_err(classify)?;
            volumes.verify(&home).await.map_err(classify)?;
            return Ok((D::ExactSuspendedRuntime, None));
        }
        let before = before.ok_or(D::EvidenceUnavailable)?;
        let identity = RuntimeStatusRequest {
            sandbox_id: snapshot.sandbox.id.to_string(),
            generation: source.generation,
            home_identity: source.home_identity.clone(),
        };
        let status = check(&before, &identity, &source.binding.runtime_id)?;
        // A replacement's live processes never become recovery permission.
        // Inspection has no stop/quiesce/suspend call, even after owner consent.
        if status.agent_processes != 0 {
            return Err(D::ChildActivity);
        }
        if reopening && !status.credential_ready {
            return Err(D::EvidenceUnavailable);
        }
        let conversations = snapshot
            .agents
            .iter()
            .filter(|agent| agent.status.is_live())
            .map(|agent| RecoveryConversation {
                agent_id: agent.id.to_string(),
                conversation_id: agent
                    .conversation
                    .conversation_id
                    .clone()
                    .unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        let history_request = RuntimeRecoveryInspectionRequest {
            runtime: RuntimeHandle::new(&source.binding.runtime_id),
            identity: identity.clone(),
            boot_id: status.boot_id.clone(),
            conversations,
        };
        let recovery = self
            .adapter
            .runtime_recovery()
            .ok_or(D::UnsupportedImageAuthority)?;
        let evidence = if reopening {
            recovery.inspect_idle_reopen(&history_request).await
        } else {
            recovery.inspect_recovery(&history_request).await
        };
        // A refused history response is not allowed to conceal native drift.
        let after = self
            .adapter
            .inspect_runtime(&request)
            .await
            .map_err(classify)?
            .ok_or(D::AuthorityChanged)?;
        check(&after, &identity, &source.binding.runtime_id)?;
        if after != before {
            return Err(D::AuthorityChanged);
        }
        volumes.verify(&home).await.map_err(classify)?;
        let evidence = evidence.map_err(|error| match error {
            AdapterError::Unavailable(_) | AdapterError::AtCapacity(_) => D::EvidenceUnavailable,
            _ => D::ProviderEvidenceRefused,
        })?;
        let expected = history_request
            .conversations
            .iter()
            .map(|c| (&c.agent_id, &c.conversation_id))
            .collect::<BTreeSet<_>>();
        let actual = evidence
            .histories
            .iter()
            .map(|c| (&c.agent_id, &c.conversation_id))
            .collect::<BTreeSet<_>>();
        if actual != expected
            || actual.len() != evidence.histories.len()
            || evidence.histories.iter().any(|h| {
                h.history_sha256.len() != 64
                    || !h
                        .history_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            || layer2_core::sandbox::SandboxUnpublishedRecoveryProof::provider_plan_digest(
                &evidence.provider_plan,
            )
            .map_err(|_| D::IncompleteHistory)?
                != evidence.provider_plan_sha256
        {
            return Err(D::IncompleteHistory);
        }
        let disposition = if status.boot_id == source.expected_boot {
            D::RecordedBootPresent
        } else {
            D::ExactBootReplacement
        };
        let proof = reopening.then(|| RetainedReopenProof {
            expected_runtime: source.binding.clone(),
            observed_boot: status.boot_id.clone(),
            home_identity: source.home_identity,
            provider_plan: evidence.provider_plan,
            provider_plan_sha256: evidence.provider_plan_sha256,
            histories: evidence.histories,
        });
        Ok((disposition, proof))
    }
}

#[async_trait::async_trait]
impl RetainedRecoveryInspector for RetainedRuntimeInspector {
    async fn inspect(&self, snapshot: &RetainedRecoverySnapshot) -> D {
        self.inspect_source(snapshot, false)
            .await
            .map(|(disposition, _)| disposition)
            .unwrap_or_else(|reason| reason)
    }
}

#[async_trait::async_trait]
impl RetainedReopenVerifier for RetainedRuntimeInspector {
    async fn verify(
        &self,
        snapshot: &RetainedReopenSnapshot,
    ) -> Result<RetainedReopenProof, SandboxStoreError> {
        snapshot.validate()?;
        self.inspect_source(&snapshot.retained, true)
            .await
            .map_err(|reason| match reason {
                D::EvidenceUnavailable => {
                    SandboxStoreError::Unavailable("retained reopening evidence unavailable".into())
                }
                _ => SandboxStoreError::InvalidTransaction("retained reopening evidence refused"),
            })?
            .1
            .ok_or(SandboxStoreError::InvalidTransaction(
                "retained reopening proof absent",
            ))
    }
}

fn check<'a>(
    inspection: &'a RuntimeInspection,
    identity: &RuntimeStatusRequest,
    runtime: &str,
) -> Result<&'a layer3_adapter::RuntimeStatus, D> {
    if inspection.observation.handle.as_str() != runtime
        || inspection.observation.sandbox_id != identity.sandbox_id
        || inspection.observation.generation != identity.generation
        || !inspection.observation.lifecycle.is_running()
    {
        return Err(D::AuthorityChanged);
    }
    if inspection.observation.runtime_contract != RUNTIME_CONTRACT_VERSION {
        return Err(D::UnsupportedImageAuthority);
    }
    let status = inspection.status.as_ref().ok_or(D::EvidenceUnavailable)?;
    status
        .validate_for(identity)
        .map_err(|_| D::AuthorityChanged)?;
    if status.restore_pending || !status.installation_ready {
        return Err(D::EvidenceUnavailable);
    }
    Ok(status)
}

fn classify(error: AdapterError) -> D {
    match error {
        AdapterError::Unavailable(_) | AdapterError::AtCapacity(_) => D::EvidenceUnavailable,
        _ => D::AuthorityChanged,
    }
}
