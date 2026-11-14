//! Receiver-side Agent authority. Challenge time precedes the manager's Mongo
//! renewal, so this monotonic lease expires first without shared wall clocks.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use layer3_adapter::owner_protocol::{
    OwnerAcknowledgement, OwnerChallenge, OwnerConfirmation, TransportOwner,
    OWNER_PROTOCOL_VERSION, RECEIVER_LEASE_MS,
};
use tokio::sync::{watch, OwnedRwLockReadGuard};
use tokio::time::Instant;

use crate::failure::{Result, RuntimeError};

mod persistence;
mod session;
pub(crate) use session::OwnerSession;

const MAX_GATES: usize = 4_096;
const MAX_CHALLENGES: usize = 16;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    owner: Option<TransportOwner>,
    required: bool,
    retired: bool,
    revision: u64,
}

struct Challenge {
    owner: TransportOwner,
    deadline: Instant,
}

struct GateState {
    record: Record,
    deadline: Option<Instant>,
    challenges: BTreeMap<String, Challenge>,
}

struct Gate {
    state: Arc<tokio::sync::Mutex<GateState>>,
    effects: Arc<tokio::sync::RwLock<()>>,
    changed: watch::Sender<()>,
    path: PathBuf,
}

pub(crate) struct OwnershipPermit {
    _effect: OwnedRwLockReadGuard<()>,
    deadline: Option<Instant>,
}

impl OwnershipPermit {
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

pub(crate) struct ManagerOwnership {
    root: PathBuf,
    gates: Mutex<BTreeMap<String, Arc<Gate>>>,
}

impl ManagerOwnership {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            gates: Mutex::new(BTreeMap::new()),
        }
    }

    fn gate(&self, agent: &str) -> Result<Arc<Gate>> {
        use sha2::{Digest, Sha256};
        if agent.is_empty() || agent.len() > 128 {
            return Err(refused());
        }
        let mut gates = self.gates.lock().unwrap();
        if let Some(gate) = gates.get(agent) {
            return Ok(gate.clone());
        }
        if gates.len() >= MAX_GATES {
            gates.retain(|_, gate| {
                Arc::strong_count(gate) > 1
                    || gate.state.try_lock().map_or(true, |state| {
                        state.deadline.is_some_and(|until| until > Instant::now())
                            || state
                                .challenges
                                .values()
                                .any(|challenge| challenge.deadline > Instant::now())
                    })
            });
        }
        if gates.len() >= MAX_GATES {
            return Err(refused());
        }
        let path = self
            .root
            .join(format!("{:x}.json", Sha256::digest(agent.as_bytes())));
        let record = persistence::load(&path, agent)?;
        let gate = Arc::new(Gate {
            state: Arc::new(tokio::sync::Mutex::new(GateState {
                record,
                deadline: None,
                challenges: BTreeMap::new(),
            })),
            effects: Arc::new(tokio::sync::RwLock::new(())),
            changed: watch::channel(()).0,
            path,
        });
        gates.insert(agent.to_owned(), gate.clone());
        Ok(gate)
    }

    pub(crate) async fn challenge(&self, owner: TransportOwner) -> Result<OwnerChallenge> {
        owner.validate().map_err(|_| refused())?;
        let gate = self.gate(&owner.agent_id)?;
        let mut state = gate.state.lock().await;
        admissible(&state.record, &owner)?;
        state
            .challenges
            .retain(|_, challenge| challenge.deadline > Instant::now());
        if state.challenges.len() >= MAX_CHALLENGES {
            return Err(refused());
        }
        let nonce = uuid::Uuid::new_v4().to_string();
        state.challenges.insert(
            nonce.clone(),
            Challenge {
                owner,
                deadline: Instant::now() + Duration::from_millis(RECEIVER_LEASE_MS),
            },
        );
        Ok(OwnerChallenge {
            protocol: OWNER_PROTOCOL_VERSION,
            nonce,
            valid_for_ms: RECEIVER_LEASE_MS,
        })
    }

    pub(crate) async fn confirm(
        &self,
        confirmation: OwnerConfirmation,
    ) -> Result<OwnerAcknowledgement> {
        let owner = confirmation.owner;
        owner.validate().map_err(|_| refused())?;
        let gate = self.gate(&owner.agent_id)?;
        let refresh = {
            let state = gate.state.lock().await;
            state.record.owner.as_ref() == Some(&owner)
                && state.record.required
                && !state.record.retired
        };
        // Renewals may proceed during an already-authorized long control call.
        // A different epoch must wait for all prior effects to finish.
        let _exclusive = if refresh {
            None
        } else {
            Some(gate.effects.clone().write_owned().await)
        };
        let mut state = gate.state.lock().await;
        let challenge = state
            .challenges
            .remove(&confirmation.nonce)
            .ok_or_else(refused)?;
        if challenge.owner != owner || challenge.deadline <= Instant::now() {
            return Err(refused());
        }
        admissible(&state.record, &owner)?;
        let mut next = state.record.clone();
        if next.owner.as_ref() != Some(&owner) || !next.required {
            next.revision = next.revision.checked_add(1).ok_or_else(refused)?;
        }
        next.owner = Some(owner.clone());
        next.required = true;
        next.retired = false;
        if !refresh {
            persistence::save(&gate.path, &next)?;
        }
        state.record = next;
        state.deadline = Some(if refresh {
            state
                .deadline
                .map_or(challenge.deadline, |until| until.max(challenge.deadline))
        } else {
            challenge.deadline
        });
        gate.changed.send_replace(());
        Ok(OwnerAcknowledgement {
            owner,
            protocol: OWNER_PROTOCOL_VERSION,
        })
    }

    pub(crate) async fn revoke(&self, owner: TransportOwner) -> Result<OwnerAcknowledgement> {
        owner.validate().map_err(|_| refused())?;
        let gate = self.gate(&owner.agent_id)?;
        let _exclusive = gate.effects.clone().write_owned().await;
        let mut state = gate.state.lock().await;
        if state
            .record
            .owner
            .as_ref()
            .is_some_and(|current| current.epoch > owner.epoch)
        {
            return Ok(OwnerAcknowledgement {
                owner,
                protocol: OWNER_PROTOCOL_VERSION,
            });
        }
        if state
            .record
            .owner
            .as_ref()
            .is_some_and(|current| current.epoch == owner.epoch && current != &owner)
        {
            return Err(refused());
        }
        let mut next = state.record.clone();
        next.owner = Some(owner.clone());
        next.required = true;
        next.retired = true;
        next.revision = next.revision.checked_add(1).ok_or_else(refused)?;
        persistence::save(&gate.path, &next)?;
        state.record = next;
        state.deadline = None;
        state.challenges.clear();
        gate.changed.send_replace(());
        Ok(OwnerAcknowledgement {
            owner,
            protocol: OWNER_PROTOCOL_VERSION,
        })
    }

    /// Operator rollback only, after a no-overlap manager drain. Preserve the
    /// watermark and retire all previously authenticated owned/legacy sessions.
    pub(crate) async fn reset_legacy(&self, owner: TransportOwner) -> Result<OwnerAcknowledgement> {
        let gate = self.gate(&owner.agent_id)?;
        let _exclusive = gate.effects.clone().write_owned().await;
        let mut state = gate.state.lock().await;
        if state.record.owner.as_ref() != Some(&owner)
            || state
                .deadline
                .is_some_and(|deadline| deadline > Instant::now())
        {
            return Err(refused());
        }
        let mut next = state.record.clone();
        next.required = false;
        next.retired = true;
        next.revision = next.revision.checked_add(1).ok_or_else(refused)?;
        persistence::save(&gate.path, &next)?;
        state.record = next;
        state.challenges.clear();
        gate.changed.send_replace(());
        Ok(OwnerAcknowledgement {
            owner,
            protocol: OWNER_PROTOCOL_VERSION,
        })
    }

    pub(crate) async fn session(
        &self,
        agent: &str,
        owner: Option<TransportOwner>,
    ) -> Result<OwnerSession> {
        if owner.as_ref().is_some_and(|owner| owner.agent_id != agent) {
            return Err(refused());
        }
        let gate = self.gate(agent)?;
        let revision = gate.state.lock().await.record.revision;
        let session = OwnerSession {
            gate,
            owner,
            revision,
        };
        session.permit().await?;
        Ok(session)
    }
}

fn admissible(record: &Record, owner: &TransportOwner) -> Result<()> {
    if record.owner.as_ref().is_some_and(|previous| {
        previous.epoch > owner.epoch
            || previous.epoch == owner.epoch && (previous != owner || record.retired)
    }) {
        Err(refused())
    } else {
        Ok(())
    }
}

fn refused() -> RuntimeError {
    RuntimeError::rejected(
        "manager_ownership_stale",
        "Agent Manager transport authority is missing, stale or expired",
    )
}

#[cfg(test)]
mod tests;
