//! One participating process: renew, rebalance, revoke receivers, then release.
mod configuration;
mod containment;
mod drain;
mod drain_contained;
#[cfg(test)]
mod drain_contained_mongo_tests;
mod drain_episode;
mod receiver_containment;
#[cfg(test)]
pub(crate) use drain::handoff as drain_receiver_for_test;
pub(crate) use drain_episode::DrainCause;
mod maintenance;
pub(crate) mod relay;
mod renewal;
mod retirement;
use crate::{sandbox::LiveAgentRegistry, store::MongoSandboxStore};
use layer2_core::{
    agent_ownership::{MaintenancePolicy, Member, OwnershipCoordinator, PROTOCOL_VERSION},
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
    drain_episode: OnceLock<drain_episode::DrainEpisode>,
    handoffs: Mutex<BTreeMap<AgentId, Instant>>,
    contained: receiver_containment::ContainedHandoffs,
    lifecycle: OnceLock<crate::agent_lifecycle::LifecycleDrain>,
    containment_times: Mutex<containment::IntentTimes>,
    supervisor: tokio::sync::Mutex<Option<tokio::task::JoinHandle<Result<(), ()>>>>,
    supervisor_outcome: OnceLock<Result<(), String>>,
}

impl AgentManager {
    #[cfg(test)]
    pub async fn register(
        store: Arc<MongoSandboxStore>,
        registry: Arc<LiveAgentRegistry>,
        address: String,
        capacity: u32,
        cleanup_pacing: bool,
        batch_renewal: bool,
    ) -> Result<Arc<Self>, String> {
        Self::register_with_policy_options(
            store,
            registry,
            address,
            capacity,
            cleanup_pacing,
            MaintenancePolicy::Legacy,
            batch_renewal,
        )
        .await
    }

    #[cfg(test)]
    pub async fn register_with_policy(
        store: Arc<MongoSandboxStore>,
        registry: Arc<LiveAgentRegistry>,
        address: String,
        capacity: u32,
        cleanup_pacing: bool,
        policy: MaintenancePolicy,
    ) -> Result<Arc<Self>, String> {
        Self::register_with_policy_options(
            store,
            registry,
            address,
            capacity,
            cleanup_pacing,
            policy,
            false,
        )
        .await
    }

    pub async fn register_with_policy_options(
        store: Arc<MongoSandboxStore>,
        registry: Arc<LiveAgentRegistry>,
        address: String,
        capacity: u32,
        cleanup_pacing: bool,
        policy: MaintenancePolicy,
        batch_renewal: bool,
    ) -> Result<Arc<Self>, String> {
        let coordinator = OwnershipCoordinator::prepare_with_policy_options(
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
            policy,
            batch_renewal,
        );
        registry.install_ownership(coordinator.clone());
        let manager = Arc::new(Self {
            coordinator,
            store,
            registry,
            serving: AtomicBool::new(false),
            retired_cursor: std::sync::atomic::AtomicUsize::new(0),
            retired_since: Mutex::default(),
            drain: tokio::sync::Mutex::new(false),
            drain_episode: OnceLock::new(),
            handoffs: Mutex::new(BTreeMap::new()),
            contained: Default::default(),
            lifecycle: OnceLock::new(),
            containment_times: Mutex::default(),
            supervisor: tokio::sync::Mutex::new(None),
            supervisor_outcome: OnceLock::new(),
        });
        Ok(manager)
    }

    /// Contain a transient receiver housekeeping failure to that receiver
    /// instead of stopping the process (#3069). Off unless configured.
    pub fn enable_receiver_containment(&self) {
        if !self.coordinator.contains_failures() {
            self.contained.enable();
        }
    }

    pub fn register_lifecycle(
        &self,
        lifecycle: crate::agent_lifecycle::LifecycleDrain,
    ) -> Result<(), String> {
        self.lifecycle
            .set(lifecycle)
            .map_err(|_| "Agent Manager lifecycle already registered".into())
    }

    pub async fn activate(self: &Arc<Self>) -> Result<(), String> {
        if self.lifecycle.get().is_none() {
            return Err("Agent Manager lifecycle drain is missing".into());
        }
        let mut supervisor = self.supervisor.lock().await;
        self.coordinator
            .activate()
            .await
            .map_err(|_| "Agent Manager membership registration failed")?;
        if !self.coordinator.is_ready() {
            return Err("Agent Manager membership expired during startup".into());
        }
        if supervisor.is_none() {
            *supervisor = Some(renewal::spawn(self));
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
