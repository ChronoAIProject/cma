use super::*;
use crate::runtime_pointer::{
    PointerObservation, PointerUnavailable, RuntimePointerController, RuntimePointerSource,
};
use crate::store::{
    agent_ownership::MongoAgentOwnership,
    released_runtimes::{BootWriter, StoreReleasedRuntime, COLLECTION, TRANSITIONS_COLLECTION},
};
use futures_util::TryStreamExt;
use layer1_public::rest::admin::warm_pools::ResizeWarmPoolBody;
use layer2_core::{
    agent_ownership::{Member, OwnershipStore, DEFAULT_MAX_OWNED_AGENTS, PROTOCOL_VERSION},
    released_runtime::{CandidateState, ReleasedLane, ReleasedTransition},
};
use layer3_adapter::{PinnedReleasedRuntime, ReleasedRuntimeSource};
use mongodb::bson::{DateTime, Document};

#[path = "first_publication.rs"]
mod first_publication;
#[path = "lifecycle.rs"]
mod lifecycle;
#[path = "lifecycle_release.rs"]
mod lifecycle_release;
#[path = "controller_startup.rs"]
mod startup;

struct Resolver<F>(F);

#[async_trait]
impl<F> RuntimePointerSource for Resolver<F>
where
    F: Fn(Lane) -> Result<String, &'static str> + Send + Sync,
{
    async fn observe(
        &self,
        lane: ReleasedLane,
    ) -> Result<Option<PointerObservation>, PointerUnavailable> {
        let lane = match lane {
            ReleasedLane::Agent => Lane::Agent,
            ReleasedLane::Machine => Lane::Machine,
        };
        (self.0)(lane)
            .map(|image| Some(PointerObservation::Verified(image)))
            .map_err(|_| PointerUnavailable)
    }
}

impl From<Lane> for ReleasedLane {
    fn from(lane: Lane) -> Self {
        match lane {
            Lane::Agent => Self::Agent,
            Lane::Machine => Self::Machine,
        }
    }
}

pub(super) struct ControllerReplica<'a> {
    controller: RuntimePointerController,
    pools: Arc<NativeWarmPools>,
    replica: &'a Replica,
    writer: BootWriter,
    lifecycle: tokio::sync::OnceCell<lifecycle::Lifecycle>,
}

pub(super) async fn wire<'a>(
    _: &Fixture,
    replica: &'a Replica,
    resolve: impl Fn(Lane) -> Result<String, &'static str> + Send + Sync + 'static,
    follow_pointer: bool,
) -> Box<dyn PointerReplica + 'a> {
    Box::new(bind_controller(replica, resolve, follow_pointer).await)
}

pub(super) async fn bind_controller(
    replica: &Replica,
    resolve: impl Fn(Lane) -> Result<String, &'static str> + Send + Sync + 'static,
    follow_pointer: bool,
) -> ControllerReplica<'_> {
    let pins = PinnedReleasedRuntime::new(image(Lane::Agent, 'a'), Some(image(Lane::Machine, '1')))
        .snapshot();
    let release = Arc::new(
        StoreReleasedRuntime::open(&replica.database, DEPLOYMENT, pins, true)
            .await
            .unwrap()
            .unwrap(),
    );
    let pools = replica.pools.clone().with_released_runtime(release.clone());
    let pointer =
        follow_pointer.then(|| Arc::new(Resolver(resolve)) as Arc<dyn RuntimePointerSource>);
    let writer = BootWriter {
        incarnation: uuid::Uuid::new_v4(),
        pod_uid: uuid::Uuid::new_v4(),
        build_digest: format!("sha256:{}", "a".repeat(64)),
    };
    let controller = RuntimePointerController::new(
        release,
        replica.store.clone(),
        pools.clone(),
        pointer,
        writer.clone(),
    );
    ControllerReplica {
        controller,
        pools,
        replica,
        writer,
        lifecycle: Default::default(),
    }
}

impl ControllerReplica<'_> {
    async fn update_writers(&self, field: &str, time: DateTime) {
        let update = self
            .replica
            .database
            .collection::<Document>(COLLECTION)
            .update_one(
                doc! {"_id": format!("{DEPLOYMENT}:writers")},
                doc! {"$set": {field: time}},
            )
            .await
            .unwrap();
        assert_eq!(update.matched_count, 1);
    }
}

#[async_trait]
impl PointerReplica for ControllerReplica<'_> {
    async fn bootstrap(&self) -> TestResult<()> {
        self.controller.register_boot().await?;
        self.controller.bootstrap().await?;
        tokio::time::timeout(DEADLINE, self.controller.await_active_ready(None)).await??;
        let ownership =
            MongoAgentOwnership::open(self.replica.database.client(), self.replica.database.name())
                .await?;
        let instance = self.writer.incarnation.to_string();
        ownership
            .register(Member {
                instance_id: instance.clone(),
                relay_address: format!("http://{instance}.example:8080"),
                protocol: PROTOCOL_VERSION,
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
            })
            .await?;
        self.controller
            .release
            .lanes
            .renew_writer(&self.writer, true)
            .await?;
        let date = DateTime::from_millis(DateTime::now().timestamp_millis() - 180_000);
        self.update_writers(&format!("writers.{instance}.ready_since"), date)
            .await;
        Ok(())
    }

    async fn enable_following(&self) {
        self.controller.set_following(true).await.unwrap();
    }

    async fn apply_target(&self, id: &str) {
        let row = self.replica.store.claim_target(id).await.unwrap().unwrap();
        self.pools
            .reconcile_candidate(&self.replica.provider, row)
            .await
            .unwrap();
    }

    async fn reconcile(&self, lane: Lane) -> TestResult<()> {
        self.controller.reconcile_once(lane.into()).await?;
        Ok(())
    }

    async fn durable_release(&self, lane: Lane) -> Release {
        let record = self
            .controller
            .release
            .lanes
            .load(lane.into())
            .await
            .unwrap()
            .unwrap();
        Release {
            active: record.active.image,
            active_revision: record.active.revision,
            candidate: record
                .candidate
                .filter(|candidate| !matches!(candidate.state, CandidateState::Refused { .. }))
                .map(|candidate| candidate.image),
        }
    }

    async fn refresh(&self) -> TestResult<()> {
        self.controller.release.refresh().await?;
        Ok(())
    }

    async fn expire_resolver_lease(&self, lane: Lane) {
        let key = match lane {
            Lane::Agent => "agent",
            Lane::Machine => "machine",
        };
        self.update_writers(
            &format!("leaders.{key}.expires_at"),
            DateTime::from_millis(0),
        )
        .await;
    }

    async fn resize_preparation(&self, plan: &NativePoolConfiguration, count: u32) {
        let candidate = self.replica.target_for(plan).await;
        let lane = if candidate.machine {
            Lane::Machine
        } else {
            Lane::Agent
        };
        let active = self.durable_release(lane).await.active;
        let active = self
            .pools
            .active_default_target(&active, candidate.machine)
            .await
            .unwrap();
        self.replica
            .store
            .resize(
                &active.id,
                &ResizeWarmPoolBody {
                    expected_revision: u64::try_from(active.revision).unwrap(),
                    target_replicas: count,
                },
                candidate.machine,
            )
            .await
            .unwrap();
    }

    async fn expire_release_settle(&self, lane: Lane) {
        let collection = self
            .replica
            .database
            .collection::<Document>("native_warm_pool_targets");
        let result = collection.update_many(
            doc! {"plan.deployment": DEPLOYMENT, "machine": if lane == Lane::Machine { mongodb::bson::Bson::Boolean(true) } else { doc! {"$ne": true}.into() }, "activated_at": {"$type": "date"}},
            vec![doc! {"$set":{"activated_at": {"$subtract": ["$activated_at", 180_000]}}}],
        ).await.unwrap();
        assert!(result.matched_count > 0);
    }

    async fn retire_once(&self, plan: &NativePoolConfiguration) -> TestResult<()> {
        let row = self.replica.target_for(plan).await;
        let write = self
            .replica
            .database
            .collection::<Document>("native_warm_pool_targets")
            .update_one(
                doc! {"_id": &row.id},
                doc! {"$set": {"reconcile_after": DateTime::from_millis(0)}},
            )
            .await?;
        assert_eq!(write.matched_count, 1);
        let target = self
            .replica
            .store
            .claim_retirement(&row.id)
            .await
            .map_err(|error| format!("claim retirement: {error:?}"))?
            .ok_or("retirement lease unavailable")?;
        self.pools
            .retire_claimed(&self.replica.provider, target)
            .await
            .map_err(|error| format!("retire target: {error:?}"))?;
        Ok(())
    }

    async fn promotion_history(&self, lane: Lane) -> Vec<String> {
        let lane = mongodb::bson::serialize_to_bson(&ReleasedLane::from(lane)).unwrap();
        let rows: Vec<Document> = self
            .replica
            .database
            .collection::<Document>(TRANSITIONS_COLLECTION)
            .find(doc! {"deployment": DEPLOYMENT, "transition.lane": lane})
            .sort(doc! {"transition.to_revision": 1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        let mut images = Vec::with_capacity(rows.len());
        for row in rows {
            let transition: ReleasedTransition = mongodb::bson::deserialize_from_document(
                row.get_document("transition").unwrap().clone(),
            )
            .unwrap();
            assert!(self
                .controller
                .release
                .lanes
                .committed(&transition)
                .await
                .unwrap());
            images.push(transition.to_image);
        }
        images
    }

    async fn admit(&self, lane: Lane, key: &str) -> TestResult<Sandbox> {
        self.lifecycle().await?.admit(lane, key).await
    }

    async fn sandbox(&self, id: &SandboxId) -> Sandbox {
        self.lifecycle().await.unwrap().sandbox(id).await.unwrap()
    }

    async fn request_update(&self, id: &SandboxId, version: &str) -> TestResult<Sandbox> {
        self.lifecycle().await?.request_update(id, version).await
    }

    async fn run_pinned_update(&self, id: &SandboxId) -> TestResult<Sandbox> {
        self.lifecycle().await?.drive_active(id).await
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo and the runtime pointer controller"]
async fn freeze_retains_candidate_until_the_other_replicas_provider_effect_settles() {
    let rig = Rig::new().await;
    let first_registry = rig.registry.clone();
    let second_registry = rig.registry.clone();
    let first = bind_controller(&rig.first, move |lane| first_registry.resolve(lane), true).await;
    let second =
        bind_controller(&rig.second, move |lane| second_registry.resolve(lane), true).await;
    rig.boot(&first, &second).await;
    rig.registry.point(Lane::Agent, 'b');
    first.reconcile(Lane::Agent).await.unwrap();
    let plan = rig.fixture.plan(Lane::Agent, 'b', capacity());
    let target = rig.first.target_for(&plan).await;
    rig.first.provider.lose_next_configuration_reply();
    let (observed, resume) = rig.first.provider.hold_configuration();
    let freeze = async {
        assert_eq!(at_gate(observed).await, plan);
        second.controller.set_following(false).await.unwrap();
        let lane = second
            .controller
            .release
            .lanes
            .load(ReleasedLane::Agent)
            .await
            .unwrap()
            .unwrap();
        assert!(lane.frozen);
        assert_eq!(lane.active.image, image(Lane::Agent, 'a'));
        assert_eq!(lane.candidate.unwrap().image, image(Lane::Agent, 'b'));
        resume.send(()).unwrap();
    };
    tokio::join!(first.apply_target(&target.id), freeze);
    let targets = rig
        .second
        .database
        .collection::<Document>("native_warm_pool_targets");
    let pending = targets
        .find_one(doc! {"_id": &target.id})
        .await
        .unwrap()
        .unwrap();
    assert!(pending
        .get_document("preparation")
        .unwrap()
        .contains_key("pending_revision"));
    second.controller.set_following(false).await.unwrap();
    assert!(second
        .durable_release(Lane::Agent)
        .await
        .candidate
        .is_some());
    let modified = targets
        .update_one(
            doc! {"_id": &target.id},
            doc! {"$set":{
                "preparation.effect_until": DateTime::from_millis(0),
                "reconcile_after": DateTime::from_millis(0),
            }},
        )
        .await
        .unwrap();
    assert_eq!(modified.matched_count, 1);
    second.apply_target(&target.id).await;
    assert!(rig
        .fixture
        .provider
        .inventory
        .lock()
        .unwrap()
        .effects
        .contains(&Effect::BeginRetirement(plan.config_sha256)));
    second.controller.set_following(false).await.unwrap();
    let lane = first
        .controller
        .release
        .lanes
        .load(ReleasedLane::Agent)
        .await
        .unwrap()
        .unwrap();
    assert!(lane.frozen && lane.candidate.is_none());
    assert_eq!(lane.active.image, image(Lane::Agent, 'a'));
    assert!(first.promotion_history(Lane::Agent).await.is_empty());
    rig.fixture.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo and the runtime pointer boot-effect fence"]
async fn expired_boot_writer_blocks_the_peer_until_its_provider_effect_settles() {
    use layer2_core::sandbox::SandboxStoreError;

    let rig = Rig::new().await;
    let registry = rig.registry.clone();
    let first = bind_controller(&rig.first, move |lane| registry.resolve(lane), true).await;
    let second = bind_controller(
        &rig.second,
        |_| panic!("the booting writer does not resolve"),
        false,
    )
    .await;
    rig.seed_ready_defaults().await;
    first.bootstrap().await.unwrap();
    first.enable_following().await;
    second.controller.register_boot().await.unwrap();
    let lanes = &second.controller.release.lanes;
    lanes.protect_boot_effect(&second.writer).await.unwrap();
    let path = format!("writers.{}", second.writer.incarnation);
    second
        .update_writers(&format!("{path}.expires_at"), DateTime::from_millis(0))
        .await;
    lanes.remove_writer(&second.writer).await.unwrap();
    rig.registry.point(Lane::Agent, 'b');
    assert!(matches!(
        first.controller.reconcile_once(ReleasedLane::Agent).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(first.durable_release(Lane::Agent).await.candidate.is_none());
    assert!(first.promotion_history(Lane::Agent).await.is_empty());
    second
        .update_writers(
            &format!("{path}.boot_effect_until"),
            DateTime::from_millis(0),
        )
        .await;
    lanes.remove_writer(&second.writer).await.unwrap();
    first.reconcile(Lane::Agent).await.unwrap();
    assert_eq!(
        second.durable_release(Lane::Agent).await.candidate,
        Some(image(Lane::Agent, 'b'))
    );
    rig.fixture.database.drop().await.unwrap();
}
