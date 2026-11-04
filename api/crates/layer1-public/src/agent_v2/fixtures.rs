use super::*;
use async_trait::async_trait;
use layer2_core::{
    resource_access::*,
    sandbox::{agent_io::*, store::*, *},
    AgentId,
};

pub(crate) use crate::responses::v2::controls::fixtures::{context, request};
pub(crate) const SNAPSHOT: &str = "/api/v2/agents/agt_fixture/events/snapshot";
pub(crate) const EVENTS: &str = "/api/v2/agents/agt_fixture/events";
pub(crate) fn task_policy() -> Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy> {
    Arc::new(serde_json::from_value(serde_json::json!({"mode":"all","accounts":[],"capabilities":["task_home_admission","suggested_followup_admission","desktop_enable","browser_takeover","replay_presentation"]})).unwrap())
}
pub(super) struct NoAdmission;
#[async_trait]
impl AgentCommandAdmissionAuthority for NoAdmission {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        _: &str,
        _: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Err(CommandAuthorizationError::Revoked)
    }
}
struct NoResources;
#[async_trait]
impl SandboxResourceReader for NoResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        unreachable!("event reads do not inspect runtime resources")
    }
}

pub(crate) struct Fixture {
    pub router: Router,
    pub state: AgentV2State,
    pub store: Arc<InMemorySandboxStore>,
    pub access: Arc<ResourceAccessService>,
}
pub(crate) async fn fixture(bound: bool, flag: bool) -> Fixture {
    let original = crate::responses::v2::controls::fixtures::fixture(bound).await;
    let reads = AgentReadService::new(original.store.clone(), Arc::new(NoResources))
        .with_resource_access(original.access.clone());
    let commands = AgentCommandService::new(
        original.store.clone(),
        original.access.clone(),
        Arc::new(NoAdmission),
    );
    let state = AgentV2State::new(reads, commands)
        .with_updates(DurableAgentUpdateRecorder::new(original.store.clone()))
        .with_ui_transport(
            flag,
            UiTransportDependencies {
                responses: true,
                controls: true,
            },
        );
    Fixture {
        router: router(state.clone()).merge(original.router),
        state,
        store: original.store,
        access: original.access,
    }
}

pub(crate) async fn append(store: &InMemorySandboxStore, kind: AgentLogKind) {
    let id = AgentId("agt_fixture".into());
    let mut agent = store.get_agent(&id).await.unwrap().unwrap();
    let revision = agent.revision;
    store
        .append_agent_log(&id, "2026-09-16T00:00:00Z".into(), kind)
        .await
        .unwrap();
    agent.conversation.last_seq += 1;
    agent.revision += 1;
    store.save_agent(revision, agent).await.unwrap();
}

pub(crate) async fn call(
    router: &Router,
    method: axum::http::Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<serde_json::Value>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let response = router
        .clone()
        .oneshot(request(method, path, subject, key, body))
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

pub(crate) fn unavailable(f: &Fixture) -> Router {
    let mut state = f.state.clone();
    state.reads = AgentReadService::new(f.store.clone(), Arc::new(NoResources));
    router(state)
}

/// The real router and recorder produce every frame. Only fixture EOF is
/// bounded after four frames so the shared replay harness can inspect live IO.
pub(crate) fn with_live_frames(f: &Fixture, lagged: bool) -> Router {
    use axum::{
        body::Body,
        extract::Request,
        middleware::{from_fn, Next},
        response::Response,
    };
    use futures_util::StreamExt;
    let updates = f.state.updates.as_ref().unwrap().clone();
    f.router.clone().layer(from_fn(move |request: Request, next: Next| {
        let updates = updates.clone();
        async move {
            let response = next.run(request).await;
            if !response.status().is_success() { return response; }
            let mut assembly = AgentMessageAssembly::default();
            for index in 0..if lagged { 40 } else { 2 } {
                let thought = !lagged && index == 1;
                let update = serde_json::json!({
                    "sessionUpdate": if thought { "agent_thought_chunk" } else { "agent_message_chunk" },
                    "messageId": if thought { "contract-thought" } else { "contract-live-message" },
                    "content": {"type":"text", "text": if thought { "Reviewing the file" } else { "Live answer" }}
                });
                updates.record_update(
                    &AgentId("agt_fixture".into()), 1, &mut assembly,
                    AgentRuntimeUpdate::Said(serde_json::from_value(update).unwrap()),
                    "2026-09-16T00:00:00Z".into(),
                ).await.unwrap();
            }
            let (parts, body) = response.into_parts();
            Response::from_parts(parts, Body::from_stream(body.into_data_stream().take(4)))
        }
    }))
}
