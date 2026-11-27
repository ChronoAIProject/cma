use super::*;
use crate::store::agent_log_segments::AUDITS;
use crate::store::sandbox_mongo_integration_tests::agent_capacity_recovery_tests::ready_with_created;
use crate::store::sandbox_mongo_integration_tests::work_lease_deadline;
use crate::store::skill_result_test_support::{assert_format, enable_segments};
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, DEFAULT_MAX_OWNED_AGENTS, PROTOCOL_VERSION,
};
use layer2_core::sandbox::store::{AgentLogPublicationIdentity, AgentWorkClaimDetail};
use mongodb::event::command::CommandEvent;
use mongodb::event::EventHandler;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod sandbox_saves;

#[derive(Clone, Copy)]
enum Boundary {
    Begin(Instant),
    Aborted(Instant),
    Committed,
}

struct Fixture {
    admin: Client,
    client: Client,
    store: MongoSandboxStore,
    name: String,
    boundaries: Arc<Mutex<Vec<Boundary>>>,
}

impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let name = format!("cma_writer_retry_{}", uuid::Uuid::new_v4().simple());
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let boundaries = Arc::new(Mutex::new(Vec::new()));
        let captured = boundaries.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(name.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let boundary = match event {
                CommandEvent::Started(event)
                    if event.command.get_bool("startTransaction").unwrap_or(false) =>
                {
                    Some(Boundary::Begin(Instant::now()))
                }
                CommandEvent::Succeeded(event) if event.command_name == "abortTransaction" => {
                    Some(Boundary::Aborted(Instant::now()))
                }
                CommandEvent::Succeeded(event) if event.command_name == "commitTransaction" => {
                    Some(Boundary::Committed)
                }
                _ => None,
            };
            if let Some(boundary) = boundary {
                captured.lock().unwrap().push(boundary);
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        Self {
            admin,
            client,
            store,
            name,
            boundaries,
        }
    }

    async fn conflicts(&self, count: i32) {
        self.boundaries.lock().unwrap().clear();
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": count },
                "data": { "failCommands": ["update"], "appName": &self.name,
                    "errorCode": 112, "errorLabels": ["TransientTransactionError"] }
            })
            .await
            .unwrap();
    }

    async fn disable(&self) -> Vec<Boundary> {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off"
            })
            .await
            .unwrap();
        self.boundaries.lock().unwrap().clone()
    }

    async fn due(&self, kind: &str) -> (Sandbox, Workspace, Agent) {
        let (parent, mut workspace, mut agent) = capture_fixture(&self.store, kind).await;
        let revision = agent.revision;
        agent.work = AgentWorkState::new(AgentWorkStep::CloseProcess, NOW.into());
        agent.revision += 1;
        self.store
            .save_agent(revision, agent.clone())
            .await
            .unwrap();
        let revision = workspace.revision;
        workspace.status = WorkspaceStatus::Removing;
        workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Remove, NOW.into());
        workspace.revision += 1;
        self.store
            .save_workspace(revision, workspace.clone())
            .await
            .unwrap();
        (parent, workspace, agent)
    }

    async fn claim(
        &self,
        kind: &str,
        workspace: &Workspace,
        agent: &Agent,
    ) -> Result<Option<u64>, SandboxStoreError> {
        if kind == "agent" {
            self.store
                .claim_agent_work(&agent.id, "writer", NOW.into(), MINUTE.into())
                .await
                .map(|row| row.map(|row| row.revision))
        } else {
            self.store
                .claim_workspace_work(&workspace.id, "writer", NOW.into(), work_lease_deadline())
                .await
                .map(|row| row.map(|row| row.revision))
        }
    }
}

fn assert_paced(boundaries: &[Boundary], exhausted: bool) {
    assert_eq!(
        boundaries
            .iter()
            .filter(|b| matches!(b, Boundary::Begin(_)))
            .count(),
        5
    );
    assert_eq!(
        boundaries
            .iter()
            .filter(|b| matches!(b, Boundary::Aborted(_)))
            .count(),
        if exhausted { 5 } else { 4 }
    );
    assert_eq!(
        boundaries
            .iter()
            .filter(|b| matches!(b, Boundary::Committed))
            .count(),
        usize::from(!exhausted)
    );
    let mut waits = Vec::new();
    for pair in boundaries.windows(2) {
        if let [Boundary::Aborted(aborted), Boundary::Begin(next)] = pair {
            waits.push(next.duration_since(*aborted));
        }
    }
    assert_eq!(waits.len(), 4);
    for (wait, milliseconds) in waits.iter().zip([10, 20, 40, 80]) {
        // Observe the gap AFTER abort acknowledgment, excluding Mongo latency.
        // Tokio's timer has millisecond granularity.
        assert!(
            *wait >= Duration::from_millis(milliseconds - 1),
            "retry gap {wait:?}, expected {milliseconds} ms"
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn writer_claim_known_aborts_are_paced_and_commit_once() {
    let f = Fixture::new().await;
    for kind in ["agent", "workspace"] {
        let (parent, workspace, agent) = f.due(kind).await;
        f.conflicts(4).await;
        let result = f.claim(kind, &workspace, &agent).await;
        let boundaries = f.disable().await;
        assert_eq!(result.unwrap(), Some(3));
        assert_paced(&boundaries, false);
        let row = f
            .store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_i64("runtime_writer_claim_fence").unwrap(), 1);
        assert_eq!(f.store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    }
    f.client.database(&f.name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn writer_claim_exhaustion_is_bounded_without_outer_retry_or_mutation() {
    let f = Fixture::new().await;
    for kind in ["agent", "workspace"] {
        let (parent, workspace, agent) = f.due(kind).await;
        // More faults than the inner budget prove outer claim loops do not
        // multiply it. Leave the failpoint armed until the public call returns.
        f.conflicts(25).await;
        let result = if kind == "agent" {
            f.store
                .claim_agent_work_observed(&agent.id, "writer", NOW.into(), MINUTE.into())
                .await
                .map(|row| row.map(|row| row.revision))
                .map_err(|failure| {
                    assert_eq!(failure.detail, Some(AgentWorkClaimDetail::RetryExhausted));
                    failure.store_error
                })
        } else {
            f.claim(kind, &workspace, &agent).await
        };
        let boundaries = f.disable().await;
        let Err(SandboxStoreError::Unavailable(message)) = result else {
            panic!("expected bounded unavailable result, got {result:?}");
        };
        assert!(message.contains("phase=parent_fence attempt=5 driver_code=Some(112)"));
        assert_paced(&boundaries, true);
        assert_eq!(f.store.get_agent(&agent.id).await.unwrap(), Some(agent));
        assert_eq!(
            f.store.get_workspace(&workspace.id).await.unwrap(),
            Some(workspace)
        );
        let row = f
            .store
            .sandboxes
            .find_one(doc! { "_id": parent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("runtime_writer_claim_fence"));
        assert_eq!(f.store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    }
    f.client.database(&f.name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn agent_claim_distinguishes_read_driver_and_publication_custody_failures() {
    let f = Fixture::new().await;
    let (_, _, mut agent) = f.due("agent").await;
    for (command, detail) in [
        ("find", AgentWorkClaimDetail::Read),
        ("update", AgentWorkClaimDetail::Driver),
    ] {
        f.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": 1 },
                "data": { "failCommands": [command], "appName": &f.name, "errorCode": 2 }
            })
            .await
            .unwrap();
        let result = f
            .store
            .claim_agent_work_observed(&agent.id, "writer", NOW.into(), MINUTE.into())
            .await;
        f.disable().await;
        let error = result.unwrap_err();
        assert_eq!(error.detail, Some(detail));
        assert!(matches!(
            error.store_error,
            SandboxStoreError::Unavailable(_)
        ));
        assert_eq!(
            f.store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
    }

    let revision = agent.revision;
    agent.revision += 1;
    agent.status = AgentStatus::Starting;
    agent.work = AgentWorkState::new(AgentWorkStep::PublishReady, NOW.into());
    f.store.save_agent(revision, agent.clone()).await.unwrap();
    f.store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"pending_worker_log_adoption_v1": {"version": -1_i64}}},
        )
        .await
        .unwrap();
    let error = f
        .store
        .claim_agent_work_observed(&agent.id, "writer", NOW.into(), MINUTE.into())
        .await
        .unwrap_err();
    assert_eq!(error.detail, Some(AgentWorkClaimDetail::PublicationCustody));
    assert!(matches!(
        error.store_error,
        SandboxStoreError::Unavailable(_)
    ));
    f.client.database(&f.name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn owned_agent_claim_classifies_segmented_source_read_after_fencing() {
    let f = Fixture::new().await;
    let agent = ready_with_created(&f.store, "claim-source-read").await;
    enable_segments(&f.store, &agent).await;
    f.store
        .append_agent_log_keyed(
            &agent.id,
            &AgentLogPublicationIdentity {
                key: "claim-source-adoption".into(),
                fingerprint: [7; 32],
                appended_at: NOW.into(),
            },
            AgentLogKind::ConversationGap { frames: 1 },
        )
        .await
        .unwrap();
    assert_format(&f.store, &agent.id, true).await;
    let agent = f.store.get_agent(&agent.id).await.unwrap().unwrap();
    let parent_filter = doc! { "_id": agent.sandbox_id.as_str() };
    let original_fence = f
        .store
        .sandboxes
        .find_one(parent_filter.clone())
        .await
        .unwrap()
        .unwrap()
        .get_i64("runtime_writer_claim_fence")
        .unwrap();
    let store = f
        .store
        .clone()
        .enable_agent_ownership("writer".into())
        .await
        .unwrap();
    let coordinator = OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        Member {
            instance_id: "writer".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://localhost:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
        },
    )
    .await
    .unwrap();
    let authority = coordinator.acquire(&agent.id).await.unwrap().unwrap();

    // The audit read is inside segmented source validation after both fences.
    let namespace = format!("{}.{AUDITS}", f.name);
    f.boundaries.lock().unwrap().clear();
    f.admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1 },
            "data": { "failCommands": ["find"], "appName": &f.name,
                "namespace": &namespace, "errorCode": 2 }
        })
        .await
        .unwrap();
    let result = authority
        .run(store.claim_agent_work_observed(&agent.id, "writer", NOW.into(), MINUTE.into()))
        .await;
    let boundaries = f.disable().await;
    let failure = result.unwrap().unwrap_err();
    assert_eq!(failure.detail, Some(AgentWorkClaimDetail::Read));
    assert!(matches!(
        failure.store_error,
        SandboxStoreError::Unavailable(_)
    ));
    assert!(matches!(
        &boundaries[..],
        [Boundary::Begin(_), Boundary::Aborted(_)]
    ));
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );

    // Preserve the raw Mongo error so a known abort at the same read retries.
    f.boundaries.lock().unwrap().clear();
    f.admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1 },
            "data": { "failCommands": ["find"], "appName": &f.name,
                "namespace": &namespace, "errorCode": 112,
                "errorLabels": ["TransientTransactionError"] }
        })
        .await
        .unwrap();
    let result = authority
        .run(store.claim_agent_work_observed(&agent.id, "writer", NOW.into(), MINUTE.into()))
        .await;
    let boundaries = f.disable().await;
    let claimed = result
        .unwrap()
        .map_err(|failure| failure.detail)
        .unwrap()
        .expect("claim succeeds after the known abort");
    assert_eq!(claimed.revision, agent.revision + 1);
    assert!(matches!(
        &boundaries[..],
        [
            Boundary::Begin(_),
            Boundary::Aborted(_),
            Boundary::Begin(_),
            Boundary::Committed
        ]
    ));
    let row = store
        .sandboxes
        .find_one(parent_filter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_i64("runtime_writer_claim_fence").unwrap(),
        original_fence + 1
    );
    f.client.database(&f.name).drop().await.unwrap();
}
