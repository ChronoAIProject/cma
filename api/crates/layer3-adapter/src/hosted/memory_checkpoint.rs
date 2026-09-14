use super::state::{digest, refused, require_credential};
use super::MemoryRuntime;
use crate::wire::{Acknowledgement, ResponseFrame, WireOperation};
use crate::{
    AgentHomePlan, AgentSlot, CheckpointHoldReceipt, CheckpointHoldRequest, CheckpointReceipt,
    CheckpointRequest, CheckpointWriterResumeReceipt, CheckpointWriterResumeRequest,
    ResourceReading, RestoreRequest, WorkspaceReceipt, WorktreeReceipt, WriterState,
};

#[derive(Clone, Serialize)]
pub(super) struct HomeArchive {
    sandbox_id: String,
    workspaces: BTreeMap<String, WorkspaceReceipt>,
    worktrees: Vec<((String, String), WorktreeReceipt)>,
    homes: BTreeMap<AgentSlot, AgentHomePlan>,
    conversations: BTreeMap<AgentSlot, String>,
}

type Archives = Mutex<BTreeMap<String, HomeArchive>>;

pub(super) fn capture(
    runtime: &mut MemoryRuntime,
    archives: &Archives,
    request: CheckpointRequest,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    require_credential(runtime, operation)?;
    runtime.writer_fence_captures += 1;
    let sha256 = capture_archive(runtime, archives, request.epoch)?;
    Ok(ResponseFrame::Checkpoint(CheckpointReceipt {
        epoch: request.epoch,
        object_reference: format!("home-{sha256}.cma"),
        sha256,
        writers: WriterState::Resumed,
    }))
}

pub(super) fn capture_for_teardown(
    runtime: &mut MemoryRuntime,
    archives: &Archives,
    request: CheckpointHoldRequest,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    require_credential(runtime, operation)?;
    if !request.has_valid_writer_resume_token() {
        return refused(operation, "writer_resume_token_invalid");
    }
    if runtime
        .resumed_checkpoint_tokens
        .contains(&request.writer_resume_token)
    {
        return refused(operation, "checkpoint_hold_retired");
    }
    if let Some((held_request, held_receipt)) = &runtime.held_checkpoint {
        return if held_request == &request {
            Ok(ResponseFrame::CheckpointForTeardown(held_receipt.clone()))
        } else {
            refused(operation, "checkpoint_hold_conflict")
        };
    }
    runtime.writer_fence_captures += 1;
    let sha256 = capture_archive(runtime, archives, request.checkpoint.epoch)?;
    let receipt = CheckpointHoldReceipt {
        checkpoint: CheckpointReceipt {
            epoch: request.checkpoint.epoch,
            object_reference: format!("home-{sha256}.cma"),
            sha256,
            writers: WriterState::Held,
        },
        writer_resume_token: request.writer_resume_token.clone(),
    };
    runtime.held_checkpoint = Some((request, receipt.clone()));
    Ok(ResponseFrame::CheckpointForTeardown(receipt))
}

pub(super) fn resume_writers(
    runtime: &mut MemoryRuntime,
    request: CheckpointWriterResumeRequest,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    if !request.has_valid_writer_resume_token() {
        return refused(operation, "writer_resume_token_invalid");
    }
    if runtime
        .held_checkpoint
        .as_ref()
        .is_some_and(|(held_request, _)| {
            !crate::writer_resume_tokens_match(
                &held_request.writer_resume_token,
                &request.writer_resume_token,
            )
        })
    {
        return refused(operation, "checkpoint_hold_conflict");
    }
    runtime
        .resumed_checkpoint_tokens
        .insert(request.writer_resume_token.clone());
    runtime.held_checkpoint = None;
    Ok(ResponseFrame::ResumeCheckpointWriters(
        CheckpointWriterResumeReceipt {
            writer_resume_token: request.writer_resume_token,
            writers_resumed: true,
        },
    ))
}

pub(super) fn restore(
    runtime: &mut MemoryRuntime,
    archives: &Archives,
    request: RestoreRequest,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    if !runtime.credential_delivered {
        return refused(operation, "credential_not_delivered");
    }
    if runtime
        .restore_bootstrap
        .as_ref()
        .is_some_and(|bootstrap| bootstrap.restore_request_sha256 != request.computed_sha256())
    {
        return refused(operation, "restore_bootstrap_conflict");
    }
    if let Some(restored) = &runtime.restored_object {
        return if restored == &request.object_reference {
            Ok(ResponseFrame::Restore(Acknowledgement::default()))
        } else {
            refused(operation, "restore_replay_conflict")
        };
    }
    let archives = archives.lock().expect("in-memory checkpoint archive lock");
    let Some(archive) = archives.get(&request.sha256) else {
        return refused(operation, "restore_archive_missing");
    };
    if request.object_reference != format!("home-{}.cma", request.sha256)
        || archive.sandbox_id != provider_request_identity(&runtime.request)?.sandbox_id
    {
        return refused(operation, "restore_archive_identity_mismatch");
    }
    runtime.workspaces = archive.workspaces.clone();
    runtime.worktrees = archive.worktrees.iter().cloned().collect();
    runtime.homes = archive.homes.clone();
    runtime.restored_conversations = archive.conversations.clone();
    runtime.restored_object = Some(request.object_reference);
    Ok(ResponseFrame::Restore(Acknowledgement::default()))
}

pub(super) fn resources(runtime: &MemoryRuntime) -> crate::Result<ResponseFrame> {
    Ok(ResponseFrame::Resources(ResourceReading {
        uptime_ms: 1,
        cpu_millis_limit: runtime
            .request
            .resource_limits
            .get("cpu")
            .and_then(|value| value.strip_suffix('m'))
            .and_then(|value| value.parse().ok()),
        ..ResourceReading::default()
    }))
}

fn capture_archive(
    runtime: &MemoryRuntime,
    archives: &Archives,
    epoch: u64,
) -> crate::Result<String> {
    let archive = HomeArchive {
        sandbox_id: provider_request_identity(&runtime.request)?.sandbox_id,
        workspaces: runtime.workspaces.clone(),
        worktrees: runtime
            .worktrees
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        homes: runtime.homes.clone(),
        conversations: runtime
            .agents
            .iter()
            .map(|(slot, agent)| (slot.clone(), agent.conversation_id.clone()))
            .collect(),
    };
    let bytes = serde_json::to_vec(&(epoch, &archive))
        .map_err(|error| crate::AdapterError::Rejected(error.to_string()))?;
    let sha256 = digest(&bytes);
    archives
        .lock()
        .expect("in-memory checkpoint archive lock")
        .insert(sha256.clone(), archive);
    Ok(sha256)
}
use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;

use super::super::deployment::provider_request_identity;
