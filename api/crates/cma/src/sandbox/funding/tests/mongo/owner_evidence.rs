use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use layer1_public::{rest::sandbox_usage_router, AuthenticatedSubject};
use layer2_core::{
    sandbox::{
        store::{SandboxTransaction, SandboxTransactionStore},
        SandboxCheckpointRef,
    },
    sandbox_usage::UsageTotals,
    sandbox_usage_cursor::{UsageCursorPolicy, UsageCursors},
    sandbox_usage_service::SandboxUsageService,
    Authorization, Capability, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const ACTIVE_SECONDS: u64 = 120;
const WINDOW_SECONDS: u64 = 300;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn owner_events_and_retried_allocated_evidence_agree_across_pause() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let name = format!(
        "cma_funding_owner_evidence_{}",
        uuid::Uuid::new_v4().simple()
    );
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let source = Arc::new(MongoFundingStateStore(store.clone()));
    let mut active = sandbox();
    active.revision = 1;
    active.last_activity_at = active.created_at.clone();
    active.updated_at = active.created_at.clone();
    let start = layer2_core::seconds_from_rfc3339(&active.created_at).unwrap();
    let pause_at = start + ACTIVE_SECONDS;
    let end = start + WINDOW_SECONDS;
    store.create_sandbox(active.clone()).await.unwrap();
    store
        .append_sandbox_event(
            &active.id,
            active.created_at.clone(),
            SandboxEventKind::Created,
        )
        .await
        .unwrap();
    let server = http::Server::with_window_start(
        vec![
            http::funding_reply("$quote"),
            http::reply(404, json!({"error": "not_found"})),
            http::funding_reply("$window"),
            http::reply(503, json!({"error": "unavailable"})),
            http::reply(
                200,
                json!({
                    "evidence_accepted": true, "window_id": http::WINDOW_ID,
                    "owner_id": active.owner, "sandbox_id": active.id, "runtime_generation": 1
                }),
            ),
        ],
        start,
    )
    .await;
    let control = Arc::new(Control::default());
    let adapter =
        ProviderFundingAdapter::new(server.provider.clone(), source.clone(), control.clone());
    assert_eq!(
        adapter.ensure_coverage(&active, at(start)).await.unwrap(),
        Some(end)
    );
    let admitted = source.load(&active.id).await.unwrap().unwrap();
    let window = admitted.current.as_ref().unwrap();
    assert_eq!((window.covered_from, window.covered_until), (start, end));

    let paused = pause(&store, &active, pause_at).await;
    adapter.settle(&paused, at(pause_at)).await.unwrap();
    source.checkpoint(&active.id, at(end)).await.unwrap();
    let (status, owner) = owner_events(store.clone(), &active.id, &active.owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(owner["next"].is_null());
    let rows = owner["items"].as_array().unwrap();
    assert_owner_quantities(rows, &active.id, start, pause_at, end);
    let expected_evidence = durable_evidence(&active, window, rows);
    let (status, foreign) = owner_events(
        store,
        &active.id,
        &SubjectId("foreign-fixture-owner".into()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(foreign.get("items").is_none());
    let denial = foreign.to_string();
    for row in rows {
        assert!(!denial.contains(row["transaction_id"].as_str().unwrap()));
    }

    let observer_client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let observer = MongoFundingStateStore(Arc::new(
        MongoSandboxStore::open(&observer_client, &name)
            .await
            .unwrap(),
    ));
    let mut first_pass = source.load(&active.id).await.unwrap().unwrap();
    let (posted, respond) = server.hold_evidence_response();
    let (attempt, frozen) = tokio::join!(adapter.emit_evidence(&mut first_pass, end), async {
        tokio::time::timeout(Duration::from_secs(10), posted)
            .await
            .unwrap()
            .unwrap();
        let state = observer.load(&active.id).await.unwrap().unwrap();
        assert_eq!(state.evidence_cursor, 0);
        assert_eq!(state.current.as_ref(), Some(window));
        assert_eq!(state.windows.len(), 1);
        assert_eq!(&state.windows[0].window, window);
        assert_eq!(state.windows[0].evidence.as_ref(), Some(&expected_evidence));
        respond.send(()).unwrap();
        state
    },);
    assert_eq!(
        attempt,
        Err(FundingError::Retryable("producer HTTP 503".into()))
    );
    assert_eq!(
        observer.load(&active.id).await.unwrap(),
        Some(frozen.clone())
    );
    assert_eq!(frozen.operation_id, admitted.operation_id);
    assert_eq!(evidence_posts(&server).len(), 1);
    let first_post = evidence_posts(&server).pop().unwrap();
    assert_eq!(
        first_post.0,
        format!(
            "/api/v1/billing/sandboxes/windows/{}/evidence",
            window.window_id
        )
    );
    assert_eq!(
        first_post.1,
        expected_post(&active, rows, window.runtime_generation)
    );

    drop(adapter);
    drop(source);
    drop(observer);
    let reopened_client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let reopened = Arc::new(
        MongoSandboxStore::open(&reopened_client, &name)
            .await
            .unwrap(),
    );
    let recovered_source = Arc::new(MongoFundingStateStore(reopened.clone()));
    let restarted =
        ProviderFundingAdapter::new(server.provider.clone(), recovered_source.clone(), control);
    let mut retry = recovered_source.load(&active.id).await.unwrap().unwrap();
    restarted.emit_evidence(&mut retry, end).await.unwrap();
    assert_eq!(evidence_posts(&server), [first_post.clone(), first_post]);
    let confirmed = recovered_source.load(&active.id).await.unwrap().unwrap();
    assert!(confirmed.windows.is_empty());
    assert_eq!(
        confirmed.evidence_cursor,
        rows.last().unwrap()["revision"].as_u64().unwrap()
    );
    assert_eq!(confirmed.revision, frozen.revision + 1);
    assert_eq!(confirmed.current.as_ref(), Some(window));
    assert_eq!(confirmed.operation_id, admitted.operation_id);
    {
        let calls = server.state.calls.lock().unwrap();
        assert_eq!(calls.len(), 5);
        assert!(calls[1]
            .1
            .ends_with(&format!("/receipts/{}", window.request_id)));
        let reservation = &calls[2].3;
        assert_eq!(reservation["request_id"], window.request_id);
        assert_eq!(reservation["operation_id"], admitted.operation_id);
        assert_eq!(reservation["owner_id"], json!(active.owner));
        assert_eq!(reservation["sandbox_id"], json!(active.id));
        assert_eq!(reservation["runtime_generation"], window.runtime_generation);
    }
    assert_eq!(
        reopened
            .get_sandbox(&active.id)
            .await
            .unwrap()
            .unwrap()
            .lifecycle
            .home,
        paused.lifecycle.home
    );
    assert_eq!(
        owner_events(reopened, &active.id, &active.owner).await,
        (StatusCode::OK, owner)
    );
    restarted.emit_evidence(&mut retry, end).await.unwrap();
    assert_eq!(evidence_posts(&server).len(), 2);
    assert_eq!(
        recovered_source.load(&active.id).await.unwrap(),
        Some(confirmed)
    );
    client.database(&name).drop().await.unwrap();
}

async fn pause(store: &MongoSandboxStore, active: &Sandbox, when: u64) -> Sandbox {
    let event_at = layer2_core::rfc3339_at(at(when));
    let mut paused = active.clone();
    paused.runtime = None;
    paused.status = SandboxStatus::Paused;
    paused.credential.delivered = false;
    paused.credential.current = None;
    paused.lifecycle.checkpoints.latest = Some(
        SandboxCheckpointRef::seal(
            &layer3_adapter::CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoints/funding".into(),
                sha256: "b".repeat(64),
                writers: layer3_adapter::WriterState::Held,
            },
            &[],
            &event_at,
        )
        .unwrap(),
    );
    paused.revision += 1;
    paused.updated_at.clone_from(&event_at);
    store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            expected_revision: active.revision,
            sandbox: Box::new(paused.clone()),
            event_at,
            event: SandboxEventKind::StatusChanged {
                from: SandboxStatus::Active,
                to: SandboxStatus::Paused,
            },
            command_revival: None,
        })
        .await
        .unwrap();
    paused
}

async fn owner_events(
    store: Arc<MongoSandboxStore>,
    id: &SandboxId,
    subject: &SubjectId,
) -> (StatusCode, Value) {
    let service = SandboxUsageService::new(
        store,
        UsageCursors::new(&[7; 32], UsageCursorPolicy::default()),
    );
    let request = Request::builder()
        .uri(format!("/api/v1/usage/sandboxes/{id}/events"))
        .extension(AuthenticatedSubject(subject.clone()))
        .extension(Authorization::new([Capability::UseOwnSandboxes]))
        .body(Body::empty())
        .unwrap();
    let response = sandbox_usage_router(service)
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 32_768).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

fn assert_owner_quantities(rows: &[Value], id: &SandboxId, start: u64, pause_at: u64, end: u64) {
    let active = pause_at - start;
    let paused = end - pause_at;
    let none = json!({"running": false, "cpu_millis": 0, "memory_mib": 0, "disk_gib": 0});
    let compute = json!({"running": true, "cpu_millis": 1000, "memory_mib": 2048, "disk_gib": 64});
    let disk = json!({"running": false, "cpu_millis": 0, "memory_mib": 0, "disk_gib": 64});
    let expected = [
        (start, start, &none, [0, 0, 0, 0]),
        (
            start,
            pause_at,
            &compute,
            [active, 1000 * active, 2048 * active, 64 * active],
        ),
        (pause_at, end, &disk, [0, 0, 0, 64 * paused]),
    ];
    assert_eq!(rows.len(), expected.len());
    for (index, (row, (from, until, allocation, quantities))) in
        rows.iter().zip(expected).enumerate()
    {
        let revision = index + 1;
        assert_eq!(row["revision"], revision);
        assert_eq!(row["transaction_id"], format!("cma-usage:{id}:{revision}"));
        assert_eq!(row["started_at"], from);
        assert_eq!(row["ended_at"], until);
        assert_eq!(&row["allocation"], allocation);
        for (key, quantity) in [
            "uptime_seconds",
            "cpu_millis_seconds",
            "memory_mib_seconds",
            "disk_gib_seconds",
        ]
        .into_iter()
        .zip(quantities)
        {
            assert_eq!(row["quantity"][key], quantity, "revision {revision}: {key}");
        }
    }
    assert_eq!(rows[0]["next_allocation"], compute);
    assert_eq!(rows[1]["next_allocation"], disk);
}

fn durable_evidence(sandbox: &Sandbox, window: &FundingWindow, rows: &[Value]) -> EvidenceRequest {
    let intervals = rows
        .iter()
        .map(|row| EvidenceInterval {
            revision: row["revision"].as_u64().unwrap(),
            started_at: row["started_at"].as_u64().unwrap(),
            ended_at: row["ended_at"].as_u64().unwrap(),
            allocation: serde_json::from_value(row["allocation"].clone()).unwrap(),
            quantity: UsageTotals {
                uptime_seconds: row["quantity"]["uptime_seconds"].as_u64().unwrap(),
                cpu_millis_seconds: row["quantity"]["cpu_millis_seconds"].as_u64().unwrap(),
                memory_mib_seconds: row["quantity"]["memory_mib_seconds"].as_u64().unwrap(),
                disk_gib_seconds: row["quantity"]["disk_gib_seconds"].as_u64().unwrap(),
            },
        })
        .collect();
    EvidenceRequest {
        window_id: window.window_id.clone(),
        owner_id: sandbox.owner.to_string(),
        sandbox_id: sandbox.id.to_string(),
        runtime_generation: window.runtime_generation,
        intervals,
    }
}

fn expected_post(sandbox: &Sandbox, rows: &[Value], generation: u64) -> Value {
    let intervals: Vec<_> = rows
        .iter()
        .map(|row| {
            json!({
                "revision": row["revision"],
                "started_at": row["started_at"],
                "ended_at": row["ended_at"],
                "allocation": {
                    "cpu_millis": row["allocation"]["cpu_millis"],
                    "memory_mib": row["allocation"]["memory_mib"],
                    "disk_gib": row["allocation"]["disk_gib"]
                },
                "quantity": {
                    "cpu_millis_seconds": row["quantity"]["cpu_millis_seconds"],
                    "memory_mib_seconds": row["quantity"]["memory_mib_seconds"],
                    "disk_gib_seconds": row["quantity"]["disk_gib_seconds"]
                }
            })
        })
        .collect();
    json!({
        "owner_id": sandbox.owner, "sandbox_id": sandbox.id, "runtime_generation": generation,
        "intervals": intervals
    })
}

fn evidence_posts(server: &http::Server) -> Vec<(String, Value)> {
    server
        .state
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, path, _, _)| method == Method::POST && path.ends_with("/evidence"))
        .map(|(_, path, _, body)| (path.clone(), body.clone()))
        .collect()
}
