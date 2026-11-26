use super::{
    MongoNativeWarmPools, NativePoolBinding, NativePoolConfiguration, PoolTarget, PoolTargetStore,
};
use crate::warm_pools::OperatorReplicas;
use async_trait::async_trait;
use layer1_public::rest::admin::warm_pools::{ResizeWarmPoolBody, WarmPoolError};
use layer3_adapter::{agent_sandbox::NativePoolDrain, HomeVolumeAttachment};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

struct RetirementHold {
    config_sha256: String,
    reached: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

pub(super) struct RetirementStore {
    store: Arc<MongoNativeWarmPools>,
    hold: Mutex<Option<RetirementHold>>,
    activation: Mutex<Option<RetirementHold>>,
}

impl RetirementStore {
    pub(super) fn new(store: Arc<MongoNativeWarmPools>) -> Self {
        Self {
            store,
            hold: Mutex::new(None),
            activation: Mutex::new(None),
        }
    }

    pub(super) fn hold_activation(&self, id: &str) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, observation) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        assert!(self
            .activation
            .lock()
            .unwrap()
            .replace(RetirementHold {
                config_sha256: id.into(),
                reached,
                resume,
            })
            .is_none());
        (observation, release)
    }

    pub(super) fn hold_marker(
        &self,
        plan: &NativePoolConfiguration,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, observation) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        assert!(self
            .hold
            .lock()
            .unwrap()
            .replace(RetirementHold {
                config_sha256: plan.config_sha256.clone(),
                reached,
                resume,
            })
            .is_none());
        (observation, release)
    }
}

#[async_trait]
impl PoolTargetStore for RetirementStore {
    async fn begin_candidate_effect(&self, target: &PoolTarget) -> Result<bool, WarmPoolError> {
        self.store.begin_candidate_effect(target).await
    }

    async fn candidate_pending(&self, target: &PoolTarget) -> Result<bool, WarmPoolError> {
        self.store.candidate_pending(target).await
    }

    async fn confirm_candidate_barrier(&self, target: &PoolTarget) -> Result<(), WarmPoolError> {
        self.store.confirm_candidate_barrier(target).await
    }

    async fn candidate_protected(&self, target: &PoolTarget) -> Result<bool, WarmPoolError> {
        self.store.candidate_protected(target).await
    }

    async fn activate(&self, id: &str) -> Result<(), WarmPoolError> {
        let gate = self
            .activation
            .lock()
            .unwrap()
            .take_if(|gate| gate.config_sha256 == id);
        if let Some(gate) = gate {
            gate.reached.send(()).expect("activation observer dropped");
            gate.resume.await.expect("activation was not released");
        }
        self.store.activate(id).await
    }

    async fn default_pin_conflicts(
        &self,
        plan: &NativePoolConfiguration,
        machine: bool,
    ) -> Result<bool, WarmPoolError> {
        self.store.default_pin_conflicts(plan, machine).await
    }

    async fn retirement_homes(
        &self,
        target: &PoolTarget,
        image: &str,
    ) -> Result<Option<Vec<HomeVolumeAttachment>>, WarmPoolError> {
        self.store.retirement_homes(target, image).await
    }

    async fn retirement_drain_homes(
        &self,
        target: &PoolTarget,
        image: &str,
    ) -> Result<Option<NativePoolDrain>, WarmPoolError> {
        self.store.retirement_drain_homes(target, image).await
    }

    async fn begin_retirement(&self, target: &PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        let hold = self
            .hold
            .lock()
            .unwrap()
            .take_if(|hold| hold.config_sha256 == target.plan.config_sha256);
        if let Some(hold) = hold {
            hold.reached.send(()).expect("retirement observer dropped");
            hold.resume
                .await
                .expect("retirement marker was not released");
        }
        self.store.begin_retirement(target).await
    }

    async fn remove_retired(&self, target: &PoolTarget) -> Result<(), WarmPoolError> {
        self.store.remove_retired(target).await
    }

    async fn list(&self) -> Result<Vec<PoolTarget>, WarmPoolError> {
        self.store.list().await
    }

    async fn find(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.store.find(id).await
    }

    async fn insert(&self, target: PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        self.store.insert(target).await
    }

    async fn resize(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
        operator_choice: bool,
    ) -> Result<PoolTarget, WarmPoolError> {
        self.store.resize(id, body, operator_choice).await
    }

    async fn latest_machine_operator_replicas(
        &self,
    ) -> Result<Option<OperatorReplicas>, WarmPoolError> {
        self.store.latest_machine_operator_replicas().await
    }

    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.store.claim().await
    }

    async fn claim_target(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.store.claim_target(id).await
    }

    async fn claim_retirement(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.store.claim_retirement(id).await
    }

    async fn bind(
        &self,
        target: &PoolTarget,
        binding: &NativePoolBinding,
    ) -> Result<(), WarmPoolError> {
        self.store.bind(target, binding).await
    }

    async fn finish(&self, target: &PoolTarget, success: bool) -> Result<(), WarmPoolError> {
        self.store.finish(target, success).await
    }
}
