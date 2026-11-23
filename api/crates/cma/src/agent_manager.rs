//! One participating process: renew, rebalance, revoke receivers, then release.
mod containment;
mod drain;
mod maintenance;
pub(crate) mod relay;
mod renewal;
mod retirement;
use crate::{sandbox::LiveAgentRegistry, store::MongoSandboxStore};
use layer2_core::{
    agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION},
    sandbox::store::AgentStore,
    AgentId,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
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
    contained: containment::ContainedHandoffs,
    lifecycle: OnceLock<crate::agent_lifecycle::LifecycleDrain>,
}

impl AgentManager {
    pub async fn register(
        store: Arc<MongoSandboxStore>,
        registry: Arc<LiveAgentRegistry>,
        address: String,
        capacity: u32,
        cleanup_pacing: bool,
    ) -> Result<Arc<Self>, String> {
        let coordinator = OwnershipCoordinator::register_with_cleanup_pacing(
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
            cleanup_pacing,
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
            contained: Default::default(),
            lifecycle: OnceLock::new(),
        });
        renewal::spawn(&manager);
        Ok(manager)
    }

    /// Contain a transient receiver housekeeping failure to that receiver
    /// instead of stopping the process (#3069). Off unless configured.
    pub fn enable_receiver_containment(&self) {
        self.contained.enable();
    }

    pub fn register_lifecycle(
        &self,
        lifecycle: crate::agent_lifecycle::LifecycleDrain,
    ) -> Result<(), String> {
        self.lifecycle
            .set(lifecycle)
            .map_err(|_| "Agent Manager lifecycle already registered".into())
    }

    pub fn activate(&self) -> Result<(), String> {
        if self.lifecycle.get().is_none() {
            return Err("Agent Manager lifecycle drain is missing".into());
        }
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
}
