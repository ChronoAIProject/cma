use super::*;
use layer3_adapter::desktop_ownership::wire::{
    DesktopAcknowledgement, DesktopConfirmation, DESKTOP_RECEIVER_LEASE_MS,
};
use layer3_adapter::DesktopOwnershipChallenge;
use std::time::Duration;

impl DesktopReceiver {
    pub(crate) fn challenge(
        &self,
        request: DesktopChallengeRequest,
    ) -> Result<DesktopOwnershipChallenge> {
        if request.sandbox_id != self.identity.sandbox
            || request.runtime_id.trim().is_empty()
            || request.runtime_id.len() > 512
        {
            return Err(refused());
        }
        let mut state = self.state.lock().unwrap();
        self.verify(&mut state)?;
        if let Some(previous) = &request.previous {
            self.check_owner(previous)?;
            if previous.sandbox_id != request.sandbox_id
                || previous.runtime_id != request.runtime_id
                || !self.live(&state, previous)
            {
                return Err(refused());
            }
        }
        state
            .challenges
            .retain(|_, challenge| challenge.deadline > Instant::now());
        if state.challenges.len() >= 16 {
            return Err(refused());
        }
        let nonce = uuid::Uuid::new_v4().to_string();
        state.challenges.insert(
            nonce.clone(),
            Challenge {
                request,
                deadline: Instant::now() + Duration::from_millis(DESKTOP_RECEIVER_LEASE_MS),
            },
        );
        Ok(DesktopOwnershipChallenge {
            nonce,
            valid_for_ms: DESKTOP_RECEIVER_LEASE_MS,
        })
    }

    pub(crate) async fn confirm(
        &self,
        confirmation: DesktopConfirmation,
    ) -> Result<DesktopAcknowledgement> {
        let owner = confirmation.ownership;
        self.check_owner(&owner)?;
        let mut tasks = self.tasks.lock().await;
        // Completed handles no longer own I/O. Reap them while holding the
        // admission lock before deciding whether a persisted drain is needed.
        while tasks.try_join_next().is_some() {}
        let (deadline, renewal) = {
            let mut state = self.state.lock().unwrap();
            self.verify(&mut state)?;
            let challenge = state
                .challenges
                .remove(&confirmation.nonce)
                .ok_or_else(refused)?;
            if challenge.deadline <= Instant::now()
                || challenge.request.sandbox_id != owner.sandbox_id
                || challenge.request.runtime_id != owner.runtime_id
            {
                return Err(refused());
            }
            let renewal = challenge.request.previous.is_some();
            if let Some(previous) = challenge.request.previous {
                if !same_session(&previous, &owner)
                    || !self.live(&state, &owner)
                    || state.record.as_ref().is_none_or(|record| {
                        owner.issued_at_ms < record.owner.issued_at_ms
                            || owner.expires_at_ms < record.owner.expires_at_ms
                    })
                {
                    return Err(refused());
                }
            } else {
                if state
                    .record
                    .as_ref()
                    .is_some_and(|record| record.owner.epoch >= owner.epoch)
                {
                    return Err(refused());
                }
                // A pending drain needs a tombstone before its first await:
                // cancellation cannot reopen legacy or retry this epoch. With
                // no prior tasks, the final durable write can install directly
                // while this same admission lock excludes new registrations.
                state.deadline = None;
                state.revision = state.revision.checked_add(1).ok_or_else(refused)?;
                state.challenges.clear();
                self.changed.send_replace(());
                if !tasks.is_empty() {
                    self.persist(
                        &mut state,
                        Record {
                            version: 1,
                            receiver: self.identity.clone(),
                            owner: owner.clone(),
                            retired: true,
                        },
                    )?;
                }
            }
            (challenge.deadline, renewal)
        };
        if !renewal {
            // Retained JoinSet + lock ensures a cancelled waiter cannot lose
            // custody. A failed task joined here has also stopped all its I/O.
            while tasks.join_next().await.is_some() {}
        }
        let mut state = self.state.lock().unwrap();
        self.verify(&mut state)?;
        if deadline <= Instant::now() || (renewal && !self.live(&state, &owner)) {
            return Err(refused());
        }
        let old_deadline = state.deadline;
        self.persist(
            &mut state,
            Record {
                version: 1,
                receiver: self.identity.clone(),
                owner: owner.clone(),
                retired: false,
            },
        )?;
        if deadline <= Instant::now()
            || (renewal && old_deadline.is_none_or(|until| until <= Instant::now()))
        {
            self.retire(&mut state)?;
            return Err(refused());
        }
        state.deadline = Some(deadline);
        if !renewal {
            state.reserved = false;
        }
        self.verify(&mut state)?;
        if deadline <= Instant::now()
            || (renewal && old_deadline.is_none_or(|until| until <= Instant::now()))
        {
            self.retire(&mut state)?;
            return Err(refused());
        }
        self.changed.send_replace(());
        Ok(DesktopAcknowledgement { ownership: owner })
    }

    pub(crate) async fn revoke(&self, owner: DesktopOwnership) -> Result<DesktopAcknowledgement> {
        self.check_owner(&owner)?;
        let mut tasks = self.tasks.lock().await;
        {
            let mut state = self.state.lock().unwrap();
            self.verify(&mut state)?;
            if state.record.as_ref().is_none_or(|record| {
                record.receiver != self.identity || !same_session(&record.owner, &owner)
            }) {
                return Err(refused());
            }
            self.retire(&mut state)?;
        }
        while tasks.join_next().await.is_some() {}
        let mut state = self.state.lock().unwrap();
        self.verify(&mut state)?;
        if state
            .record
            .as_ref()
            .is_none_or(|record| !record.retired || !same_session(&record.owner, &owner))
        {
            self.poison(&mut state);
            return Err(unavailable());
        }
        Ok(DesktopAcknowledgement { ownership: owner })
    }

    pub(super) fn retire(&self, state: &mut State) -> Result<()> {
        state.deadline = None;
        state.challenges.clear();
        self.changed.send_replace(());
        if let Some(mut record) = state.record.clone() {
            record.retired = true;
            self.persist(state, record)?;
        }
        Ok(())
    }
}
