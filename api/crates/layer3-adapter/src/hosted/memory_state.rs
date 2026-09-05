use sha2::{Digest, Sha256};

use super::{InMemoryHostedBackend, InMemoryRuntimeSnapshot, MemoryRuntime};
use crate::wire::{ResponseFrame, WireFailure, WireOperation};
use crate::{AdapterError, CommandReceipt};

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
}

pub(super) fn require_credential(
    runtime: &MemoryRuntime,
    operation: WireOperation,
) -> crate::Result<()> {
    if runtime.credential_delivered {
        Ok(())
    } else {
        Err(AdapterError::Rejected(format!(
            "{operation:?}: credential_not_delivered"
        )))
    }
}

pub(super) fn replay_workspace_operation(
    runtime: &MemoryRuntime,
    operation_id: &str,
    request_sha256: &str,
    operation: WireOperation,
) -> crate::Result<Option<ResponseFrame>> {
    let Some((stored_digest, response)) = runtime.workspace_operations.get(operation_id) else {
        return Ok(None);
    };
    if stored_digest != request_sha256 {
        return refused(operation, "workspace_replay_mismatch");
    }
    Ok(Some(response.clone()))
}

pub(super) fn remember_workspace_operation(
    runtime: &mut MemoryRuntime,
    operation_id: String,
    request_sha256: String,
    response: ResponseFrame,
) {
    runtime
        .workspace_operations
        .insert(operation_id, (request_sha256, response));
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

pub(super) fn digest(bytes: &[u8]) -> String {
    let bytes = Sha256::digest(bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
