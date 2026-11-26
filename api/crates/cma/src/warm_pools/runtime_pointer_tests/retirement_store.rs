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
    selection: Mutex<Option<RetirementHold>>,
    selected: Mutex<Option<RetirementHold>>,
}

impl RetirementStore {
    pub(super) fn new(store: Arc<MongoNativeWarmPools>) -> Self {
        Self {
            store,
            hold: Mutex::new(None),
            activation: Mutex::new(None),
            selection: Mutex::new(None),
            selected: Mutex::new(None),
        }
    }

    pub(super) fn hold_activation(&self, id: &str) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        Self::install_hold(&self.activation, id)
    }

    /// Capture the default target list returned after activation, before the
    /// controller publishes the first pointer from the selected target.
    pub(super) fn hold_selection(&self, id: &str) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        Self::install_hold(&self.selection, id)
    }

    fn install_hold(
        slot: &Mutex<Option<RetirementHold>>,
        id: &str,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, observation) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        assert!(slot
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
        Self::install_hold(&self.hold, &plan.config_sha256)
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
        self.store.activate(id).await?;
        if let Some(gate) = self
            .selection
            .lock()
            .unwrap()
            .take_if(|gate| gate.config_sha256 == id)
        {
            *self.selected.lock().unwrap() = Some(gate);
        }
        Ok(())
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
        let rows = self.store.list().await?;
        let gate = self.selected.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.reached.send(()).expect("selection observer dropped");
            gate.resume.await.expect("selection was not released");
        }
        Ok(rows)
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
