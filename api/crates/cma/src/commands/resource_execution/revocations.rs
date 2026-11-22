use super::*;
use futures_util::{stream, StreamExt};
use layer2_core::resource_access::execution::{
    unix_millis_now, ExecutionRecordV1, MemberCompletionV1, RevocationOperationV1,
};
use std::time::Duration;

// A control observation deadline supplies no stop evidence. The next turn
// queries the same receiver/request if a reply or commit was lost.
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(2);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const FINALIZATION_TIMEOUT: Duration = Duration::from_secs(2);

impl RevocationWorker {
    pub(crate) async fn reconcile(&self, id: &wire::Digest) -> ExecutionResult<Progress> {
        let (operation, members) = tokio::time::timeout(CAPTURE_TIMEOUT, self.capture(id))
            .await
            .unwrap_or(Err(ExecutionError::Unavailable))?;
        if matches!(operation.phase, RevocationPhaseV1::Effective { .. }) {
            return Ok(Progress::Effective);
        }
        let mut pending = stream::iter(members.into_iter().map(|member| async {
            // Queueing for local capacity consumes no receiver observation time.
            // The permit covers both control calls and retained evidence writes.
            let _slot = self
                .participant_slots
                .acquire()
                .await
                .map_err(|_| ExecutionError::Unavailable)?;
            tokio::time::timeout(
                OBSERVATION_TIMEOUT,
                self.complete_member(&operation, member),
            )
            .await
            .unwrap_or(Err(ExecutionError::Unknown))
        }))
        .buffer_unordered(PARTICIPANT_CONCURRENCY);
        let mut completed = 0;
        let mut failures = Vec::new();
        while let Some(result) = pending.next().await {
            match result {
                Ok(()) => completed += 1,
                Err(error) => failures.push(error),
            }
        }
        if failures.is_empty() {
            // R1 rechecks all frozen rows and lineage in the final transaction.
            // A dropped commit observation may already have committed. The next
            // attempt must reread this operation before doing participant work.
            let finalized =
                tokio::time::timeout(FINALIZATION_TIMEOUT, self.store.finish_revocation(id))
                    .await
                    .unwrap_or(Err(ExecutionError::Unknown))?;
            finalized.validate()?;
            if finalized.key != operation.key
                || finalized.epoch != operation.epoch
                || finalized.intent != operation.intent
                || finalized.frozen_members != operation.frozen_members
                || finalized.frozen_at_unix_ms != operation.frozen_at_unix_ms
                || finalized.deadline_unix_ms != operation.deadline_unix_ms
                || !matches!(finalized.phase, RevocationPhaseV1::Effective { .. })
            {
                return Err(ExecutionError::EvidenceMismatch);
            }
            return Ok(Progress::Effective);
        }
        Ok(Progress::Pending {
            completed,
            total: operation.frozen_members,
            deadline_exceeded: operation.deadline_exceeded_at(unix_millis_now()?)?,
            failures,
        })
    }

    async fn capture(
        &self,
        id: &wire::Digest,
    ) -> ExecutionResult<(RevocationOperationV1, Vec<ExecutionRecordV1>)> {
        let operation = self
            .store
            .revocation(id)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        operation.validate()?;
        if &operation.key.operation_id != id {
            return Err(ExecutionError::EvidenceMismatch);
        }
        let members = if matches!(operation.phase, RevocationPhaseV1::Effective { .. }) {
            Vec::new()
        } else {
            self.cohort(&operation).await?
        };
        Ok((operation, members))
    }

    async fn cohort(
        &self,
        operation: &RevocationOperationV1,
    ) -> ExecutionResult<Vec<ExecutionRecordV1>> {
        let mut members = Vec::with_capacity(operation.frozen_members as usize);
        let mut after = 0;
        while after < operation.frozen_members {
            let page = self
                .store
                .frozen_members(&operation.key.operation_id, after, MAX_PAGE_MEMBERS)
                .await?;
            if page.is_empty() || page.len() > MAX_PAGE_MEMBERS as usize {
                return Err(ExecutionError::Corrupt);
            }
            for member in page {
                if member.ordinal != after + 1
                    || member.ordinal > operation.frozen_members
                    || member.epoch != operation.epoch
                {
                    return Err(ExecutionError::EvidenceMismatch);
                }
                member.barrier_request(operation)?;
                after = member.ordinal;
                members.push(member);
            }
        }
        Ok(members)
    }

    async fn complete_member(
        &self,
        operation: &RevocationOperationV1,
        mut member: ExecutionRecordV1,
    ) -> ExecutionResult<()> {
        let request = member.barrier_request(operation)?;
        if member.completion.is_some() {
            member
                .barrier
                .as_ref()
                .ok_or(ExecutionError::EvidenceMismatch)?
                .validate_for(&request)?;
            return Ok(());
        }
        let participant = self.participants.resolve(&member.member).await?;
        participant.require_member(&member.member)?;
        let completion = match participant.receipt(&request).await? {
            Some(receipt) => receipt,
            None => {
                let barrier = participant.barrier(&request).await?;
                member.observe_barrier(&request, &barrier)?;
                self.store.observe_barrier(&barrier).await?;
                participant.stop(&request).await?
            }
        };
        let barrier = match &completion {
            MemberCompletionV1::Execution { receipt } => &receipt.barrier,
            MemberCompletionV1::Release { receipt } => &receipt.barrier,
        };
        let barrier_persisted = member.barrier.as_ref() == Some(barrier);
        member.observe_barrier(&request, barrier)?;
        member.observe_completion(&completion)?;
        if !barrier_persisted {
            self.store.observe_barrier(barrier).await?;
        }
        self.store.observe_completion(&completion).await
    }
}
