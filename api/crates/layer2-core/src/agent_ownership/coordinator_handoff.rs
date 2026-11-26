use super::*;

impl OwnershipCoordinator {
    pub async fn handoff<F>(
        &self,
        authority: &Arc<Authority>,
        revoke_transport: F,
    ) -> Result<(), OwnershipError>
    where
        F: std::future::Future<Output = Result<(), OwnershipError>>,
    {
        if self.contains_failures() {
            authority.begin_retirement(super::super::RetirementIntent::Cooperative)?;
            let until = Instant::now() + Duration::from_secs(6);
            let result = retirement::within(
                until,
                self.settle_retirement(authority, revoke_transport, until),
            )
            .await
            .map(|_| ());
            return authority.settlement_error().map_or(result, Err);
        }
        // Serialize exact-token retirement through receiver confirmation and
        // durable release. Cancellation leaves completion false for safe retry.
        let mut complete = authority.cleanup.lock().await;
        if complete.settlement.is_some() || complete.legacy_forgotten {
            return Ok(());
        }
        authority.retiring.store(true, Ordering::Release);
        authority.revoke();
        // Do not shorten the lease until every receiving runtime's cleanup
        // obligation is settled. Delayed input/ACK packets may be in flight.
        bounded(revoke_transport).await?;
        // Cooperative cleanup also ends its retry episode. Old completion
        // must not clear observations belonging to a replacement authority.
        if self
            .owned
            .lock()
            .unwrap()
            .get(&authority.token.agent_id)
            .is_some_and(|slot| slot.owns(authority))
        {
            self.cleanup_pacing
                .receiver_cleanup_completed(&authority.token.agent_id);
        }
        bounded(self.store.release(&authority.token)).await?;
        self.forget_legacy_authority(authority);
        complete.settlement = Some(super::super::Settlement::Released);
        Ok(())
    }

    /// Drop only cancelled local custody after a successful durable observation
    /// proves a newer epoch replaced it. Absence may race an uncertain renewal,
    /// so it retains the cancelled token until ACK/release or a new acquisition.
    /// This never releases a lease or changes a replacement epoch.
    pub async fn forget_cancelled(
        &self,
        authority: &Arc<Authority>,
    ) -> Result<bool, OwnershipError> {
        if self.contains_failures() {
            return Ok(false);
        }
        if !authority.receiver_cancelled() {
            return Ok(false);
        }
        let mut complete = authority.cleanup.lock().await;
        if complete.settlement.is_some() || complete.legacy_forgotten {
            return Ok(true);
        }
        let owner = bounded(self.store.owner(&authority.token.agent_id)).await?;
        if owner
            .as_ref()
            .is_none_or(|lease| lease.token.epoch <= authority.token.epoch)
        {
            return Ok(false);
        }
        self.forget_legacy_authority(authority);
        complete.settlement = Some(super::super::Settlement::Superseded {
            newer_epoch: owner.unwrap().token.epoch,
        });
        Ok(true)
    }

    /// Preserve the receiver-only policy's existing expiry handling. The
    /// combined policy always retains its epoch floor until proved settlement.
    pub async fn forget_unconfirmed_retirement(
        &self,
        authority: &Arc<Authority>,
    ) -> Result<bool, OwnershipError> {
        if self.contains_failures()
            || !authority.retiring.load(Ordering::Acquire)
            || authority.receiver_cancelled()
        {
            return Ok(false);
        }
        let mut complete = authority.cleanup.lock().await;
        if complete.settlement.is_some() || complete.legacy_forgotten {
            return Ok(true);
        }
        let owner = bounded(self.store.owner(&authority.token.agent_id)).await?;
        if owner.is_some_and(|lease| lease.token == authority.token) {
            return Ok(false);
        }
        self.forget_legacy_authority(authority);
        complete.legacy_forgotten = true;
        Ok(true)
    }

    fn forget_legacy_authority(&self, authority: &Arc<Authority>) {
        let mut owned = self.owned.lock().unwrap();
        let id = &authority.token.agent_id;
        if let Some(slot) = owned.get_mut(id).filter(|slot| slot.owns(authority)) {
            slot.authority = None;
            if slot.acquisition.is_none() {
                owned.remove(id);
            }
        }
    }
}
