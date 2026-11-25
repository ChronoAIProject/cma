//! Exact one-use renewal with authentic store and caller-cancellation evidence.

use super::{ArmError, Registry, MAX_LIFETIME};
use layer2_core::{
    agent_ownership::{Authority, CapturedOwner, OwnershipError, OwnershipStore, RenewalWindow},
    qualification::{RenewalAttempt, RenewalHook},
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenewalOutcome {
    Committed,
    Unavailable,
    Stale,
    Incompatible,
    Invalid,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenewalEvidence {
    pub started: bool,
    pub result: Option<RenewalOutcome>,
    pub caller_dropped: bool,
}

pub(crate) struct Entry {
    generation: Arc<()>,
    expires_at: Instant,
    work: Option<SelectedRenewal>,
}

struct SelectedRenewal {
    owner: CapturedOwner,
    isolated_store: Arc<dyn OwnershipStore>,
    evidence: watch::Sender<RenewalEvidence>,
}

pub struct RenewalArm {
    registration: RenewalRegistration,
    evidence: watch::Receiver<RenewalEvidence>,
}

impl RenewalArm {
    pub fn evidence(&self) -> RenewalEvidence {
        *self.evidence.borrow()
    }

    pub async fn changed(&mut self) -> Option<RenewalEvidence> {
        self.evidence.changed().await.ok()?;
        Some(*self.evidence.borrow_and_update())
    }

    pub fn disarm(self) {
        drop(self.registration);
    }
}

struct RenewalRegistration {
    registry: Registry,
    generation: Arc<()>,
}

impl Drop for RenewalRegistration {
    fn drop(&mut self) {
        let mut entry = self.registry.renewal.lock().expect("renewal registry lock");
        if entry
            .as_ref()
            .is_some_and(|entry| Arc::ptr_eq(&entry.generation, &self.generation))
        {
            *entry = None;
        }
    }
}

impl Registry {
    /// The caller prepares a store client dedicated to this one attempt.
    pub fn arm_renewal(
        &self,
        owner: CapturedOwner,
        isolated_store: Arc<dyn OwnershipStore>,
        lifetime: Duration,
    ) -> Result<RenewalArm, ArmError> {
        if lifetime.is_zero() || lifetime > MAX_LIFETIME {
            return Err(ArmError::InvalidLifetime);
        }
        let now = Instant::now();
        let mut entry = self.renewal.lock().expect("renewal registry lock");
        if entry.as_ref().is_some_and(|entry| entry.expires_at > now) {
            return Err(ArmError::AlreadyArmed);
        }
        let (sender, evidence) = watch::channel(RenewalEvidence::default());
        let generation = Arc::new(());
        *entry = Some(Entry {
            generation: generation.clone(),
            expires_at: now + lifetime,
            work: Some(SelectedRenewal {
                owner,
                isolated_store,
                evidence: sender,
            }),
        });
        Ok(RenewalArm {
            registration: RenewalRegistration {
                registry: self.clone(),
                generation,
            },
            evidence,
        })
    }

    fn claim_renewal(
        &self,
        authority: &Arc<Authority>,
    ) -> Option<(SelectedRenewal, RenewalRegistration)> {
        let mut entry = self.renewal.lock().expect("renewal registry lock");
        let entry = entry.as_mut()?;
        if Instant::now() >= entry.expires_at || !entry.work.as_ref()?.owner.matches(authority) {
            return None;
        }
        Some((
            entry.work.take()?,
            RenewalRegistration {
                registry: self.clone(),
                generation: entry.generation.clone(),
            },
        ))
    }
}

impl RenewalHook for Registry {
    fn selected(&self, authority: &Arc<Authority>) -> bool {
        self.renewal
            .lock()
            .expect("renewal registry lock")
            .as_ref()
            .is_some_and(|entry| {
                Instant::now() < entry.expires_at
                    && entry
                        .work
                        .as_ref()
                        .is_some_and(|work| work.owner.matches(authority))
            })
    }

    fn take_renewal(
        &self,
        authority: &Arc<Authority>,
        window: RenewalWindow,
    ) -> Option<RenewalAttempt> {
        let (work, registration) = self.claim_renewal(authority)?;
        let token = authority.token.clone();
        let dropped = work.evidence.clone();
        let write = async move {
            work.evidence
                .send_modify(|evidence| evidence.started = true);
            let outcome = work.isolated_store.renew(&token).await;
            let observed = match &outcome {
                Ok(lease) if window.accepts(&token, lease) => RenewalOutcome::Committed,
                Ok(_) => RenewalOutcome::Invalid,
                Err(OwnershipError::Unavailable(_)) => RenewalOutcome::Unavailable,
                Err(OwnershipError::Stale) => RenewalOutcome::Stale,
                Err(OwnershipError::IncompatibleProtocol) => RenewalOutcome::Incompatible,
                Err(OwnershipError::Invalid(_)) => RenewalOutcome::Invalid,
            };
            work.evidence
                .send_modify(|evidence| evidence.result = Some(observed));
            outcome
        };
        Some(RenewalAttempt::new(write, move || {
            dropped.send_modify(|evidence| evidence.caller_dropped = true);
            drop(registration);
        }))
    }
}
