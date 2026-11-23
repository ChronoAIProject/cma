//! Count actual driver commands across the composed public conversation route.
use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use futures_util::StreamExt;
use tower::ServiceExt;

struct Defaults;
#[async_trait::async_trait]
impl AgentContextDefaults for Defaults {
    async fn defaults(&self) -> Result<layer2_core::PlatformDefaults, AgentContextError> {
        panic!("replay cannot create resources")
    }
}

fn app(f: &Fixture) -> (Router, Arc<DurableAgentUpdateRecorder>) {
    let updates = DurableAgentUpdateRecorder::new(Arc::new(f.store.clone()));
    (app_with_reads(f, f.reads(false), updates.clone()), updates)
}

pub(super) fn app_with_reads(
    f: &Fixture,
    reads: Arc<AgentReadService>,
    updates: Arc<DurableAgentUpdateRecorder>,
) -> Router {
    let store = Arc::new(f.store.clone());
    let profiles = Arc::new(layer2_core::profiles::InMemoryProfileStore::default());
    let runtimes = Arc::new(crate::sandbox::LiveAgentRegistry::default());
    let agents = AgentService::new(
        store.clone(),
        profiles.clone(),
        runtimes.clone(),
        layer2_core::RuntimeCapabilities::new([layer2_core::RuntimeCapability::Conversation]),
    );
    let context = AgentContextService::new(
        SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(store.clone(), profiles.clone()),
        agents,
        profiles,
        Arc::new(Defaults),
    );
    let state = layer1_public::AgentConversationState::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        reads,
        AgentOperationService::new(store, runtimes),
        context,
    )
    .with_bounded_replay(true)
    .with_updates(updates.clone());
    layer1_public::agui::router(state)
}

async fn open(
    f: &Fixture,
    app: &Router,
    after: u64,
    follow: bool,
    authorized: bool,
) -> axum::response::Response {
    let context = fixture::context(f.agent.owner.clone());
    let mut request = Request::get(format!(
        "/api/v1/agui/events?thread_id={}&starting_after={after}&follow={follow}",
        f.agent.id
    ))
    .body(Body::empty())
    .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(
            context.caller.subject.clone(),
        ));
    request
        .extensions_mut()
        .insert(context.caller.authorization.clone());
    if authorized {
        request.extensions_mut().insert(context);
    }
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response
}

fn commands(f: &Fixture) -> u64 {
    let actual = f.meter.stats.lock().unwrap();
    let commands = actual.commands.values().sum();
    assert_eq!(commands, actual.checkouts);
    commands
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn composed_replay_page_and_live_burst_have_bounded_mongo_commands() {
    for authorized in [false, true] {
        let mut f = Fixture::semantic_messages(200).await;
        f.store = f
            .store
            .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
            .await
            .unwrap();
        let (app, updates) = app(&f);
        f.meter.clear();
        let response = open(&f, &app, 0, false, authorized).await;
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let wire = String::from_utf8(bytes.to_vec()).unwrap();
        let page_commands = commands(&f);
        assert_eq!(wire.matches("TEXT_MESSAGE_CONTENT").count(), 196);
        let ids: Vec<u64> = wire
            .lines()
            .filter_map(|line| line.strip_prefix("id: "))
            .map(|id| id.parse().unwrap())
            .collect();
        // OperationDeliveryChanged at five is internal dispatch evidence and
        // has no public projection. Every visible original cursor is retained.
        assert_eq!(ids, (2..=200).filter(|seq| *seq != 5).collect::<Vec<_>>());
        assert!(page_commands <= 25, "page commands: {page_commands}");

        f.meter.clear();
        let mut body = open(&f, &app, 200, true, authorized)
            .await
            .into_body()
            .into_data_stream();
        let open_commands = commands(&f);
        // Producer writes/reads are outside the observation budget. Both the
        // stream opening and every read made while releasing it are counted.
        for _ in 0..100 {
            updates.record_update(&f.agent.id, 1, &mut AgentMessageAssembly::default(), AgentRuntimeUpdate::Said(
                serde_json::from_value(serde_json::json!({"sessionUpdate":"agent_message_chunk","messageId":"burst","content":{"type":"text","text":"burst fragment"}})).unwrap()
            ), NOW.into()).await.unwrap();
        }
        f.meter.clear();
        for _ in 0..100 {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(2), body.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let wire = String::from_utf8_lossy(&frame);
            assert!(wire.contains("burst fragment"), "{wire}");
            assert!(!wire.contains("id:"));
        }
        let release_commands = commands(&f);
        assert!(release_commands <= 6, "burst release: {release_commands}");
        assert!(
            open_commands + release_commands <= 40,
            "burst full route: {}",
            open_commands + release_commands
        );
        eprintln!("AGUI_MONGO_BUDGET authorized={authorized} page_entries=200 page_frames={} page_commands={page_commands} live_fragments=100 open_commands={open_commands} release_commands={release_commands} live_route_commands={}", wire.matches("data: ").count(), open_commands + release_commands);
        drop(body);
        drop(app);
        drop(updates);
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner with failpoints"]
async fn timed_out_owner_release_aborts_without_inventing_a_run_failure() {
    let mut f = Fixture::semantic_messages(200).await;
    f.store = f
        .store
        .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
        .await
        .unwrap();
    let (app, updates) = app(&f);
    let mut body = open(&f, &app, 0, false, true)
        .await
        .into_body()
        .into_data_stream();
    assert!(body.next().await.unwrap().is_ok());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    f.client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1},
            "data": {"failCommands": ["aggregate"], "errorCode": 50}
        })
        .await
        .unwrap();
    assert!(
        body.next().await.unwrap().is_err(),
        "read timeout must abort finite replay, never emit a run outcome"
    );
    assert!(body.next().await.is_none());
    f.client
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    drop(body);
    drop(app);
    drop(updates);
    f.close().await;
}
