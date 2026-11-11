use super::*;

impl LedgerCollectionCheckpointV2 {
    pub fn apply(&self, mutation: LedgerCollectionCheckpointMutation) -> Result<Self> {
        if !self.validate() {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        use LedgerCollectionCheckpointMutation::*;
        match mutation {
            Claim {
                holder_sha256,
                now,
                expires_at,
            } => {
                let mut next = self.clone();
                next.progress = self.progress.claim(holder_sha256, now, expires_at)?;
                next.checked()
            }
            PinPage { fence, now, page } => {
                self.idle()?;
                let mut next = self.clone();
                next.progress = self.progress.pin_page(fence, now, page)?;
                next.checked()
            }
            RecordReference {
                fence,
                now,
                reference,
            } => {
                self.require_claim(fence, now)?;
                if let Some(original) = &self.accepted_artifact {
                    return if *original == reference {
                        Ok(self.clone())
                    } else {
                        Err(LedgerCollectionProgressError::PendingTransition)
                    };
                }
                self.idle()?;
                let mut next = self.bump()?;
                next.accepted_artifact = Some(reference);
                next.checked()
            }
            Select {
                fence,
                now,
                pending,
            } => {
                self.require_claim(fence, now)?;
                if self.accepted_artifact.is_none() {
                    return Err(LedgerCollectionProgressError::PendingTransition);
                }
                let mut next = self.clone();
                next.progress = self.progress.select(fence, now, pending)?;
                next.checked()
            }
            Acknowledge {
                fence,
                now,
                acknowledgement,
            } => self.acknowledge(fence, now, acknowledgement),
            SettleAcknowledged {
                fence,
                now,
                operation_id,
                receipt_sha256,
            } => {
                self.require_claim(fence, now)?;
                let pending = self
                    .progress
                    .pending
                    .as_ref()
                    .ok_or(LedgerCollectionProgressError::PendingTransition)?;
                if pending.phase != LedgerCollectionPhase::Acknowledged
                    || pending.operation_id != operation_id
                    || pending.receipt_sha256.as_deref() != Some(&receipt_sha256)
                {
                    return Err(LedgerCollectionProgressError::PendingTransition);
                }
                let mut next = self.bump()?;
                next.settled_after = Some(pending.group_key.clone());
                next.progress.last_ack_sha256 = Some(receipt_sha256);
                next.progress.active_revision = self
                    .acknowledged_revision
                    .ok_or(LedgerCollectionProgressError::Invalid)?;
                // Every mutation consumes exactly one original group. Its old
                // page/cursor cannot enumerate the new revision.
                next.progress.pending = None;
                next.progress.page = None;
                next.progress.cursor = None;
                next.progress.complete = false;
                next.accepted_artifact = None;
                next.acknowledged_revision = None;
                next.checked()
            }
            SelectRetained {
                fence,
                now,
                pending,
            } => {
                self.require_claim(fence, now)?;
                self.idle()?;
                let mut next = self.clone();
                next.progress = self.progress.select(fence, now, pending)?;
                // A separately verified no-effect classification, not an A
                // source-transition receipt. No mutation archive is consulted.
                next.progress.pending.as_mut().expect("selected").phase =
                    LedgerCollectionPhase::Retained;
                next.checked()
            }
            SettleRetained {
                fence,
                now,
                operation_id,
            } => {
                self.require_claim(fence, now)?;
                let pending = self
                    .progress
                    .pending
                    .as_ref()
                    .ok_or(LedgerCollectionProgressError::PendingTransition)?;
                if pending.phase != LedgerCollectionPhase::Retained
                    || self.accepted_artifact.is_some()
                {
                    return Err(LedgerCollectionProgressError::PendingTransition);
                }
                let mut next = self.clone();
                next.progress = self.progress.settle_group(fence, now, &operation_id)?;
                next.settled_after = Some(pending.group_key.clone());
                next.checked()
            }
            Advance {
                fence,
                now,
                active_revision,
                page_sha256,
            } => {
                self.idle()?;
                let mut next = self.clone();
                next.progress = self
                    .progress
                    .advance(fence, now, active_revision, &page_sha256)?;
                next.checked()
            }
            Refresh {
                fence,
                now,
                active_revision,
            } => {
                self.require_claim(fence, now)?;
                self.idle()?;
                if active_revision < self.progress.active_revision
                    || (active_revision == self.progress.active_revision && !self.progress.complete)
                {
                    return Err(LedgerCollectionProgressError::StaleCursor);
                }
                let mut next = self.bump()?;
                if self.progress.complete {
                    next.progress.sweep = self
                        .progress
                        .sweep
                        .checked_add(1)
                        .ok_or(LedgerCollectionProgressError::Invalid)?;
                    next.settled_after = None;
                }
                next.progress.active_revision = active_revision;
                next.progress.cursor = None;
                next.progress.page = None;
                next.progress.complete = false;
                next.checked()
            }
        }
    }

    fn acknowledge(
        &self,
        fence: u64,
        now: i64,
        ack: LedgerCollectionFinalAcknowledgement,
    ) -> Result<Self> {
        self.require_claim(fence, now)?;
        let pending = self
            .progress
            .pending
            .as_ref()
            .ok_or(LedgerCollectionProgressError::PendingTransition)?;
        let reference = self
            .accepted_artifact
            .as_ref()
            .ok_or(LedgerCollectionProgressError::PendingTransition)?;
        if pending.operation_id != ack.operation_id
            || pending.group_key != ack.group_key
            || pending.source_sha256 != ack.source_sha256
            || pending.observation_sha256 != ack.observation_sha256
            || reference.plan_sha256 != ack.plan_sha256
            || self.progress.binding_sha256 != ack.binding_sha256
            || self.progress.active_revision != ack.before_revision
            || ack.after_revision <= ack.before_revision
            || !digest(&ack.receipt_sha256)
        {
            return Err(LedgerCollectionProgressError::Invalid);
        }
        if pending.phase == LedgerCollectionPhase::Acknowledged {
            return if pending.receipt_sha256.as_ref() == Some(&ack.receipt_sha256)
                && self.acknowledged_revision == Some(ack.after_revision)
            {
                Ok(self.clone())
            } else {
                Err(LedgerCollectionProgressError::Invalid)
            };
        }
        if pending.phase != LedgerCollectionPhase::Selected {
            return Err(LedgerCollectionProgressError::PendingTransition);
        }
        let mut next = self.bump()?;
        let pending = next.progress.pending.as_mut().expect("checked pending");
        // B2's final-only progression preserves A's original final hash. B1's
        // transition methods are unchanged; no intermediate receipt is invented.
        pending.phase = LedgerCollectionPhase::Acknowledged;
        pending.receipt_sha256 = Some(ack.receipt_sha256);
        next.acknowledged_revision = Some(ack.after_revision);
        next.checked()
    }
}
