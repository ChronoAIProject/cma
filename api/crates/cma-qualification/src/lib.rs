//! Local qualification checkpoints selected by immutable operation digests.

use layer2_core::{
    agent_ownership::CapturedOwner,
    qualification::{ControlCheckpoint, ControlContext, ControlSchedule},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::oneshot,
    time::{timeout_at, Instant},
};

#[cfg(unix)]
pub mod transport;

mod renewal;
pub use renewal::{RenewalArm, RenewalEvidence, RenewalOutcome};

const MAX_ARMS: usize = 32;
const MAX_LIFETIME: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub struct Registry {
    entries: Arc<Mutex<HashMap<ControlContext, Entry>>>,
    renewal: Arc<Mutex<Option<renewal::Entry>>>,
}

struct Entry {
    generation: Arc<()>,
    expires_at: Instant,
    checkpoint: Option<Checkpoint>,
}

struct Checkpoint {
    reached: oneshot::Sender<ControlSchedule>,
    release: oneshot::Receiver<()>,
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ArmError {
    #[error("checkpoint lifetime is out of bounds")]
    InvalidLifetime,
    #[error("a checkpoint for this request is already armed")]
    AlreadyArmed,
    #[error("qualification checkpoint capacity reached")]
    AtCapacity,
    #[error("captured authority belongs to a different Agent")]
    WrongAgent,
}

impl Registry {
    pub fn arm(
        &self,
        context: ControlContext,
        lifetime: Duration,
    ) -> Result<ArmedControl, ArmError> {
        if lifetime.is_zero() || lifetime > MAX_LIFETIME {
            return Err(ArmError::InvalidLifetime);
        }
        let now = Instant::now();
        let expires_at = now + lifetime;
        let mut entries = self.entries.lock().expect("checkpoint registry lock");
        entries.retain(|_, entry| entry.expires_at > now);
        if entries.contains_key(&context) {
            return Err(ArmError::AlreadyArmed);
        }
        if entries.len() >= MAX_ARMS {
            return Err(ArmError::AtCapacity);
        }
        let (on_reach, reached) = oneshot::channel();
        let (release, on_release) = oneshot::channel();
        let generation = Arc::new(());
        entries.insert(
            context,
            Entry {
                generation: generation.clone(),
                expires_at,
                checkpoint: Some(Checkpoint {
                    reached: on_reach,
                    release: on_release,
                }),
            },
        );
        Ok(ArmedControl {
            registration: Registration {
                registry: self.clone(),
                context,
                generation,
            },
            reached: Some(reached),
            captured_owner: None,
            release,
            expires_at,
        })
    }

    pub fn arm_with_owner(
        &self,
        context: ControlContext,
        lifetime: Duration,
        owner: CapturedOwner,
    ) -> Result<ArmedControl, ArmError> {
        if context.agent_digest() != owner.agent_digest() {
            return Err(ArmError::WrongAgent);
        }
        let mut arm = self.arm(context, lifetime)?;
        arm.captured_owner = Some(owner);
        Ok(arm)
    }

    fn take_checkpoint(
        &self,
        context: ControlContext,
    ) -> Option<(Checkpoint, Registration, Instant)> {
        let mut entries = self.entries.lock().expect("checkpoint registry lock");
        let entry = entries.get_mut(&context)?;
        if Instant::now() >= entry.expires_at {
            entries.remove(&context);
            return None;
        }
        Some((
            entry.checkpoint.take()?,
            Registration {
                registry: self.clone(),
                context,
                generation: entry.generation.clone(),
            },
            entry.expires_at,
        ))
    }
}

#[async_trait::async_trait]
impl ControlCheckpoint for Registry {
    async fn after_schedule(&self, context: ControlContext, outcome: ControlSchedule) {
        let Some((checkpoint, _registration, expires_at)) = self.take_checkpoint(context) else {
            return;
        };
        if checkpoint.reached.send(outcome).is_ok() {
            let _ = timeout_at(expires_at, checkpoint.release).await;
        }
    }
}

struct Registration {
    registry: Registry,
    context: ControlContext,
    generation: Arc<()>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut entries = self
            .registry
            .entries
            .lock()
            .expect("checkpoint registry lock");
        if entries
            .get(&self.context)
            .is_some_and(|entry| Arc::ptr_eq(&entry.generation, &self.generation))
        {
            entries.remove(&self.context);
        }
    }
}

/// Owns the one-use barrier. Dropping it disarms an unreached barrier or
/// resumes the original handler if it is already waiting.
pub struct ArmedControl {
    registration: Registration,
    reached: Option<oneshot::Receiver<ControlSchedule>>,
    captured_owner: Option<CapturedOwner>,
    release: oneshot::Sender<()>,
    expires_at: Instant,
}

/// Proof that the exact associated request reached this arm's barrier.
pub struct ReachedControl {
    generation: Arc<()>,
    pub schedule: ControlSchedule,
}

impl ArmedControl {
    pub async fn reached(&mut self) -> Option<ReachedControl> {
        let receiver = self.reached.as_mut()?;
        if Instant::now() >= self.expires_at {
            self.reached = None;
            return None;
        }
        let outcome = timeout_at(self.expires_at, receiver).await;
        self.reached = None;
        let schedule = outcome.ok()?.ok()?;
        if Instant::now() >= self.expires_at {
            return None;
        }
        Some(ReachedControl {
            generation: self.registration.generation.clone(),
            schedule,
        })
    }

    /// Consume the capture only while its linked request is still at the barrier.
    pub fn cancel_owner(&mut self, reached: &ReachedControl) -> bool {
        if !Arc::ptr_eq(&reached.generation, &self.registration.generation)
            || Instant::now() >= self.expires_at
            || self.release.is_closed()
        {
            return false;
        }
        self.captured_owner
            .take()
            .is_some_and(CapturedOwner::cancel)
    }

    /// Release only the barrier owned by this arm.
    pub fn release(self) {
        let Self {
            release,
            registration,
            ..
        } = self;
        let _ = release.send(());
        drop(registration);
    }
}

#[cfg(test)]
mod tests;
