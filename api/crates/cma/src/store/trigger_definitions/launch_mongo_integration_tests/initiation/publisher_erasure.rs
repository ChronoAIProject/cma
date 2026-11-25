//! Real-store publisher-erasure boundaries for visitor Launch operations.
use super::*;
use crate::trigger::{
    LaunchResourceDriver, LaunchResourceEffect, LaunchResourceError, LaunchWorker,
    LaunchWorkerConfig,
};
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::{SandboxStore, WorktreeStore};
use layer2_core::sandbox::{
    AgentStatus, AgentWorkState, AgentWorkStep, FirstMessageRecord, WorktreeStatus,
};
mod admission_tests;
mod cache_tests;
mod checkpoint_tests;
mod counter_tests;
mod integrity_tests;
mod profile_race_tests;

async fn continuity_fixture() -> Fixture {
    let mut f = Fixture::new(false).await;
    let profiles = f
        .seed
        .profiles
        .read_trigger_profiles(&f.seed.target)
        .await
        .unwrap();
    f.parent.profile = layer2_core::profiles::SandboxProfileSnapshot::from(&profiles.sandbox);
    f.workspace.profile =
        layer2_core::profiles::WorkspaceProfileSnapshot::from(&profiles.workspace);
    // The retained-access fixture uses placeholder digests. This boundary
    // needs the exact, published pins that a real Launch admission persists.
    let db = f.seed.client.database(&f.seed.database);
    for (name, id, snapshot) in [
        (
            "sandboxes",
            f.parent.id.as_str(),
            mongodb::bson::serialize_to_bson(&f.parent.profile).unwrap(),
        ),
        (
            "workspaces",
            f.workspace.id.as_str(),
            mongodb::bson::serialize_to_bson(&f.workspace.profile).unwrap(),
        ),
    ] {
        assert_eq!(
            db.collection::<Document>(name)
                .update_one(doc! {"_id":id}, doc! {"$set":{"record.profile":snapshot}})
                .await
                .unwrap()
                .modified_count,
            1
        );
    }
    f
}

async fn erase_publisher(f: &Fixture) {
    let db = f.seed.client.database(&f.seed.database);
    let definitions = db.collection::<Document>("trigger_definitions");
    definitions
        .delete_many(doc! {"owner":f.seed.owner.as_str()})
        .await
        .unwrap();
    let revisions = db.collection::<Document>("trigger_definition_revisions");
    let mut rows = revisions
        .find(doc! {"owner":f.seed.owner.as_str()})
        .await
        .unwrap();
    while let Some(row) = rows.try_next().await.unwrap() {
        let mut revision = crate::store::trigger_definitions::documents::read_revision(&row)
            .unwrap()
            .revision;
        revision.erase_publisher();
        revisions
            .update_one(
                doc! {"_id":row.get("_id").unwrap().clone()},
                doc! {"$set":{
                    "owner":"deleted-account", "mutation_version":1_i64,
                    "sandbox_profile_id":"", "workspace_profile_id":"", "agent_profile_id":"",
                    "record":mongodb::bson::serialize_to_bson(&revision).unwrap(),
                }},
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_draft_start_returns_typed_retired_and_keeps_history() {
    let f = Fixture::new(false).await;
    let mut request = f.request("draft-publisher-erasure");
    request.start_if_ready = false;
    let launch = f
        .service
        .create_or_resume_as(&f.identity, &f.public.public_id, request)
        .await
        .unwrap()
        .launch;
    assert_eq!(launch.state, LaunchState::Draft);
    erase_publisher(&f).await;
    let result = f
        .service
        .start(
            &f.identity.caller,
            &launch.id,
            StartLaunch {
                trust_confirmed_digest: launch.review.trust_digest().map(str::to_owned),
                acknowledgements: Vec::new(),
            },
        )
        .await;
    assert!(matches!(result, Err(LaunchServiceError::TriggerRetired)));
    assert_eq!(
        f.service
            .read(&f.identity.caller, &launch.id)
            .await
            .unwrap(),
        launch
    );
    f.close().await;
}

struct Driver(Arc<Fixture>);
#[async_trait::async_trait]
impl LaunchResourceDriver for Driver {
    async fn trigger_publisher_erased(&self, launch: &Launch) -> Result<bool, LaunchResourceError> {
        self.0
            .seed
            .store
            .trigger_publisher_erased(
                &launch.trigger_id,
                &launch.revision_digest,
                &launch.public_id,
            )
            .await
            .map_err(|_| {
                LaunchResourceError::terminal(
                    "launch_resource_invalid",
                    "Stored publisher erasure evidence is invalid",
                )
            })
    }
    async fn trigger_revision(
        &self,
        id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, LaunchResourceError> {
        Ok(self
            .0
            .seed
            .store
            .trigger_revision(id, digest)
            .await
            .unwrap())
    }
    async fn frozen_profiles(
        &self,
        target: &TriggerTarget,
    ) -> Result<ResolvedTriggerProfiles, LaunchResourceError> {
        self.0
            .seed
            .profiles
            .read_trigger_profiles(target)
            .await
            .map_err(|_| {
                LaunchResourceError::terminal(
                    "frozen_profile_missing",
                    "The retained profile is unavailable",
                )
            })
    }
    async fn orchestration_paused(&self) -> Result<bool, LaunchResourceError> {
        Ok(false)
    }
    async fn requirements(
        &self,
        _: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, LaunchResourceError> {
        Ok(services
            .iter()
            .map(|service| ServiceRequirement {
                service: service.clone(),
                state: ServiceRequirementState::Satisfied,
            })
            .collect())
    }
    async fn sandbox(
        &self,
        id: &layer2_core::SandboxId,
    ) -> Result<Option<layer2_core::sandbox::Sandbox>, LaunchResourceError> {
        Ok(self.0.resources.get_sandbox(id).await.unwrap())
    }
    async fn workspace(
        &self,
        id: &layer2_core::WorkspaceId,
    ) -> Result<Option<layer2_core::sandbox::Workspace>, LaunchResourceError> {
        Ok(self.0.resources.get_workspace(id).await.unwrap())
    }
    async fn agent(
        &self,
        id: &layer2_core::AgentId,
    ) -> Result<Option<layer2_core::sandbox::Agent>, LaunchResourceError> {
        Ok(self.0.resources.get_agent(id).await.unwrap())
    }
    async fn admitted_trigger_agent(
        &self,
        launch: &Launch,
    ) -> Result<Option<layer2_core::sandbox::Agent>, LaunchResourceError> {
        self.0
            .resources
            .admitted_trigger_agent(launch)
            .await
            .map_err(|_| {
                LaunchResourceError::terminal(
                    "invalid_admission_receipt",
                    "The admission receipt is invalid",
                )
            })
    }
    async fn matching_workspace(
        &self,
        _: &SubjectId,
        _: &layer2_core::SandboxId,
        _: &layer2_core::WorkspaceProfileId,
    ) -> Result<Option<layer2_core::sandbox::Workspace>, LaunchResourceError> {
        panic!("erased publishers cannot start a new resource")
    }
    async fn apply(&self, effect: LaunchResourceEffect) -> Result<String, LaunchResourceError> {
        let LaunchResourceEffect::CreateAgent {
            initiation,
            request,
            reuse_existing,
            ..
        } = effect
        else {
            panic!("only the admitted Agent effect may run")
        };
        let initiation = *initiation.unwrap();
        let launch = self
            .0
            .seed
            .store
            .launch(initiation.launch_id(), &self.0.identity.caller.subject)
            .await
            .unwrap()
            .unwrap();
        let mut admission = self.0.admission(&launch).await;
        admission.first_message = request.first_message.map(|text| {
            FirstMessageRecord::new(
                admission.creation_key.clone(),
                text,
                admission.event_at.clone(),
            )
            .unwrap()
        });
        Ok(self
            .0
            .resources
            .admit_agent_with_trigger_initiator(admission, initiation, reuse_existing)
            .await
            .unwrap()
            .creation()
            .agent
            .id
            .to_string())
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_admitted_launch_without_agent_settles_typed_retired() {
    let f = Arc::new(Fixture::new(false).await);
    let launch = f.launch("admitted-publisher-erasure").await;
    assert!(launch.resolution.agent.is_pending());
    erase_publisher(&f).await;
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("erasure-launch-worker"),
    );
    assert!(worker
        .run_one(&launch.id, std::time::SystemTime::now())
        .await
        .unwrap());
    let settled = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, LaunchState::FailedTerminal);
    let failure = settled.failure.unwrap();
    assert_eq!(failure.code, "trigger_retired");
    assert!(failure.safe_detail.contains("another Trigger"));
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_keeps_created_agent_and_completed_launch_continuity() {
    for completed_before_erasure in [false, true] {
        let f = counter_tests::empty_message_fixture().await;
        let launch = f
            .ready_for_agent(f.launch("created-agent-erasure").await)
            .await;
        let f = Arc::new(f);
        let worker = LaunchWorker::new(
            Arc::new(f.seed.store.clone()),
            Arc::new(Driver(f.clone())),
            LaunchWorkerConfig::new("created-agent-erasure-worker"),
        );
        let now = std::time::SystemTime::now() + std::time::Duration::from_secs(61);
        assert!(worker.run_one(&launch.id, now).await.unwrap());
        let stored = f
            .seed
            .store
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.work.step, LaunchWorkStep::WaitForAgent);
        let mut agent = counter_tests::publish_agent_ready(&f, &stored).await;
        let db = f.seed.client.database(&f.seed.database);
        if !completed_before_erasure {
            erase_publisher(&f).await;
        }
        assert!(worker
            .run_one(&launch.id, now + std::time::Duration::from_secs(1))
            .await
            .unwrap());
        let ready = f
            .seed
            .store
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.state, LaunchState::Ready, "{:?}", ready.failure);
        assert!(ready.failure.is_none());
        agent.status = AgentStatus::Stopped;
        agent.stopped_at = Some(layer2_core::now_rfc3339());
        agent.revision += 1;
        db.collection::<Document>("agents")
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        assert!(worker
            .run_one(&launch.id, now + std::time::Duration::from_secs(2))
            .await
            .unwrap());
        let completed = f
            .seed
            .store
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.state, LaunchState::Completed);
        if completed_before_erasure {
            erase_publisher(&f).await;
        }
        assert_eq!(
            f.service
                .read(&f.identity.caller, &launch.id)
                .await
                .unwrap(),
            completed
        );
        assert_eq!(
            f.resources.get_agent(&agent.id).await.unwrap().unwrap(),
            agent
        );
        assert!(!worker
            .run_one(&launch.id, now + std::time::Duration::from_secs(3))
            .await
            .unwrap());
        db.drop().await.unwrap();
    }
}
