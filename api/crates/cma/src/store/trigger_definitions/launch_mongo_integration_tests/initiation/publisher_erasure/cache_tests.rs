//! A scheduled run keeps its cache and checks erased authority after refusal.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
enum CacheCase {
    Normal,
    Erased,
    OrdinaryRefusal,
}

struct CachedDriver {
    fixture: Arc<Fixture>,
    reads: AtomicUsize,
    case: CacheCase,
}

#[async_trait::async_trait]
impl LaunchResourceDriver for CachedDriver {
    async fn trigger_publisher_erased(&self, launch: &Launch) -> Result<bool, LaunchResourceError> {
        Driver(self.fixture.clone())
            .trigger_publisher_erased(launch)
            .await
    }
    async fn trigger_revision(
        &self,
        id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, LaunchResourceError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Driver(self.fixture.clone())
            .trigger_revision(id, digest)
            .await
    }
    async fn frozen_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, LaunchResourceError> {
        Driver(self.fixture.clone()).frozen_profiles(target).await
    }
    async fn orchestration_paused(&self) -> Result<bool, LaunchResourceError> {
        Ok(false)
    }
    async fn requirements(
        &self,
        owner: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, LaunchResourceError> {
        Driver(self.fixture.clone())
            .requirements(owner, services)
            .await
    }
    async fn sandbox(
        &self,
        id: &layer2_core::SandboxId,
    ) -> Result<Option<layer2_core::sandbox::Sandbox>, LaunchResourceError> {
        Driver(self.fixture.clone()).sandbox(id).await
    }
    async fn workspace(
        &self,
        id: &layer2_core::WorkspaceId,
    ) -> Result<Option<layer2_core::sandbox::Workspace>, LaunchResourceError> {
        Driver(self.fixture.clone()).workspace(id).await
    }
    async fn agent(
        &self,
        id: &layer2_core::AgentId,
    ) -> Result<Option<layer2_core::sandbox::Agent>, LaunchResourceError> {
        Driver(self.fixture.clone()).agent(id).await
    }
    async fn matching_workspace(
        &self,
        owner: &SubjectId,
        sandbox: &layer2_core::SandboxId,
        profile: &layer2_core::WorkspaceProfileId,
    ) -> Result<Option<layer2_core::sandbox::Workspace>, LaunchResourceError> {
        Driver(self.fixture.clone())
            .matching_workspace(owner, sandbox, profile)
            .await
    }
    async fn apply(&self, effect: LaunchResourceEffect) -> Result<String, LaunchResourceError> {
        let LaunchResourceEffect::CreateAgent {
            initiation,
            request,
            reuse_existing,
            ..
        } = effect
        else {
            panic!("only the pending Agent effect is expected");
        };
        let initiation = *initiation.unwrap();
        let f = &self.fixture;
        let launch = f
            .seed
            .store
            .launch(initiation.launch_id(), &f.identity.caller.subject)
            .await
            .unwrap()
            .unwrap();
        let mut admission = f.admission(&launch).await;
        admission.first_message = request.first_message.map(|text| {
            FirstMessageRecord::new(
                admission.creation_key.clone(),
                text,
                admission.event_at.clone(),
            )
            .unwrap()
        });
        if matches!(self.case, CacheCase::Erased) {
            admission_tests::erase_publisher_records(f).await;
        }
        if matches!(self.case, CacheCase::OrdinaryRefusal) {
            return Err(LaunchResourceError::terminal(
                "agent_launch_rejected",
                "Agent creation was rejected",
            ));
        }
        f.resources
            .admit_catalogue_agent_with_trigger_initiator(admission, initiation, reuse_existing)
            .await
            .map(|result| result.creation().agent.id.to_string())
            .map_err(|_| {
                LaunchResourceError::terminal(
                    "agent_launch_rejected",
                    "Agent creation was rejected",
                )
            })
    }
}

async fn scheduled_cache(case: CacheCase) {
    let f = Arc::new(continuity_fixture().await);
    let launch = f
        .ready_for_agent(f.launch("scheduled-cache-erasure").await)
        .await;
    let driver = Arc::new(CachedDriver {
        fixture: f.clone(),
        reads: AtomicUsize::new(0),
        case,
    });
    let before = f
        .seed
        .client
        .database(&f.seed.database)
        .collection::<Document>("agents")
        .count_documents(doc! {})
        .await
        .unwrap();
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        driver.clone(),
        LaunchWorkerConfig::new("scheduled-cache-worker"),
    );
    assert!(worker
        .run_scheduled(
            &launch.id,
            std::time::SystemTime::now() + std::time::Duration::from_secs(61)
        )
        .await
        .unwrap());
    let current = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    let after = f
        .seed
        .client
        .database(&f.seed.database)
        .collection::<Document>("agents")
        .count_documents(doc! {})
        .await
        .unwrap();
    if !matches!(case, CacheCase::Normal) {
        assert_eq!(current.state, LaunchState::FailedTerminal);
        assert!(current.resolution.agent.is_pending());
        let failure = current.failure.unwrap();
        if matches!(case, CacheCase::Erased) {
            assert_eq!(failure.code, "trigger_retired");
            assert!(failure.safe_detail.contains("another Trigger"));
        } else {
            assert_eq!(failure.code, "agent_launch_rejected");
            assert_eq!(failure.safe_detail, "Agent creation was rejected");
        }
        assert_eq!(after, before);
        assert_eq!(driver.reads.load(Ordering::SeqCst), 2);
    } else {
        assert_eq!(current.work.step, LaunchWorkStep::WaitForAgent);
        assert!(current.failure.is_none());
        assert_eq!(after, before + 1);
        assert_eq!(driver.reads.load(Ordering::SeqCst), 1);
    }
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn scheduled_normal_launch_reads_its_frozen_revision_once() {
    scheduled_cache(CacheCase::Normal).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cached_agent_refusal_rechecks_erased_publisher_and_settles_typed_retired() {
    scheduled_cache(CacheCase::Erased).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cached_agent_refusal_keeps_an_ordinary_live_publisher_failure() {
    scheduled_cache(CacheCase::OrdinaryRefusal).await;
}
