use super::*;

impl Bridge {
    pub(crate) async fn reserve_capacity_recovery(
        &self,
        challenge: &str,
        writer_fence: u64,
    ) -> Result<(), CapacityRecoveryError> {
        let mut lease = self.capacity_recovery.lock().await;
        if lease
            .as_ref()
            .is_some_and(|(_, _, expires_at)| *expires_at <= Instant::now())
        {
            lease.take();
        }
        if let Some((current, fence, _)) = &*lease {
            if current != challenge || *fence != writer_fence {
                return Err(CapacityRecoveryError::IdentityConflict);
            }
        } else {
            *lease = Some((
                challenge.to_owned(),
                writer_fence,
                Instant::now() + CAPACITY_RECOVERY_LEASE,
            ));
        }
        Ok(())
    }

    pub(crate) async fn consume_capacity_recovery(&self, challenge: &str) -> bool {
        let mut lease = self.capacity_recovery.lock().await;
        if lease
            .as_ref()
            .is_some_and(|(_, _, expires_at)| *expires_at <= Instant::now())
        {
            lease.take();
        }
        if lease
            .as_ref()
            .is_some_and(|(current, _, _)| current == challenge)
        {
            lease.take();
            true
        } else {
            false
        }
    }

    pub(crate) async fn capacity_recovery_proof(
        &self,
        request: &CapacityRecoveryRequest,
        cwd: String,
    ) -> Result<CapacityRecoveryNativeProof, CapacityRecoveryError> {
        let lease_matches = self.capacity_recovery.lock().await.as_ref().is_some_and(
            |(challenge, fence, expires_at)| {
                challenge == &request.challenge
                    && *fence == request.writer_fence
                    && *expires_at > Instant::now()
            },
        );
        if !lease_matches {
            return Err(CapacityRecoveryError::IdentityConflict);
        }
        if self.is_ended() || self.select_output_acknowledgement(true) != Ok(true) {
            return Err(CapacityRecoveryError::Unavailable);
        }
        let conversation_id = self.conversation_id.lock().await.clone();
        if conversation_id.as_deref() != Some(request.target.conversation_id.as_str()) {
            return Err(CapacityRecoveryError::IdentityConflict);
        }
        let native_state = self
            .native_state
            .lock()
            .await
            .clone()
            .ok_or(CapacityRecoveryError::StateUnconfirmed)?;
        let output = self
            .retained_output
            .recovery_boundary()
            .await
            .ok_or(CapacityRecoveryError::OutputUnconfirmed)?;
        let confirmation = CapacityRecoveryConfirmation {
            request: request.clone(),
            native_state,
            output,
        };
        confirmation.validate_for(request)?;
        Ok(CapacityRecoveryNativeProof { confirmation, cwd })
    }

}
