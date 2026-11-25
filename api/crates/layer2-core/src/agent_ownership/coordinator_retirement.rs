use super::*;
use crate::agent_ownership::{
    authority::CleanupState, RetirementIntent, Settlement, PROTOCOL_VERSION,
};
use std::future::Future;

const ATTEMPT_BUDGET: Duration = Duration::from_secs(6);
const STAGE_BUDGET: Duration = Duration::from_secs(2);

impl OwnershipCoordinator {
    pub(super) async fn settle_retirement<F>(
        &self,
        authority: &Arc<Authority>,
        revoke: F,
        until: Instant,
    ) -> Result<Settlement, OwnershipError>
    where
        F: Future<Output = Result<(), OwnershipError>>,
    {
        if let Some(error) = authority.settlement_error() {
            return Err(error);
        }
        let mut state = within(until, async { Ok(authority.cleanup.lock().await) }).await?;
        if let Some(error) = authority.settlement_error() {
            return Err(error);
        }
        if let Some(settlement) = state.settlement {
            return Ok(settlement);
        }
        if !self.exact_slot(authority).await {
            return Err(OwnershipError::Stale);
        }
        if !state.local_quiescent {
            stage(until, authority.join_local_work()).await?;
            state.local_quiescent = true;
        }
        // A receiver that always times out must not starve the independent
        // newer-owner proof. This opportunity survives a dropped attempt.
        if state.observe_next || self.is_draining() {
            state.observe_next = false;
            let observation_until = until.min(Instant::now() + STAGE_BUDGET);
            match within(observation_until, async {
                let proof = self.observe_successor(authority).await?;
                if proof.is_some() {
                    authority.abort_renewal_until(observation_until).await?;
                }
                Ok(proof)
            })
            .await
            {
                Ok(Some(settlement)) => {
                    self.complete_retirement(authority, &mut state, settlement)
                        .await;
                    return Ok(settlement);
                }
                Ok(None) | Err(OwnershipError::Unavailable(_)) => (),
                Err(error) => return Err(error),
            }
        }
        state.observe_next = true;
        let result = stage(until, revoke).await;
        if let Some(error) = authority.settlement_error() {
            return Err(error);
        }
        result?;
        let release_until = until.min(Instant::now() + STAGE_BUDGET);
        within(release_until, async {
            authority.join_renewal_until(release_until).await?;
            self.store.release(&authority.token).await
        })
        .await?;
        self.complete_retirement(authority, &mut state, Settlement::Released)
            .await;
        Ok(Settlement::Released)
    }

    pub async fn drain_authority_until(
        &self,
        authority: &Arc<Authority>,
        until: Instant,
    ) -> Result<Settlement, OwnershipError> {
        if !self.is_draining() {
            return Err(OwnershipError::Stale);
        }
        // Shutdown may own generically fenced authorities as well as ordinary
        // retirements. It supplies no provenance for serving-time recovery.
        authority.revoke();
        let until = until.min(Instant::now() + ATTEMPT_BUDGET);
        let result = within(
            until,
            self.settle_retirement(
                authority,
                authority.revoke_transports_after_local_drain(),
                until,
            ),
        )
        .await;
        authority.settlement_error().map_or(result, Err)
    }

    pub async fn retirement_background(
        &self,
        authority: &Arc<Authority>,
    ) -> Result<Option<Settlement>, OwnershipError> {
        authority.begin_retirement(RetirementIntent::Cooperative)?;
        let Ok(_slot) = self.cleanup_slots.try_acquire() else {
            return Ok(None);
        };
        let until = Instant::now() + ATTEMPT_BUDGET;
        let Some(mut attempt) = self.cleanup_pacing.begin(authority) else {
            return Ok(None);
        };
        let result = within(
            until,
            self.settle_retirement(
                authority,
                authority.revoke_transports_after_local_drain(),
                until,
            ),
        )
        .await;
        if let Some(error) = authority.settlement_error() {
            return Err(error);
        }
        match result {
            Ok(settlement) => {
                attempt.confirmed();
                tracing::info!(parent: None,
                    stage = "receiver_housekeeping",
                    outcome = match settlement {
                        Settlement::Released => "released",
                        Settlement::Superseded { .. } => "superseded",
                    },
                    epoch = authority.token.epoch,
                    "Ownership cleanup completed"
                );
                Ok(Some(settlement))
            }
            Err(OwnershipError::Unavailable(_)) => {
                tracing::debug!(parent: None,
                    stage = "receiver_housekeeping",
                    outcome = "pending",
                    epoch = authority.token.epoch,
                    "Ownership cleanup remains pending"
                );
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    async fn observe_successor(
        &self,
        authority: &Authority,
    ) -> Result<Option<Settlement>, OwnershipError> {
        let Some(owner) = self.store.owner(&authority.token.agent_id).await? else {
            return Ok(None);
        };
        if owner.token.agent_id != authority.token.agent_id
            || owner.token.instance_id.is_empty()
            || owner.token.protocol != PROTOCOL_VERSION
            || owner.token.epoch == 0
            || owner.expires_at_ms <= 0
        {
            return Err(OwnershipError::Invalid(
                "invalid successor observation".into(),
            ));
        }
        Ok(
            (owner.token.epoch > authority.token.epoch).then_some(Settlement::Superseded {
                newer_epoch: owner.token.epoch,
            }),
        )
    }

    pub(super) async fn exact_slot(&self, authority: &Arc<Authority>) -> bool {
        self.owned
            .lock()
            .unwrap()
            .get(&authority.token.agent_id)
            .is_some_and(|slot| slot.owns(authority))
    }

    pub(super) async fn complete_retirement(
        &self,
        authority: &Arc<Authority>,
        state: &mut CleanupState,
        settlement: Settlement,
    ) {
        let mut owned = self.owned.lock().unwrap();
        if owned
            .get(&authority.token.agent_id)
            .is_some_and(|slot| slot.owns(authority))
        {
            self.cleanup_pacing
                .receiver_cleanup_completed(&authority.token.agent_id);
            if owned
                .get(&authority.token.agent_id)
                .is_some_and(|slot| slot.acquisition.is_none())
            {
                owned.remove(&authority.token.agent_id);
            }
        }
        state.settlement = Some(settlement);
    }
}

pub(super) async fn stage<T>(
    until: Instant,
    operation: impl Future<Output = Result<T, OwnershipError>>,
) -> Result<T, OwnershipError> {
    within(until.min(Instant::now() + STAGE_BUDGET), operation).await
}

pub(super) async fn within<T>(
    until: Instant,
    operation: impl Future<Output = Result<T, OwnershipError>>,
) -> Result<T, OwnershipError> {
    if Instant::now() >= until {
        return Err(deadline_exceeded());
    }
    tokio::time::timeout_at(until, operation)
        .await
        .map_err(|_| deadline_exceeded())?
}
