use super::state::{digest, refused, require_credential};
use super::MemoryRuntime;
use crate::wire::{Acknowledgement, ResponseFrame, WireOperation};
use crate::{
    CheckpointHoldReceipt, CheckpointHoldRequest, CheckpointReceipt, CheckpointRequest,
    CheckpointWriterResumeReceipt, CheckpointWriterResumeRequest, ResourceReading, RestoreRequest,
    RuntimeHandle, WriterState,
};

pub(super) fn capture(
    runtime: &mut MemoryRuntime,
    handle: &RuntimeHandle,
    request: CheckpointRequest,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    require_credential(runtime, operation)?;
    runtime.writer_fence_captures += 1;
    let sha256 = capture_digest(runtime, handle, request.epoch);
    Ok(ResponseFrame::Checkpoint(CheckpointReceipt {
        epoch: request.epoch,
        object_reference: format!("home-{sha256}.cma"),
        sha256,
        writers: WriterState::Resumed,
    }))
}

pub(super) fn capture_for_teardown(
    runtime: &mut MemoryRuntime,
    handle: &RuntimeHandle,
    request: CheckpointHoldRequest,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    require_credential(runtime, operation)?;
    if !request.has_valid_writer_resume_token() {
        return refused(operation, "writer_resume_token_invalid");
    }
    if let Some((held_request, held_receipt)) = &runtime.held_checkpoint {
        return if held_request == &request {
            Ok(ResponseFrame::CheckpointForTeardown(held_receipt.clone()))
        } else {
            refused(operation, "checkpoint_hold_conflict")
        };
    }
    runtime.writer_fence_captures += 1;
    let sha256 = capture_digest(runtime, handle, request.checkpoint.epoch);
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

fn capture_digest(runtime: &MemoryRuntime, handle: &RuntimeHandle, epoch: u64) -> String {
    let marker = format!("{}:{epoch}:{}", handle.0, runtime.writer_fence_captures);
    digest(marker.as_bytes())
}
