use layer3_adapter::wire::{Acknowledgement, ResponseFrame};
use layer3_adapter::{
    CheckpointHoldReceipt, CheckpointHoldRequest, CheckpointWriterResumeReceipt,
    CheckpointWriterResumeRequest, WriterState,
};

use super::{HeldCheckpoint, Phase, Supervisor};
use crate::failure::{Result, RuntimeError};
use crate::writer_fence::WriterFence;

#[path = "checkpoint_writer_tokens.rs"]
mod tokens;

impl Supervisor {
    pub(super) async fn checkpoint(
        &self,
        request: &layer3_adapter::CheckpointRequest,
    ) -> Result<ResponseFrame> {
        let roots = self.writer_roots().await;
        let mut fence = WriterFence::quiesce(roots, self.writer_boundary)
            .map_err(|error| RuntimeError::unavailable("writer_fence_failed", error))?;
        let receipt = self.checkpoint.capture(request).await?;
        fence
            .resume()
            .map_err(|error| RuntimeError::unavailable("writer_resume_failed", error))?;
        tracing::info!(writers = fence.writer_count(), "checkpoint writers resumed");
        Ok(ResponseFrame::Checkpoint(receipt))
    }

    pub(super) async fn checkpoint_for_teardown(
        &self,
        request: CheckpointHoldRequest,
    ) -> Result<ResponseFrame> {
        if !request.has_valid_writer_resume_token() {
            return Err(RuntimeError::rejected(
                "writer_resume_token_invalid",
                "writer resume token must be 64 lowercase hexadecimal characters",
            ));
        }
        let mut held = self.held_checkpoint.lock().await;
        if tokens::was_resumed(&self.config, &request.writer_resume_token)? {
            return Err(RuntimeError::rejected(
                "checkpoint_hold_retired",
                "checkpoint writer token was already resumed",
            ));
        }
        if let Some(existing) = held.as_ref() {
            return if existing.request == request {
                Ok(ResponseFrame::CheckpointForTeardown(
                    existing.receipt.clone(),
                ))
            } else {
                Err(RuntimeError::rejected(
                    "checkpoint_hold_conflict",
                    "another checkpoint writer hold is already active",
                ))
            };
        }

        let roots = self.writer_roots().await;
        let fence = WriterFence::quiesce(roots, self.writer_boundary)
            .map_err(|error| RuntimeError::unavailable("writer_fence_failed", error))?;
        let mut checkpoint = self.checkpoint.capture(&request.checkpoint).await?;
        checkpoint.writers = WriterState::Held;
        let receipt = CheckpointHoldReceipt {
            checkpoint,
            writer_resume_token: request.writer_resume_token.clone(),
        };
        tracing::info!(
            writers = fence.writer_count(),
            "checkpoint writers retained for teardown"
        );
        *held = Some(HeldCheckpoint {
            request,
            receipt: receipt.clone(),
            fence,
        });
        Ok(ResponseFrame::CheckpointForTeardown(receipt))
    }

    pub(super) async fn resume_checkpoint_writers(
        &self,
        request: CheckpointWriterResumeRequest,
    ) -> Result<ResponseFrame> {
        if !request.has_valid_writer_resume_token() {
            return Err(RuntimeError::rejected(
                "writer_resume_token_invalid",
                "writer resume token must be 64 lowercase hexadecimal characters",
            ));
        }

        let mut held = self.held_checkpoint.lock().await;
        if held.as_ref().is_some_and(|existing| {
            !layer3_adapter::writer_resume_tokens_match(
                &existing.request.writer_resume_token,
                &request.writer_resume_token,
            )
        }) {
            return Err(RuntimeError::rejected(
                "checkpoint_hold_conflict",
                "writer resume token does not own the active checkpoint hold",
            ));
        }
        tokens::retire(&self.config, &request.writer_resume_token)?;
        let Some(mut existing) = held.take() else {
            return Ok(ResponseFrame::ResumeCheckpointWriters(
                CheckpointWriterResumeReceipt {
                    writer_resume_token: request.writer_resume_token,
                    writers_resumed: true,
                },
            ));
        };
        // The retired token cannot replay a hold/export. Release its potentially
        // multi-gigabyte local archive before waking the Sandbox's processes.
        // A failed export also retires this token and will recapture under new
        // authority; its incomplete remote upload has separate durable cleanup.
        if let Err(error) = self
            .checkpoint
            .discard(&existing.receipt.checkpoint.object_reference)
            .await
        {
            *held = Some(existing);
            return Err(error);
        }
        if let Err(error) = existing.fence.resume() {
            *held = Some(existing);
            return Err(RuntimeError::unavailable("writer_resume_failed", error));
        }
        tracing::info!(
            writers = existing.fence.writer_count(),
            "checkpoint writers resumed after aborted teardown"
        );
        Ok(ResponseFrame::ResumeCheckpointWriters(
            CheckpointWriterResumeReceipt {
                writer_resume_token: request.writer_resume_token,
                writers_resumed: true,
            },
        ))
    }

    pub(super) async fn restore(
        &self,
        request: &layer3_adapter::RestoreRequest,
    ) -> Result<ResponseFrame> {
        let phase = *self.phase.read().await;
        let bootstrap = self.restore_bootstrap.read().await.clone();
        match phase {
            Phase::RestorePending | Phase::Provisioned | Phase::Installed => {
                return Err(RuntimeError::rejected(
                    "credential_not_installed",
                    "checkpoint restore requires credential delivery first",
                ))
            }
            Phase::RestoreCredentialed | Phase::Credentialed => {}
        }
        if bootstrap
            .as_ref()
            .is_some_and(|identity| identity.restore_request_sha256 != request.computed_sha256())
        {
            return Err(RuntimeError::rejected(
                "restore_bootstrap_conflict",
                "restore request does not match the inherited installation checkpoint",
            ));
        }
        let already_restored = crate::persistent_home::begin_restore(&self.config, request)?;
        if already_restored {
            // A retried transport may have reimported the archive. Acknowledge
            // the durable completion while preserving all subsequent user work.
            self.checkpoint.discard(&request.object_reference).await?;
            return Ok(ResponseFrame::Restore(Acknowledgement::default()));
        }
        if !self.agents.read().await.is_empty() {
            return Err(RuntimeError::rejected(
                "restore_agents_open",
                "checkpoint restore requires every agent process to be closed",
            ));
        }
        let mut fence = WriterFence::quiesce([], self.writer_boundary)
            .map_err(|error| RuntimeError::unavailable("writer_fence_failed", error))?;
        self.checkpoint.restore(request).await?;
        // The durable remote recovery point remains available for retries.
        // Keeping its local compressed transfer after extraction needlessly
        // competes with restored files and the Agents that are about to start.
        self.checkpoint.discard(&request.object_reference).await?;
        crate::persistent_home::complete_restore(&self.config, request)?;
        fence
            .resume()
            .map_err(|error| RuntimeError::unavailable("writer_resume_failed", error))?;
        if phase == Phase::RestoreCredentialed {
            *self.phase.write().await = Phase::Credentialed;
        }
        Ok(ResponseFrame::Restore(Acknowledgement::default()))
    }

    async fn writer_roots(&self) -> Vec<i32> {
        self.agents
            .read()
            .await
            .values()
            .filter_map(|agent| agent.bridge.writer_session_root())
            .collect()
    }
}
