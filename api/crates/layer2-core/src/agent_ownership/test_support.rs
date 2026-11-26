//! Shared deterministic store for exercising the production coordinator. Mongo
//! integration tests separately verify real CAS, transactions and majority writes.

use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::time::Instant;

use super::*;

#[derive(Default)]
struct State {
    members: BTreeMap<String, Member>,
    owners: BTreeMap<AgentId, (u64, Option<OwnershipLease>)>,
    failures: BTreeMap<AgentId, (OwnershipToken, OwnershipFailure)>,
}

pub struct MemoryOwnershipStore {
    started: Instant,
    fixed_time_ms: Option<i64>,
    state: Arc<Mutex<State>>,
    partitioned: AtomicBool,
    diagnostics_enabled: AtomicBool,
}

impl Default for MemoryOwnershipStore {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            fixed_time_ms: None,
            state: Arc::default(),
            partitioned: AtomicBool::new(false),
            diagnostics_enabled: AtomicBool::new(true),
        }
    }
}

impl MemoryOwnershipStore {
    pub fn at_fixed_time(now_ms: i64) -> Self {
        Self {
            fixed_time_ms: Some(now_ms),
            ..Self::default()
        }
    }

    pub fn enable_receiver_diagnostics(&self, enabled: bool) {
        self.diagnostics_enabled.store(enabled, Ordering::Release);
    }

    pub fn recorded_failure(&self, id: &AgentId) -> Option<(OwnershipToken, OwnershipFailure)> {
        self.state.lock().unwrap().failures.get(id).cloned()
    }
    pub fn client(&self) -> Arc<Self> {
        Arc::new(Self {
            started: self.started,
            fixed_time_ms: self.fixed_time_ms,
            state: self.state.clone(),
            partitioned: AtomicBool::new(false),
            diagnostics_enabled: AtomicBool::new(true),
        })
    }

    pub fn partition(&self, partitioned: bool) {
        self.partitioned.store(partitioned, Ordering::Release);
    }

    fn now(&self) -> i64 {
        self.fixed_time_ms
            .unwrap_or_else(|| self.started.elapsed().as_millis() as i64 + 1)
    }

    fn accessible(&self) -> Result<(), OwnershipError> {
        if self.partitioned.load(Ordering::Acquire) {
            Err(OwnershipError::Unavailable(
                "synthetic store partition".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn snapshot(&self, state: &State) -> Membership {
        Membership {
            now_ms: self.now(),
            members: state.members.values().cloned().collect(),
        }
    }

    fn require<'a>(
        &self,
        state: &'a State,
        token: &OwnershipToken,
    ) -> Result<&'a OwnershipLease, OwnershipError> {
        state
            .owners
            .get(&token.agent_id)
            .and_then(|(_, lease)| lease.as_ref())
            .filter(|lease| lease.token == *token && lease.expires_at_ms > self.now())
            .filter(|_| {
                state.members.get(&token.instance_id).is_some_and(|member| {
                    member.is_live(self.now())
                        && !member.draining
                        && member.protocol == PROTOCOL_VERSION
                })
            })
            .ok_or(OwnershipError::Stale)
    }
}

#[async_trait::async_trait]
impl OwnershipStore for MemoryOwnershipStore {
    fn receiver_diagnostics_enabled(&self) -> bool {
        self.diagnostics_enabled.load(Ordering::Acquire)
    }

    async fn register(&self, mut member: Member) -> Result<Membership, OwnershipError> {
        self.accessible()?;
        if member.protocol != PROTOCOL_VERSION {
            return Err(OwnershipError::IncompatibleProtocol);
        }
        let mut state = self.state.lock().unwrap();
        if state.members.contains_key(&member.instance_id) {
            return Err(OwnershipError::Stale);
        }
        member.expires_at_ms = self.now() + DEFAULT_LEASE_MS as i64;
        member.owned_agents = 0;
        state.members.insert(member.instance_id.clone(), member);
        Ok(self.snapshot(&state))
    }

    async fn heartbeat(&self, id: &str, draining: bool) -> Result<Membership, OwnershipError> {
        self.accessible()?;
        let mut state = self.state.lock().unwrap();
        let member = state.members.get_mut(id).ok_or(OwnershipError::Stale)?;
        if !member.is_live(self.now()) || (member.draining && !draining) {
            return Err(OwnershipError::Stale);
        }
        member.expires_at_ms = self.now() + DEFAULT_LEASE_MS as i64;
        member.draining = draining;
        let snapshot = self.snapshot(&state);
        snapshot.require_compatible()?;
        Ok(snapshot)
    }

    async fn membership(&self) -> Result<Membership, OwnershipError> {
        self.accessible()?;
        let snapshot = self.snapshot(&self.state.lock().unwrap());
        snapshot.require_compatible()?;
        Ok(snapshot)
    }

    async fn owner(&self, id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.accessible()?;
        Ok(self
            .state
            .lock()
            .unwrap()
            .owners
            .get(id)
            .and_then(|(_, lease)| lease.as_ref())
            .filter(|lease| lease.expires_at_ms > self.now())
            .cloned())
    }

    async fn reconciliation_owners(
        &self,
        ids: &[&AgentId],
    ) -> Result<Vec<ReconciliationOwner>, OwnershipError> {
        self.accessible()?;
        let now = self.now();
        let state = self.state.lock().unwrap();
        Ok(ids
            .iter()
            .filter_map(|id| {
                let lease = state.owners.get(*id)?.1.as_ref()?;
                let remaining_ms = u64::try_from(lease.expires_at_ms.checked_sub(now)?).ok()?;
                Some(ReconciliationOwner {
                    token: lease.token.clone(),
                    remaining_ms,
                })
            })
            .collect())
    }

    async fn has_owner_history(&self, id: &AgentId) -> Result<bool, OwnershipError> {
        self.accessible()?;
        Ok(self.state.lock().unwrap().owners.contains_key(id))
    }

    async fn acquire(
        &self,
        id: &AgentId,
        instance: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError> {
        self.accessible()?;
        let mut state = self.state.lock().unwrap();
        if let Some(lease) = state
            .owners
            .get(id)
            .and_then(|(_, lease)| lease.as_ref())
            .filter(|lease| lease.expires_at_ms > self.now())
        {
            return Ok((lease.token.instance_id == instance).then(|| lease.clone()));
        }
        let membership = self.snapshot(&state);
        let previous_member = state
            .owners
            .get(id)
            .and_then(|(_, lease)| lease.as_ref())
            .map(|lease| lease.token.instance_id.as_str());
        if candidates(id, &membership, previous_member)?
            .first()
            .is_none_or(|member| member.instance_id != instance)
        {
            return Ok(None);
        }
        let (epoch, previous) = state.owners.get(id).cloned().unwrap_or((0, None));
        let epoch = epoch
            .checked_add(1)
            .ok_or_else(|| OwnershipError::Invalid("epoch exhausted".into()))?;
        state
            .members
            .get_mut(instance)
            .ok_or(OwnershipError::Stale)?
            .owned_agents += 1;
        if let Some(previous) = previous {
            if let Some(member) = state.members.get_mut(&previous.token.instance_id) {
                member.owned_agents = member.owned_agents.saturating_sub(1);
            }
        }
        let lease = OwnershipLease {
            token: OwnershipToken {
                agent_id: id.clone(),
                instance_id: instance.into(),
                epoch,
                protocol: PROTOCOL_VERSION,
            },
            expires_at_ms: self.now() + DEFAULT_LEASE_MS as i64,
        };
        state
            .owners
            .insert(id.clone(), (epoch, Some(lease.clone())));
        Ok(Some(lease))
    }

    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError> {
        self.accessible()?;
        let mut state = self.state.lock().unwrap();
        let mut lease = self.require(&state, token)?.clone();
        lease.expires_at_ms = self.now() + DEFAULT_LEASE_MS as i64;
        state
            .owners
            .insert(token.agent_id.clone(), (token.epoch, Some(lease.clone())));
        Ok(lease)
    }

    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.accessible()?;
        let mut state = self.state.lock().unwrap();
        if state
            .owners
            .get(&token.agent_id)
            .and_then(|(_, lease)| lease.as_ref())
            .is_some_and(|lease| lease.token == *token)
        {
            state
                .owners
                .insert(token.agent_id.clone(), (token.epoch, None));
            if let Some(member) = state.members.get_mut(&token.instance_id) {
                member.owned_agents = member.owned_agents.saturating_sub(1);
            }
        }
        Ok(())
    }

    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError> {
        self.accessible()?;
        self.require(&self.state.lock().unwrap(), token).map(|_| ())
    }

    async fn record_receiver_failure(
        &self,
        token: &OwnershipToken,
        failure: OwnershipFailure,
    ) -> Result<(), OwnershipError> {
        self.accessible()?;
        let mut state = self.state.lock().unwrap();
        let current = state
            .owners
            .get(&token.agent_id)
            .and_then(|(_, lease)| lease.as_ref());
        if current.is_none_or(|lease| lease.token != *token || lease.expires_at_ms <= self.now()) {
            return Err(OwnershipError::Stale);
        }
        let retained = state
            .failures
            .entry(token.agent_id.clone())
            .or_insert((token.clone(), failure));
        if retained.0 != *token {
            *retained = (token.clone(), failure);
        }
        Ok(())
    }
}
