use super::*;
use crate::warm_pools::tests::resize::Planner;
use layer3_adapter::{
    agent_sandbox::{NativePoolClaim, NativePoolClaimRequest, NativePoolCleanup, NativePoolTarget},
    AdapterError, HomeVolumeAttachment, Result,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;

#[derive(Default)]
pub(super) struct Inventory {
    pools: BTreeMap<String, Pool>,
    pub(super) effects: Vec<Effect>,
    claims: BTreeMap<(String, u64), NativePoolClaim>,
}

struct Pool {
    status: NativePoolStatus,
    applied: Option<NativePoolTarget>,
    retiring: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect {
    Configure(String, u32),
    Resize(String, u32),
    Claim(String),
    BeginRetirement(String),
    Drain(String, Vec<HomeVolumeAttachment>),
    Retire(String, Vec<HomeVolumeAttachment>),
}

#[derive(Clone, Default)]
pub(super) struct Provider {
    pub(super) inventory: Arc<Mutex<Inventory>>,
    inventory_gate: Arc<Mutex<Option<Gate<Vec<NativePoolStatus>>>>>,
    configure_gate: Arc<Mutex<Option<Gate<NativePoolConfiguration>>>>,
    lose_configuration_reply: Arc<AtomicBool>,
    claim_gate: Arc<Mutex<Option<Gate<NativePoolClaim>>>>,
    claim_request_gate: Arc<Mutex<Option<Gate<NativePoolClaimRequest>>>>,
}

struct Gate<T> {
    captured: oneshot::Sender<T>,
    resume: oneshot::Receiver<()>,
}

impl<T> Gate<T> {
    fn install(slot: &Mutex<Option<Self>>) -> (oneshot::Receiver<T>, oneshot::Sender<()>) {
        let (captured_tx, captured) = oneshot::channel();
        let (resume, resume_rx) = oneshot::channel();
        assert!(slot
            .lock()
            .unwrap()
            .replace(Self {
                captured: captured_tx,
                resume: resume_rx,
            })
            .is_none());
        (captured, resume)
    }

    async fn pause(slot: &Mutex<Option<Self>>, value: &T)
    where
        T: Clone,
    {
        let gate = slot.lock().unwrap().take();
        if let Some(gate) = gate {
            assert!(
                gate.captured.send(value.clone()).is_ok(),
                "gate observer dropped"
            );
            gate.resume.await.expect("held effect was not released");
        }
    }
}

fn rejected(message: &str) -> AdapterError {
    AdapterError::Rejected(message.into())
}

impl Inventory {
    pub(super) fn readiness(&mut self, plan: &NativePoolConfiguration, ready: u32) {
        let pool = self.exact_mut(&Planner::binding(plan)).unwrap();
        assert!(ready <= pool.status.desired_replicas);
        pool.status.observed = true;
        pool.status.ready_replicas = ready;
    }

    pub(super) fn status(&self, plan: &NativePoolConfiguration) -> &NativePoolStatus {
        let pool = &self.pools[&plan.config_sha256];
        assert_eq!(pool.status.binding, Planner::binding(plan));
        &pool.status
    }

    fn exact_mut(&mut self, binding: &NativePoolBinding) -> Result<&mut Pool> {
        self.pools
            .get_mut(&binding.config_sha256)
            .filter(|pool| &pool.status.binding == binding)
            .ok_or_else(|| rejected("provider did not find the exact test binding"))
    }

    fn resize(
        &mut self,
        binding: &NativePoolBinding,
        replicas: u32,
        target: Option<&NativePoolTarget>,
    ) -> Result<NativePoolStatus> {
        let pool = self.exact_mut(binding)?;
        if pool.retiring
            || pool.applied.as_ref().is_some_and(|applied| {
                target.is_none_or(|next| {
                    next.target_id != applied.target_id
                        || next.revision < applied.revision
                        || (next.revision == applied.revision && next.replicas != applied.replicas)
                })
            })
        {
            return Err(rejected("provider resize lost its authority"));
        }
        pool.applied = target.cloned();
        if replicas != pool.status.desired_replicas {
            pool.status.desired_replicas = replicas;
            pool.status.ready_replicas = 0;
            pool.status.observed = false;
        }
        let status = pool.status.clone();
        self.effects
            .push(Effect::Resize(binding.config_sha256.clone(), replicas));
        Ok(status)
    }
}

impl Provider {
    pub(super) fn for_replica(&self) -> Self {
        Self {
            inventory: self.inventory.clone(),
            ..Self::default()
        }
    }

    pub(super) fn hold_inventory(
        &self,
    ) -> (
        oneshot::Receiver<Vec<NativePoolStatus>>,
        oneshot::Sender<()>,
    ) {
        Gate::install(&self.inventory_gate)
    }

    pub(super) fn hold_claim(&self) -> (oneshot::Receiver<NativePoolClaim>, oneshot::Sender<()>) {
        Gate::install(&self.claim_gate)
    }

    pub(super) fn hold_claim_request(
        &self,
    ) -> (
        oneshot::Receiver<NativePoolClaimRequest>,
        oneshot::Sender<()>,
    ) {
        Gate::install(&self.claim_request_gate)
    }

    pub(super) fn hold_configuration(
        &self,
    ) -> (
        oneshot::Receiver<NativePoolConfiguration>,
        oneshot::Sender<()>,
    ) {
        Gate::install(&self.configure_gate)
    }

    pub(super) fn lose_next_configuration_reply(&self) {
        self.lose_configuration_reply.store(true, Ordering::SeqCst);
    }

    fn admit_claim(&self, request: &NativePoolClaimRequest) -> Result<NativePoolClaim> {
        request.validate()?;
        let mut inventory = self.inventory.lock().unwrap();
        inventory
            .effects
            .push(Effect::Claim(request.pool.config_sha256.clone()));
        let key = (request.home.sandbox_id.clone(), request.runtime_generation);
        if let Some(claim) = inventory.claims.get(&key) {
            return if claim.request == *request {
                Ok(claim.clone())
            } else {
                Err(rejected(
                    "test claim originally used a different pool or home",
                ))
            };
        }
        if inventory.exact_mut(&request.pool)?.retiring {
            return Err(rejected("the test claim pool has begun retirement"));
        }
        let claim = NativePoolClaim {
            request: request.clone(),
            name: format!("claim-{}", uuid::Uuid::new_v4().simple()),
            uid: format!("claim-{}", uuid::Uuid::new_v4().simple()),
            resource_version: "1".into(),
            sandbox_name: Some(format!("member-{}", uuid::Uuid::new_v4().simple())),
        };
        inventory.claims.insert(key, claim.clone());
        Ok(claim)
    }
}

#[async_trait]
impl NativePoolControl for Provider {
    fn plan(&self, spec: &NativePoolSpec) -> Result<NativePoolConfiguration> {
        Planner::default().plan(spec)
    }

    async fn configure_planned(
        &self,
        plan: &NativePoolConfiguration,
        replicas: u32,
    ) -> Result<NativePoolStatus> {
        Gate::pause(&self.configure_gate, plan).await;
        let binding = Planner::binding(plan);
        let mut inventory = self.inventory.lock().unwrap();
        let pool = inventory
            .pools
            .entry(plan.config_sha256.clone())
            .or_insert(Pool {
                status: NativePoolStatus {
                    binding: binding.clone(),
                    desired_replicas: replicas,
                    ready_replicas: 0,
                    observed: false,
                },
                applied: None,
                retiring: false,
            });
        assert_eq!(pool.status.binding, binding);
        // A replay can recover a binding after its configure response was lost.
        let status = pool.status.clone();
        inventory
            .effects
            .push(Effect::Configure(plan.config_sha256.clone(), replicas));
        if self.lose_configuration_reply.swap(false, Ordering::SeqCst) {
            return Err(rejected("configuration was applied and its reply was lost"));
        }
        Ok(status)
    }

    async fn inventory(&self) -> Result<Vec<NativePoolStatus>> {
        let rows: Vec<_> = self
            .inventory
            .lock()
            .unwrap()
            .pools
            .values()
            .map(|pool| pool.status.clone())
            .collect();
        Gate::pause(&self.inventory_gate, &rows).await;
        Ok(rows)
    }

    async fn resize(&self, binding: &NativePoolBinding, replicas: u32) -> Result<NativePoolStatus> {
        self.inventory
            .lock()
            .unwrap()
            .resize(binding, replicas, None)
    }

    async fn resize_fenced(
        &self,
        binding: &NativePoolBinding,
        target: &NativePoolTarget,
    ) -> Result<NativePoolStatus> {
        self.inventory
            .lock()
            .unwrap()
            .resize(binding, target.replicas, Some(target))
    }

    async fn claim(&self, request: &NativePoolClaimRequest) -> Result<NativePoolClaim> {
        Gate::pause(&self.claim_request_gate, request).await;
        let claim = self.admit_claim(request)?;
        Gate::pause(&self.claim_gate, &claim).await;
        Ok(claim)
    }

    async fn observe_claim(&self, original: &NativePoolClaim) -> Result<NativePoolClaim> {
        let inventory = self.inventory.lock().unwrap();
        let key = (
            original.request.home.sandbox_id.clone(),
            original.request.runtime_generation,
        );
        inventory
            .claims
            .get(&key)
            .filter(|claim| *claim == original)
            .cloned()
            .ok_or_else(|| rejected("original test claim is missing"))
    }

    async fn inventory_claims(&self) -> Result<Vec<NativePoolClaim>> {
        Ok(self
            .inventory
            .lock()
            .unwrap()
            .claims
            .values()
            .cloned()
            .collect())
    }

    async fn begin_retirement(&self, binding: &NativePoolBinding) -> Result<()> {
        let mut inventory = self.inventory.lock().unwrap();
        inventory
            .effects
            .push(Effect::BeginRetirement(binding.config_sha256.clone()));
        inventory.exact_mut(binding)?.retiring = true;
        Ok(())
    }

    async fn drain_pool(
        &self,
        binding: &NativePoolBinding,
        evidence: &layer3_adapter::agent_sandbox::NativePoolDrain,
    ) -> Result<()> {
        let mut inventory = self.inventory.lock().unwrap();
        inventory.effects.push(Effect::Drain(
            binding.config_sha256.clone(),
            evidence.homes.clone(),
        ));
        let pool = inventory.exact_mut(binding)?;
        assert!(
            pool.retiring,
            "drain must follow the provider retirement marker"
        );
        pool.status.desired_replicas = 0;
        pool.status.ready_replicas = 0;
        Ok(())
    }

    async fn retire_pool(
        &self,
        binding: &NativePoolBinding,
        retained_homes: &[HomeVolumeAttachment],
    ) -> Result<bool> {
        let mut inventory = self.inventory.lock().unwrap();
        inventory.effects.push(Effect::Retire(
            binding.config_sha256.clone(),
            retained_homes.to_vec(),
        ));
        assert!(inventory.exact_mut(binding)?.retiring);
        if !retained_homes.is_empty()
            || inventory
                .claims
                .values()
                .any(|claim| &claim.request.pool == binding)
        {
            return Ok(false);
        }
        inventory.pools.remove(&binding.config_sha256);
        Ok(true)
    }

    async fn adopt_home(&self, original: &NativePoolClaim) -> Result<HomeVolumeAttachment> {
        let claim = self.observe_claim(original).await?;
        let request = claim.request.home.clone();
        let member = claim.sandbox_name.as_ref().unwrap();
        let pvc_uid = format!("pvc-{member}");
        let receipt = layer3_adapter::HomeVolumeReceipt {
            volume_id: request.volume_id.clone(),
            request_sha256: request.computed_sha256(),
            storage_reference: format!("home-{member}"),
            storage_generation: pvc_uid.clone(),
            native_authority: Some(Box::new(
                layer3_adapter::agent_sandbox::NativePoolHomeOrigin {
                    sandbox_uid: format!("uid-{member}"),
                    pvc_uid,
                    claim,
                },
            )),
        };
        receipt.validate_for(&request)?;
        Ok(HomeVolumeAttachment { request, receipt })
    }

    async fn plan_cleanup(&self, _: &NativePoolCleanup) -> Result<NativePoolCleanup> {
        unreachable!("the test does not authorize claim cleanup")
    }

    async fn cleanup(&self, _: &NativePoolCleanup) -> Result<()> {
        unreachable!("the test does not authorize claim cleanup")
    }
}
