use super::*;

#[derive(Default)]
pub(super) struct LeaseState {
    highest_fence: u64,
    pub(super) lease: Option<Lease>,
}

pub(super) struct Lease {
    challenge: String,
    fence: u64,
    pub(super) expires_at: Instant,
    proof: Option<(u64, CapacityRecoveryNativeProof)>,
}

impl Bridge {
    pub(crate) async fn reserve_capacity_recovery(
        &self,
        challenge: &str,
        writer_fence: u64,
    ) -> Result<(), CapacityRecoveryError> {
        if self.observation.is_none() {
            return Err(CapacityRecoveryError::Unsupported);
        }
        let mut state = self.capacity_recovery.lock().await;
        if state.lease.as_ref().is_some_and(|lease| {
            lease.expires_at > Instant::now()
                && lease.challenge == challenge
                && lease.fence == writer_fence
        }) {
            return Ok(());
        }
        // Retain the high-water fence after consumption/expiry. An old worker
        // cannot obtain another lease or revive a consumed challenge.
        if writer_fence <= state.highest_fence
            || writer_fence == u64::MAX
            || state
                .lease
                .as_ref()
                .is_some_and(|lease| lease.challenge == challenge)
        {
            return Err(CapacityRecoveryError::IdentityConflict);
        }
        state.highest_fence = writer_fence;
        state.lease = Some(Lease {
            challenge: challenge.to_owned(),
            fence: writer_fence,
            expires_at: Instant::now() + CAPACITY_RECOVERY_LEASE,
            proof: None,
        });
        Ok(())
    }

    pub(crate) async fn consume_capacity_recovery(&self, challenge: &str) -> bool {
        let Some(observation) = &self.observation else {
            return false;
        };
        let mut state = self.capacity_recovery.lock().await;
        let Some(lease) = &state.lease else {
            return false;
        };
        if lease.challenge != challenge || lease.expires_at <= Instant::now() || self.is_ended() {
            return false;
        }
        let Some((revision, proof)) = &lease.proof else {
            return false;
        };
        let observed = observation.lock().await;
        if observed.pending_output
            || observed.revision != *revision
            || self
                .retained_output
                .recovery_evidence()
                .await
                .is_none_or(|evidence| evidence.boundary != proof.confirmation.output)
        {
            tracing::debug!("capacity recovery proof changed before attachment");
            return false;
        }
        state.lease.take();
        true
    }

    pub(crate) async fn capacity_recovery_proof(
        &self,
        request: &CapacityRecoveryRequest,
        cwd: String,
    ) -> Result<CapacityRecoveryNativeProof, CapacityRecoveryError> {
        let observation = self
            .observation
            .as_ref()
            .ok_or(CapacityRecoveryError::Unsupported)?;
        let mut state = self.capacity_recovery.lock().await;
        let lease = state
            .lease
            .as_mut()
            .filter(|lease| {
                lease.challenge == request.challenge
                    && lease.fence == request.writer_fence
                    && lease.expires_at > Instant::now()
            })
            .ok_or(CapacityRecoveryError::IdentityConflict)?;
        if self.is_ended() || self.select_output_acknowledgement(true) != Ok(true) {
            return Err(CapacityRecoveryError::Unavailable);
        }
        let observed = observation.lock().await;
        if observed.pending_output || observed.revision == u64::MAX {
            return Err(CapacityRecoveryError::StateUnconfirmed);
        }
        if observed.conversation_id.as_deref() != Some(request.target.conversation_id.as_str()) {
            return Err(CapacityRecoveryError::IdentityConflict);
        }
        let native_state = observed
            .native_state
            .clone()
            .ok_or(CapacityRecoveryError::StateUnconfirmed)?;
        let output = self
            .retained_output
            .recovery_evidence()
            .await
            .ok_or(CapacityRecoveryError::OutputUnconfirmed)?
            .boundary;
        let confirmation = CapacityRecoveryConfirmation {
            request: request.clone(),
            native_state,
            output,
        };
        confirmation.validate_for(request)?;
        let proof = CapacityRecoveryNativeProof { confirmation, cwd };
        if let Some((revision, issued)) = &lease.proof {
            // One challenge names one snapshot, including on an HTTP retry.
            if *revision != observed.revision || *issued != proof {
                return Err(CapacityRecoveryError::StateUnconfirmed);
            }
        } else {
            lease.proof = Some((observed.revision, proof.clone()));
        }
        Ok(proof)
    }
}
