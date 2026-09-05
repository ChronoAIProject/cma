use sha2::{Digest, Sha256};

use super::{InMemoryHostedBackend, InMemoryRuntimeSnapshot, MemoryOperation, MemoryRuntime};
use crate::wire::{ResponseFrame, WireFailure, WireOperation};
use crate::{AdapterError, CommandReceipt};
use serde::Serialize;

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
        return refused(operation, "operation_replay_mismatch");
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
