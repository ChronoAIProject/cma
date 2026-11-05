//! One participating process: renew, rebalance, revoke receivers, then release.
pub(crate) mod relay;
mod retirement;
use crate::{sandbox::LiveAgentRegistry, store::MongoSandboxStore};
use futures_util::{stream, StreamExt};
use layer2_core::{
    agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION},
    sandbox::store::AgentStore,
    AgentId,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::Instant;

pub(crate) struct AgentManager {
    pub coordinator: Arc<OwnershipCoordinator>,
    store: Arc<MongoSandboxStore>,
    registry: Arc<LiveAgentRegistry>,
    serving: AtomicBool,
    retired_cursor: std::sync::atomic::AtomicUsize,
    retired_since: Mutex<BTreeMap<AgentId, Instant>>,
    drain: tokio::sync::Mutex<bool>,
    handoffs: Mutex<BTreeMap<AgentId, Instant>>,
}

impl AgentManager {
    pub async fn register(
        store: Arc<MongoSandboxStore>,
        registry: Arc<LiveAgentRegistry>,
        address: String,
        capacity: u32,
    ) -> Result<Arc<Self>, String> {
        let coordinator = OwnershipCoordinator::register(
            store
                .agent_ownership
                .clone()
                .ok_or("ownership store is missing")?,
            Member {
                instance_id: store
                    .agent_manager_instance()
                    .ok_or("instance identity is missing")?
                    .into(),
                protocol: PROTOCOL_VERSION,
                relay_address: address,
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: capacity,
            },
        )
        .await
        .map_err(|_| "Agent Manager membership registration failed")?;
        registry.install_ownership(coordinator.clone());
        let manager = Arc::new(Self {
            coordinator,
            store,
            registry,
            serving: AtomicBool::new(false),
            retired_cursor: std::sync::atomic::AtomicUsize::new(0),
            retired_since: Mutex::default(),
            drain: tokio::sync::Mutex::new(false),
            handoffs: Mutex::new(BTreeMap::new()),
        });
        let weak = Arc::downgrade(&manager);
        // Start renewal during composition: dependency startup may exceed a lease.
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                if manager.coordinator.is_draining() {
                    return;
                }
                if !manager.coordinator.is_ready() {
                    manager.coordinator.fail().await;
                    return;
                }
                let candidates = match manager.coordinator.maintain().await {
                    Ok(candidates) => candidates,
                    Err(_) => {
                        tracing::error!(
                            "Agent Manager lost membership or ownership; restarting is required"
                        );
                        return;
                    }
                };
                if manager.retire_inactive().await.is_err() {
                    manager.coordinator.fail().await;
                    return;
                }
                for authority in candidates {
                    let started = *manager
                        .handoffs
                        .lock()
                        .unwrap()
                        .entry(authority.token.agent_id.clone())
                        .or_insert_with(Instant::now);
                    let busy = tokio::time::timeout(
                        Duration::from_secs(2),
                        manager.store.get_agent(&authority.token.agent_id),
                    )
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .flatten()
                    .is_none_or(|agent| {
                        agent.conversation.active_turn.is_some() || agent.work.lease.is_some()
                    });
                    if busy && started.elapsed() < Duration::from_secs(60) {
                        continue;
                    }
                    manager.registry.detach_owner(&authority.token);
                    if manager
                        .coordinator
                        .handoff(&authority, authority.revoke_transports())
                        .await
                        .is_err()
                    {
                        manager.coordinator.fail().await;
                        return;
                    }
                    manager
                        .handoffs
                        .lock()
                        .unwrap()
                        .remove(&authority.token.agent_id);
                }
            }
        });
        Ok(manager)
    }

    pub fn activate(&self) -> Result<(), String> {
        if !self.coordinator.is_ready() {
            return Err("Agent Manager membership expired during startup".into());
        }
        self.serving.store(true, Ordering::Release);
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        self.serving.load(Ordering::Acquire) && self.coordinator.is_ready()
    }

    pub async fn failed(&self) {
        let mut failure = self.coordinator.failure_signal();
        loop {
            if !self.coordinator.is_ready() && !self.coordinator.is_draining() {
                self.coordinator.fail().await;
                return;
            }
            tokio::select! {
                _ = failure.wait_for(|failed| *failed) => return,
                _ = tokio::time::sleep(Duration::from_secs(1)) => (),
            }
        }
    }

    pub async fn drain(&self) {
        let mut drained = self.drain.lock().await;
        if *drained {
            return;
        }
        self.serving.store(false, Ordering::Release);
        let owned = self.coordinator.begin_drain().await;
        let releasing = stream::iter(owned.into_iter().map(|authority| async move {
            self.registry.detach_owner(&authority.token);
            self.coordinator
                .handoff(&authority, authority.revoke_transports())
                .await
        }))
        .buffer_unordered(4)
        .collect::<Vec<_>>();
        let complete = tokio::time::timeout(Duration::from_secs(20), releasing).await;
        if complete.is_err() || complete.is_ok_and(|results| results.iter().any(Result::is_err)) {
            tracing::warn!("Agent Manager drain retained unconfirmed leases for expiry");
        }
        *drained = true;
    }
}
