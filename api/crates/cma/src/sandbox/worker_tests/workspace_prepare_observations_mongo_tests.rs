use super::*;
use crate::store::MongoSandboxStore;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{SandboxEventKind, WorkspacePrepareObservations};
use layer3_adapter::provider::SandboxClient;
use layer3_adapter::workspace_prepare_observation::{Code, Stage};
use mongodb::Client;
use serde_json::json;

type Requests = Arc<Mutex<Vec<RequestFrame>>>;

async fn fail_prepare(
    State(requests): State<Requests>,
    Json(frame): Json<RequestFrame>,
) -> Json<ResponseFrame> {
    let operation = frame.operation();
    assert!(matches!(
        operation,
        WireOperation::PrepareWorkspace | WireOperation::RecoverWorkspacePrepare
    ));
    let mut requests = requests.lock().unwrap();
    let retryable = !requests
        .iter()
        .any(|request| request.operation() == operation);
    requests.push(frame);
    Json(ResponseFrame::Failure(WireFailure {
        operation,
        code: if retryable {
            "repository_source_unavailable"
        } else {
            "workspace_prepare_outcome_unknown"
        }
        .into(),
        detail: "fixture provider detail".into(),
        retryable,
    }))
}

fn workspace_observations(row: &Workspace) -> &WorkspacePrepareObservations {
    if let Some(recovery) = &row.work.prepare_recovery {
        recovery.attempts[0].observations.as_deref().unwrap()
    } else {
        row.work.prepare_observations.as_deref().unwrap()
    }
}

fn wire_identity(frame: &RequestFrame) -> (&str, &str) {
    match frame {
        RequestFrame::PrepareWorkspace(request) => (&request.operation_id, &request.request_sha256),
        RequestFrame::RecoverWorkspacePrepare(frame) => {
            (&frame.request.operation_id, &frame.request.request_sha256)
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn workspace_prepare_observations_survive_provider_http_and_mongo_worker_handoff() {
    let mut fixture = Fixture::new().await;
    let requests: Requests = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let endpoint = base.clone();
    let app = Router::new()
        .route(
            "/v1/sandboxes/{id}/endpoints/9090",
            get(move || async move { Json(json!({"endpoint": endpoint})) }),
        )
        .route("/runtime/control", post(fail_prepare))
        .with_state(requests.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Arc::get_mut(&mut fixture.probe).unwrap().transport = Some(
        ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
            base,
            "fixture-provider-value",
        ))
        .unwrap(),
    );

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!(
        "workspace_prepare_observations_{}",
        uuid::Uuid::new_v4().simple()
    );
    let store = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_renewable_work(true),
    );
    let mut parent = fixture.harness.stored(&fixture.original.sandbox_id).await;
    parent.revision = 1;
    store.create_sandbox(parent.clone()).await.unwrap();
    let mut recovery = fixture.original.clone();
    recovery.revision = 1;
    store.create_workspace(recovery).await.unwrap();
    let service = WorkspaceService::new(store.clone(), Arc::new(InMemoryProfileStore::default()));
    let owner = caller("owner", [Capability::UseOwnSandboxes]);
    let workspace = service
        .create_inline(
            &owner,
            &parent.id,
            CreateInlineWorkspace {
                source: InlineWorkspaceSource::Empty,
                name: Some("prepare".into()),
            },
        )
        .await
        .unwrap();
    service
        .recover_prepare(&owner, &fixture.original.id, "recovery")
        .await
        .unwrap();

    let successor_client = Client::with_uri_str(&uri).await.unwrap();
    let successor = Arc::new(
        MongoSandboxStore::open(&successor_client, &name)
            .await
            .unwrap()
            .with_renewable_work(true),
    );
    let config = |holder: &str| {
        let mut config = WorkspaceWorkerConfig::new(holder, "test")
            .with_replicas(true)
            .with_prepare_observations(true);
        config.retry_delay = Duration::ZERO;
        config
    };
    let original_worker = fixture.worker_with_store(store.clone(), config("original-worker"));
    let successor_worker = fixture.worker_with_store(successor.clone(), config("successor-worker"));
    for (id, operation, stage) in [
        (
            &workspace.id,
            WireOperation::PrepareWorkspace,
            Stage::PrepareRpc,
        ),
        (
            &fixture.original.id,
            WireOperation::RecoverWorkspacePrepare,
            Stage::RecoveryRpc,
        ),
    ] {
        assert!(original_worker
            .run_one(id, SystemTime::now())
            .await
            .unwrap());
        let pending = successor.get_workspace(id).await.unwrap().unwrap();
        assert_eq!(pending.status, WorkspaceStatus::Preparing);
        assert_eq!(pending.work.attempts, 1);
        let first = workspace_observations(&pending);
        assert_eq!(first.first.observation.stage, stage);
        assert_eq!(
            first.first.observation.code,
            Code::RepositorySourceUnavailable
        );
        assert_eq!(first.first.observation.wire_retryable, Some(true));
        assert!(first.terminal.is_none());

        assert!(successor_worker
            .run_one(id, SystemTime::now())
            .await
            .unwrap());
        let failed = store.get_workspace(id).await.unwrap().unwrap();
        assert_eq!(failed.status, WorkspaceStatus::Failed);
        let failure = failed.failed.as_ref().unwrap();
        assert_eq!(failure.class, "workspace_runtime_rejected");
        let observed = workspace_observations(&failed);
        assert_eq!(observed.first, first.first);
        assert_eq!(observed.operation_id, first.operation_id);
        assert_eq!(observed.request_sha256, first.request_sha256);
        let terminal = observed.terminal.as_ref().unwrap();
        assert_eq!(terminal.observation.stage, stage);
        assert_eq!(
            terminal.observation.code,
            Code::WorkspacePrepareOutcomeUnknown
        );
        assert_eq!(terminal.observation.wire_retryable, Some(false));
        assert_eq!(terminal.attempt, 1);
        assert_eq!(terminal.writer_sha256, first.first.writer_sha256);
        assert!(terminal.writer_sha256.is_some());
        assert!(!serde_json::to_string(observed)
            .unwrap()
            .contains("fixture provider detail"));

        let events = store.sandbox_events(&parent.id, 0, 20).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(&event.kind,
                    SandboxEventKind::WorkspaceFailed { workspace_id, failure: event_failure }
                      if workspace_id == id && event_failure == failure
                ))
                .count(),
            1
        );
        let requests = requests.lock().unwrap();
        let wire: Vec<_> = requests
            .iter()
            .filter(|request| request.operation() == operation)
            .collect();
        assert_eq!(wire.len(), 2);
        assert_eq!(
            serde_json::to_value(wire[0]).unwrap(),
            serde_json::to_value(wire[1]).unwrap()
        );
        let (operation_id, digest) = wire_identity(wire[0]);
        assert_eq!(observed.operation_id, operation_id);
        assert_eq!(observed.request_sha256, digest);
    }
    server.abort();
    client.database(&name).drop().await.unwrap();
}
