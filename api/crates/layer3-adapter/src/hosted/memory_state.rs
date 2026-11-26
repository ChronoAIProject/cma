use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::super::deployment::provider_request_identity;
use super::{InMemoryHostedBackend, MemoryOperation, MemoryRuntime};
use crate::provider::ProviderSandboxRequest;
use crate::wire::{ResponseFrame, WireFailure, WireOperation};
use crate::{
    AdapterError, AgentHomePlan, AgentLaunch, AgentSlot, CommandReceipt, RuntimeHandle,
    RuntimeLifecycle, RuntimeObservation, WorkspaceReceipt, WorktreeReceipt,
};
use serde::Serialize;

/// Observable in-memory state used by Layer 2 lifecycle tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InMemoryRuntimeSnapshot {
    pub request: ProviderSandboxRequest,
    pub credential_delivered: bool,
    pub workspaces: BTreeMap<String, WorkspaceReceipt>,
    pub worktrees: BTreeMap<(String, String), WorktreeReceipt>,
    pub homes: BTreeMap<AgentSlot, AgentHomePlan>,
    pub agents: BTreeMap<AgentSlot, AgentLaunch>,
    pub writer_fence_captures: u64,
    pub writers_held: bool,
    pub restored_object: Option<String>,
}

impl MemoryRuntime {
    pub(super) fn snapshot(&self) -> InMemoryRuntimeSnapshot {
        InMemoryRuntimeSnapshot {
            request: self.request.clone(),
            credential_delivered: self.credential_delivered,
            workspaces: self.workspaces.clone(),
            worktrees: self.worktrees.clone(),
            homes: self.homes.clone(),
            agents: self
                .agents
                .iter()
                .map(|(slot, agent)| (slot.clone(), agent.launch.clone()))
                .collect(),
            writer_fence_captures: self.writer_fence_captures,
            writers_held: self.held_checkpoint.is_some(),
            restored_object: self.restored_object.clone(),
        }
    }
}

impl InMemoryHostedBackend {
    pub(super) fn queued_failure(&self, operation: WireOperation) -> Option<WireFailure> {
        let mut failures = self.failures.lock().expect("in-memory failure queue lock");
        let index = failures
            .iter()
            .position(|failure| failure.operation == operation)?;
        failures.remove(index)
    }

    pub(super) fn take_lost_response(&self, operation: WireOperation) -> bool {
        let mut responses = self
            .lost_responses
            .lock()
            .expect("in-memory lost-response queue lock");
        let Some(index) = responses.iter().position(|queued| *queued == operation) else {
            return false;
        };
        responses.remove(index);
        true
    }
}

pub(super) fn memory_observation(
    handle: &RuntimeHandle,
    request: &ProviderSandboxRequest,
) -> crate::Result<RuntimeObservation> {
    let identity = provider_request_identity(request)?;
    Ok(RuntimeObservation {
        sandbox_id: identity.sandbox_id,
        generation: identity.generation,
        handle: handle.clone(),
        image_sha256: identity.image_sha256,
        spec_fingerprint: identity.spec_fingerprint,
        runtime_contract: identity.runtime_contract,
        lifecycle: RuntimeLifecycle {
            state: "Running".into(),
            reason: None,
        },
    })
}

pub(super) fn reclaim(
    runtimes: &mut BTreeMap<RuntimeHandle, MemoryRuntime>,
    observation: &RuntimeObservation,
) -> crate::Result<Option<MemoryRuntime>> {
    let Some(runtime) = runtimes.get(&observation.handle) else {
        return Ok(None);
    };
    if memory_observation(&observation.handle, &runtime.request)? != *observation {
        return Err(AdapterError::Rejected(
            "provider runtime changed after it was observed".into(),
        ));
    }
    Ok(runtimes.remove(&observation.handle))
}

pub(super) fn require_credential(
    runtime: &MemoryRuntime,
    operation: WireOperation,
) -> crate::Result<()> {
    if runtime.install.is_some()
        && runtime.restore_bootstrap.is_none()
        && matches!(
            operation,
            WireOperation::PrepareWorkspace
                | WireOperation::WorkspaceSetup
                | WireOperation::AddWorktree
                | WireOperation::MaterializeAgentHome
                | WireOperation::OpenAgent
                | WireOperation::CacheSkillArchive
        )
    {
        return Ok(());
    }
    if runtime.credential_delivered
        && (runtime.restore_bootstrap.is_none() || runtime.restored_object.is_some())
    {
        Ok(())
    } else {
        Err(AdapterError::Rejected(format!(
            "{operation:?}: credential_not_delivered"
        )))
    }
}

pub(super) fn replay_operation(
    runtime: &MemoryRuntime,
    operation_id: &str,
    request_sha256: &str,
    payload_sha256: &str,
    operation: WireOperation,
) -> crate::Result<Option<ResponseFrame>> {
    let Some(stored) = runtime.operations.get(operation_id) else {
        return Ok(None);
    };
    if stored.request_sha256 != request_sha256 || stored.payload_sha256 != payload_sha256 {
        return if operation == WireOperation::OpenAgent {
            Err(AdapterError::OperationIdentityConflict(format!(
                "{operation:?}: operation_replay_mismatch"
            )))
        } else {
            refused(operation, "operation_replay_mismatch")
        };
    }
    Ok(Some(stored.response.clone()))
}

pub(super) fn remember_operation(
    runtime: &mut MemoryRuntime,
    operation_id: String,
    request_sha256: String,
    payload_sha256: String,
    response: ResponseFrame,
) {
    runtime.operations.insert(
        operation_id,
        MemoryOperation {
            request_sha256,
            payload_sha256,
            response,
        },
    );
}

pub(super) fn refused<T>(operation: WireOperation, code: &str) -> crate::Result<T> {
    Err(AdapterError::Rejected(format!("{operation:?}: {code}")))
}

pub(super) fn successful_commands(commands: &[String]) -> Vec<CommandReceipt> {
    let empty = digest(&[]);
    commands
        .iter()
        .enumerate()
        .map(|(index, _)| CommandReceipt {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            exit_code: 0,
            stdout_sha256: empty.clone(),
            stderr_sha256: empty.clone(),
        })
        .collect()
}

pub(super) fn runtime_home(runtime: &MemoryRuntime) -> &str {
    runtime
        .request
        .env
        .get("CHRONO_RUNTIME_HOME")
        .map(String::as_str)
        .unwrap_or("/home/cma")
}

pub(super) fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn safe_identity(value: &str) -> bool {
    value.len() <= 64 && !value.starts_with('.') && safe_name(value)
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn fingerprint(value: &impl Serialize) -> crate::Result<String> {
    serde_json::to_vec(value)
        .map(|bytes| digest(&bytes))
        .map_err(|error| AdapterError::Rejected(format!("encode replay request: {error}")))
}

pub(super) fn digest(bytes: &[u8]) -> String {
    let bytes = Sha256::digest(bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn channel(conversation_id: Option<String>) -> crate::ConversationChannel {
    crate::ConversationChannel {
        input: crate::ConversationInputSender::spawn(1, |_| async { Ok(()) }),
        conversation_id,
    }
}
