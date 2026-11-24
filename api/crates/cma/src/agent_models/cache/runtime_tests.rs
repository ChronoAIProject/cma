use super::*;
use crate::agent_models::test_runtime::Probe;
use crate::{projections::UnavailableSandboxResources, store::agent_access_fixtures};
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
    Router,
};
use layer1_public::{rest::agent_models_router, AuthenticatedSubject};
use layer2_core::{
    sandbox::{
        store::{AgentStore, InMemorySandboxStore, SandboxStore, WorkspaceStore, WorktreeStore},
        AgentReadService, AgentStatus, AgentWorkState, AgentWorkStep, RuntimeBinding, SandboxHome,
        WorktreeStatus,
    },
    AgentId, Authorization, Capability, SubjectId,
};
use layer3_adapter::{
    agent_sandbox::{RuntimeCapacityFailure, RuntimeCapacityReason, RuntimeWorkloadClass},
    AdapterError, HomeVolumePolicy, HomeVolumeReceipt, RuntimeCapacityKind,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

fn fixture() -> (Sandbox, RuntimeStatus, Arc<Probe>) {
    let mut root = agent_access_fixtures::sandbox("support-cache");
    let mut home = SandboxHome::reserve(
        &root.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-original".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home".into(),
        storage_generation: "original".into(),
    })
    .unwrap();
    let status: RuntimeStatus = serde_json::from_value(serde_json::json!({
        "sandbox_id": root.id, "generation": 1,
        "home_identity": home.attachment().unwrap().identity().unwrap(),
        "boot_id": "a".repeat(32), "installation_ready": true,
        "credential_ready": true, "restore_pending": false, "agent_processes": 1,
        "agent_model_catalog": true,
    }))
    .unwrap();
    root.runtime = Some(RuntimeBinding {
        runtime_id: "runtime-fixture".into(),
        runtime_version: "image".into(),
        provision_generation: Some(1),
        boot_id: Some(status.boot_id.clone()),
        bound_at: root.created_at.clone(),
    });
    root.lifecycle.runtime_generation_high_water = 1;
    root.lifecycle.home = Some(Box::new(home));
    root.validate().unwrap();
    let probe = Arc::new(Probe::new(status.clone()));
    (root, status, probe)
}

#[tokio::test]
async fn support_cache_coalesces_and_revalidates_exact_runtime_identity() {
    let (root, status, probe) = fixture();
    let support = CachedModelSupport::wrap(probe.clone(), true);
    let values = futures_util::future::join_all((0..200).map(|_| support.supported(&root))).await;
    assert!(values.into_iter().all(|value| value));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    for field in 0..4 {
        let mut changed = root.clone();
        match field {
            0 => changed.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32)),
            1 => {
                changed.runtime.as_mut().unwrap().provision_generation = Some(2);
                changed.lifecycle.runtime_generation_high_water = 2;
            }
            2 => changed.lifecycle.home = None,
            _ => changed.id = layer2_core::SandboxId("other".into()),
        }
        assert!(!support.supported(&changed).await);
    }
    // A valid unsupported observation is distinct from a failed transport.
    let mut next = root.clone();
    next.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    let mut unsupported = status;
    unsupported.boot_id = "b".repeat(32);
    unsupported.agent_model_catalog = false;
    *probe.status.lock().unwrap() = Ok(unsupported);
    assert!(!support.supported(&next).await);
    let before = probe.calls.load(Ordering::SeqCst);
    assert!(!support.supported(&next).await);
    assert_eq!(probe.calls.load(Ordering::SeqCst), before);
}

#[tokio::test(start_paused = true)]
async fn support_failure_timeout_unknown_boot_and_disabled_cache_do_not_reuse_success() {
    let (root, status, probe) = fixture();
    let support = CachedModelSupport::wrap(probe.clone(), true);
    *probe.status.lock().unwrap() = Err(AdapterError::Unavailable("fixture unavailable".into()));
    assert!(!support.supported(&root).await);
    *probe.status.lock().unwrap() = Ok(status);
    probe.delay_ms.store(6000, Ordering::SeqCst);
    assert!(!support.supported(&root).await);
    probe.delay_ms.store(0, Ordering::SeqCst);
    assert!(support.supported(&root).await);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 3);
    let mut unbound = root.clone();
    unbound.runtime.as_mut().unwrap().boot_id = None;
    assert!(support.supported(&unbound).await);
    assert!(support.supported(&unbound).await);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 5);
    let disabled = CachedModelSupport::wrap(probe.clone(), false);
    assert!(disabled.supported(&root).await);
    assert!(disabled.supported(&root).await);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 7);
}

#[tokio::test(start_paused = true)]
async fn concurrent_stalled_support_reads_do_not_serialize_timeouts() {
    let (root, _, probe) = fixture();
    probe.delay_ms.store(6000, Ordering::SeqCst);
    let support = CachedModelSupport::wrap(probe, true);
    let started = tokio::time::Instant::now();
    let values = futures_util::future::join_all((0..200).map(|_| support.supported(&root))).await;
    assert!(values.into_iter().all(|value| !value));
    assert_eq!(started.elapsed(), Duration::from_secs(5));
}

#[derive(Default)]
struct Catalog(AtomicUsize);

#[async_trait::async_trait]
impl AgentModelCatalog for Catalog {
    async fn models(&self, _: &Sandbox) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![AgentModelView {
            id: "fixture-model".into(),
            label: "Fixture Model".into(),
            available: false,
        }])
    }
}

async fn model_reads(root: Sandbox) -> (Arc<AgentReadService>, AgentId) {
    let workspace = agent_access_fixtures::workspace(&root, "support-cache");
    let creation = agent_access_fixtures::admission(&workspace, "support-cache")
        .materialize(&workspace)
        .unwrap();
    let mut worktree = creation.worktree;
    worktree.status = WorktreeStatus::Ready;
    let mut agent = creation.agent;
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(agent.created_at.clone());
    agent.work = AgentWorkState::new(AgentWorkStep::None, agent.created_at.clone());
    agent.conversation.conversation_id = Some("fixture-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    let agent_id = agent.id.clone();
    let store = Arc::new(InMemorySandboxStore::default());
    store.create_sandbox(root).await.unwrap();
    store.create_workspace(workspace).await.unwrap();
    store.create_worktree(worktree).await.unwrap();
    store.create_agent(agent).await.unwrap();
    let reads = AgentReadService::new(store, Arc::new(UnavailableSandboxResources));
    (reads, agent_id)
}

async fn list_models(
    router: &Router,
    owner: &SubjectId,
    agent: &AgentId,
) -> (StatusCode, HeaderMap, Value) {
    let request = Request::builder()
        .uri(format!("/api/v1/agents/{agent}/models"))
        .extension(AuthenticatedSubject(owner.clone()))
        .extension(Authorization::new([Capability::UseOwnSandboxes]))
        .body(Body::empty())
        .unwrap();
    let (parts, body) = router.clone().oneshot(request).await.unwrap().into_parts();
    let bytes = to_bytes(body, 4096).await.unwrap();
    (
        parts.status,
        parts.headers,
        serde_json::from_slice(&bytes).unwrap(),
    )
}

#[tokio::test(start_paused = true)]
async fn model_list_preserves_runtime_capacity_and_retries_the_same_binding() {
    for (cache_enabled, boot_known) in [(false, true), (true, true), (true, false)] {
        let (mut root, status, probe) = fixture();
        if !boot_known {
            root.runtime.as_mut().unwrap().boot_id = None;
        }
        let owner = root.owner.clone();
        let (reads, agent) = model_reads(root).await;
        let catalog = Arc::new(Catalog::default());
        let router = agent_models_router(
            reads,
            catalog.clone(),
            CachedModelSupport::wrap(probe.clone(), cache_enabled),
        );
        *probe.status.lock().unwrap() = Err(RuntimeCapacityFailure {
            kind: RuntimeCapacityKind::Request,
            class: RuntimeWorkloadClass::Ordinary,
            reason: RuntimeCapacityReason::Exhausted,
        }
        .into());
        let (http_status, headers, body) = list_models(&router, &owner, &agent).await;
        assert_eq!(catalog.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            http_status,
            StatusCode::TOO_MANY_REQUESTS,
            "cache enabled: {cache_enabled}, boot known: {boot_known}, body: {body}"
        );
        assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
        assert_eq!(headers[header::RETRY_AFTER], "1");
        assert_eq!(body["code"], "runtime_request_capacity_exhausted");

        *probe.status.lock().unwrap() = Ok(status);
        let (http_status, _, body) = list_models(&router, &owner, &agent).await;
        assert_eq!(http_status, StatusCode::OK);
        assert_eq!(
            body,
            json!({"models":[{"id":"fixture-model","label":"Fixture Model","available":true}]})
        );
        assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    }
}
