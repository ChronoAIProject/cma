use super::*;

pub(super) struct Control {
    transport: ProviderHostedControlTransport,
    captured: Captured,
    state: Mutex<Authority>,
}

struct Authority {
    owner: DesktopOwnership,
    until: Instant,
    challenges: BTreeMap<String, (DesktopOwnershipChallenge, Instant)>,
    retired: bool,
}

impl Control {
    pub(super) fn new(
        transport: ProviderHostedControlTransport,
        captured: Captured,
        owner: DesktopOwnership,
        until: Instant,
    ) -> Self {
        Self {
            transport,
            captured,
            state: Mutex::new(Authority {
                owner,
                until,
                challenges: BTreeMap::new(),
                retired: false,
            }),
        }
    }
}

#[async_trait::async_trait]
impl DesktopOwnershipControl for Control {
    async fn ownership_challenge(&self) -> crate::Result<DesktopOwnershipChallenge> {
        let mut state = self.state.lock().await;
        if state.retired || state.until <= Instant::now() {
            return Err(refused());
        }
        let started = Instant::now();
        let request = DesktopChallengeRequest {
            sandbox_id: state.owner.sandbox_id.clone(),
            runtime_id: state.owner.runtime_id.clone(),
            previous: Some(state.owner.clone()),
        };
        let challenge: DesktopOwnershipChallenge = bounded(
            state.until.min(started + Duration::from_secs(3)),
            self.captured
                .exchange(&self.transport, "challenge", &request),
        )
        .await?;
        let until = deadline(started, &challenge)?;
        state
            .challenges
            .retain(|_, (_, until)| *until > Instant::now());
        if state.challenges.len() >= 16 || state.challenges.contains_key(&challenge.nonce) {
            return Err(refused());
        }
        state
            .challenges
            .insert(challenge.nonce.clone(), (challenge.clone(), until));
        Ok(challenge)
    }

    async fn renew_ownership(
        &self,
        owner: &DesktopOwnership,
        challenge: &DesktopOwnershipChallenge,
    ) -> crate::Result<()> {
        owner.validate()?;
        let mut state = self.state.lock().await;
        if state.retired
            || !same_session(&state.owner, owner)
            || owner.issued_at_ms < state.owner.issued_at_ms
            || owner.expires_at_ms < state.owner.expires_at_ms
        {
            return Err(refused());
        }
        let (original, until) = state
            .challenges
            .remove(&challenge.nonce)
            .ok_or_else(refused)?;
        if original != *challenge {
            return Err(refused());
        }
        let request = DesktopConfirmation {
            ownership: owner.clone(),
            nonce: challenge.nonce.clone(),
        };
        let reply: DesktopAcknowledgement = bounded(
            state.until.min(until),
            self.captured.exchange(&self.transport, "confirm", &request),
        )
        .await?;
        if reply.ownership != *owner {
            return Err(refused());
        }
        state.owner = owner.clone();
        state.until = until;
        Ok(())
    }

    async fn revoke_ownership(&self, owner: &DesktopOwnership) -> crate::Result<()> {
        owner.validate()?;
        let mut state = self.state.lock().await;
        if !same_session(&state.owner, owner) {
            return Err(refused());
        }
        state.retired = true;
        state.challenges.clear();
        let reply: DesktopAcknowledgement = bounded(
            Instant::now() + Duration::from_secs(2),
            self.captured.exchange(&self.transport, "revoke", owner),
        )
        .await?;
        if reply.ownership != *owner {
            return Err(refused());
        }
        Ok(())
    }
}
