use super::*;
use layer2_core::{sandbox::Sandbox, SandboxId};
use mongodb::bson::doc;
use std::future::Future;
use tokio::sync::oneshot;

#[path = "continuity.rs"]
mod continuity;
#[path = "controller.rs"]
mod controller;
#[path = "promotion_qualified.rs"]
mod qualified;
use controller::wire;

#[derive(Clone, Default)]
struct Registry(Arc<Mutex<RegistryState>>);

#[derive(Default)]
struct RegistryState {
    pointers: BTreeMap<Lane, String>,
    outage: bool,
    requests: Vec<Lane>,
}

impl Registry {
    fn point(&self, lane: Lane, release: char) {
        self.0
            .lock()
            .unwrap()
            .pointers
            .insert(lane, image(lane, release));
    }

    fn outage(&self, enabled: bool) {
        self.0.lock().unwrap().outage = enabled;
    }

    fn request_count(&self) -> usize {
        self.0.lock().unwrap().requests.len()
    }

    // This supplies the verified digest at the registry boundary. Elections
    // and admission of the candidate remain the controller's responsibility.
    fn resolve(&self, lane: Lane) -> Result<String, &'static str> {
        let mut registry = self.0.lock().unwrap();
        registry.requests.push(lane);
        if registry.outage {
            return Err("registry unavailable");
        }
        registry
            .pointers
            .get(&lane)
            .cloned()
            .ok_or("pointer missing")
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Release {
    active: String,
    // Preparation in progress. A refused candidate is not preparing.
    candidate: Option<String>,
    active_revision: u64,
}

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// Translate only between the acceptance scenarios and the published step (b)
// interfaces. All state transitions and durable reads must use the real
// controller, stores, and admissions. The sole missing binding is wire().
#[async_trait]
trait PointerReplica: Send + Sync {
    async fn bootstrap(&self) -> TestResult<()>;
    async fn enable_following(&self);
    async fn apply_target(&self, id: &str);
    async fn reconcile(&self, lane: Lane) -> TestResult<()>;
    async fn durable_release(&self, lane: Lane) -> Release;
    async fn refresh(&self) -> TestResult<()>;
    async fn expire_resolver_lease(&self, lane: Lane);
    // Candidate preparation follows the active plan's desired count.
    async fn resize_preparation(&self, plan: &NativePoolConfiguration, count: u32);
    async fn expire_release_settle(&self, lane: Lane);
    async fn retire_once(&self, plan: &NativePoolConfiguration) -> TestResult<()>;
    // Read durable promoted digests in order, excluding the bootstrap pin.
    async fn promotion_history(&self, lane: Lane) -> Vec<String>;
    async fn admit(&self, lane: Lane, key: &str) -> TestResult<Sandbox>;
    async fn sandbox(&self, id: &SandboxId) -> Sandbox;
    async fn request_update(&self, id: &SandboxId, version: &str) -> TestResult<Sandbox>;
    async fn run_pinned_update(&self, id: &SandboxId) -> TestResult<Sandbox>;
}

struct Rig {
    fixture: Fixture,
    first: Replica,
    second: Replica,
    registry: Registry,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let first = fixture.replica().await;
        let second = fixture.replica().await;
        let registry = Registry::default();
        registry.point(Lane::Agent, 'a');
        registry.point(Lane::Machine, '1');
        Self {
            fixture,
            first,
            second,
            registry,
        }
    }

    async fn controller<'a>(&'a self, replica: &'a Replica) -> Box<dyn PointerReplica + 'a> {
        let registry = self.registry.clone();
        wire(
            &self.fixture,
            replica,
            move |lane| registry.resolve(lane),
            true,
        )
        .await
    }

    async fn boot(&self, first: &dyn PointerReplica, second: &dyn PointerReplica) {
        self.seed_ready_defaults().await;
        first.bootstrap().await.unwrap();
        second.bootstrap().await.unwrap();
        first.enable_following().await;
    }

    async fn seed_ready_defaults(&self) {
        self.first.seed_defaults().await;
        for (lane, release) in [(Lane::Agent, 'a'), (Lane::Machine, '1')] {
            let plan = self.fixture.plan(lane, release, capacity());
            self.fixture
                .provider
                .configure_planned(&plan, TARGET)
                .await
                .unwrap();
            self.fixture.readiness(&plan, TARGET);
        }
    }

    async fn prepare(
        &self,
        controller: &dyn PointerReplica,
        lane: Lane,
        release: char,
    ) -> NativePoolConfiguration {
        self.registry.point(lane, release);
        controller.reconcile(lane).await.unwrap();
        let view = controller.durable_release(lane).await;
        assert_eq!(
            view.candidate.as_deref(),
            Some(image(lane, release).as_str())
        );
        let plan = self.fixture.plan(lane, release, capacity());
        let target = self.first.target_for(&plan).await;
        assert!(target.activated_at.is_none());
        assert!(!target.retiring);
        if target.binding.is_none() {
            controller.apply_target(&target.id).await;
        }
        plan
    }
}

fn runtime(sandbox: &Sandbox) -> &str {
    &sandbox
        .retained_runtime_binding()
        .expect("allocation or update must persist its runtime")
        .runtime_version
}

async fn at_gate<T>(receiver: oneshot::Receiver<T>) -> T {
    tokio::time::timeout(DEADLINE, receiver)
        .await
        .expect("operation must reach the gate")
        .expect("gated operation exited early")
}

async fn inject_unknown_commit(
    fixture: &Fixture,
    replica: &Replica,
    operation: impl Future<Output = TestResult<()>>,
) {
    let admin = Client::with_options(fixture.mongo.clone())
        .unwrap()
        .database("admin");
    // A write concern failure is reported after the command runs. A connection
    // failure before commit would only exercise retry of an unapplied write.
    let installed = admin
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": {"times": 5},
            "data": {
                "appName": &replica.mongo_app_name,
                "failCommands": ["commitTransaction"],
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected unknown promotion commit result",
                },
                "errorLabels": ["UnknownTransactionCommitResult"],
            },
        })
        .await
        .expect("requires a disposable Mongo replica set with test commands enabled");
    let result = tokio::time::timeout(DEADLINE, operation).await;
    let cleared = admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .expect("disable Mongo failpoint before assertions");
    let count = |response: &mongodb::bson::Document| {
        response
            .get_i64("count")
            .or_else(|_| response.get_i32("count").map(i64::from))
            .unwrap()
    };
    assert!(count(&cleared) > count(&installed));
    result
        .expect("promotion must reconcile the ambiguous commit")
        .unwrap();
}

fn assert_undrained(fixture: &Fixture, plan: &NativePoolConfiguration) {
    let provider = fixture.provider.inventory.lock().unwrap();
    let hash = &plan.config_sha256;
    assert!(!provider.effects.iter().any(|effect| match effect {
        Effect::Resize(key, 0)
        | Effect::BeginRetirement(key)
        | Effect::Drain(key, _)
        | Effect::Retire(key, _) => key == hash,
        _ => false,
    }));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn active_serves_through_replica_restart_while_candidate_is_below_target() {
    let mut rig = Rig::new().await;
    let (before, candidate) = {
        let leader = rig.controller(&rig.first).await;
        let follower = rig.controller(&rig.second).await;
        rig.boot(leader.as_ref(), follower.as_ref()).await;
        let before = leader.durable_release(Lane::Agent).await;
        let candidate = rig.prepare(leader.as_ref(), Lane::Agent, 'b').await;
        rig.fixture.readiness(&candidate, TARGET - 1);
        leader.reconcile(Lane::Agent).await.unwrap();
        let existing = leader.admit(Lane::Agent, "before-restart").await.unwrap();
        assert_eq!(runtime(&existing), before.active);
        (before, candidate)
    };
    rig.second = rig.fixture.restart(rig.second).await;
    let controller = rig.controller(&rig.second).await;
    controller.bootstrap().await.unwrap();
    let right = controller
        .admit(Lane::Agent, "after-restart")
        .await
        .unwrap();
    assert_eq!(runtime(&right), before.active);
    let after = controller.durable_release(Lane::Agent).await;
    assert_eq!(after.active, before.active);
    assert_eq!(after.active_revision, before.active_revision);
    assert!(rig
        .first
        .target_for(&candidate)
        .await
        .activated_at
        .is_none());
    assert_undrained(&rig.fixture, &candidate);
    assert_undrained(
        &rig.fixture,
        &rig.fixture.plan(Lane::Agent, 'a', capacity()),
    );
    rig.fixture.database.drop().await.unwrap();
}
