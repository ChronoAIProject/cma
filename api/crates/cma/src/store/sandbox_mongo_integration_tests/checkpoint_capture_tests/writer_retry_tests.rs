use super::*;
use mongodb::event::command::CommandEvent;
use mongodb::event::EventHandler;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
                .claim_workspace_work(&workspace.id, "writer", NOW.into(), MINUTE.into())
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
        let result = f.claim(kind, &workspace, &agent).await;
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
