use axum::http::{Method, StatusCode};
use layer2_core::sandbox::store::{
    AgentLogStore, AgentStore, InMemorySandboxStore, SandboxLifecycleAgentScope,
    SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore,
};
use layer2_core::sandbox::{
    AgentStatus, Sandbox, SandboxEventKind, SandboxWorkStep, AGENT_CONTEXT_ADMISSION_BOUND,
};
use serde_json::json;

use crate::rest::agents_tests::{
    conversation_fixture, insert_agent, json_response, request, seeded_agent,
};

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:05:00Z";

#[tokio::test(start_paused = true)]
async fn context_creation_keeps_waiting_until_checkpoint_writers_resume() {
    let (state, store) = conversation_fixture().await;
    let mut parent = store
        .get_sandbox(&layer2_core::SandboxId("sbx_route".into()))
        .await
        .unwrap()
        .unwrap();
    let mut next = parent.clone();
    next.begin_checkpoint_capture(NOW.into()).unwrap();
    advance_capture(&store, &mut parent, next).await;
    let creation = tokio::spawn(async move {
        state
            .context
            .create(
                &layer2_core::Caller {
                    subject: layer2_core::SubjectId("owner-a".into()),
                    authorization: layer2_core::Authorization::new([
                        layer2_core::Capability::UseOwnSandboxes,
                    ]),
                },
                Default::default(),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(!creation.is_finished());
    resume_writers(&store, &mut parent).await;
    let agent = creation.await.unwrap().unwrap();
    assert_eq!(agent.status, AgentStatus::Starting);
    assert_eq!(agent.sandbox_id, parent.id);
    assert_eq!(agent.workspace_id.as_str(), "wks_route");
}

#[tokio::test(start_paused = true)]
async fn checkpoint_input_is_refused_immediately_and_never_admitted_after_resume() {
    for (path, body) in [
        (
            "/api/v1/agui/run",
            json!({"threadId": "agt_checkpoint_input", "messages": [{"role": "user", "content": "Not accepted"}]}),
        ),
        (
            "/api/v1/agui/run",
            json!({"threadId": "agt_checkpoint_input", "resume": [{"interruptId": "approval-one", "status": "resolved", "payload": {"decision": "allow"}}]}),
        ),
        (
            "/api/v1/responses",
            json!({"conversation": "agt_checkpoint_input", "input": "Not accepted", "stream": true}),
        ),
    ] {
        let (state, store) = conversation_fixture().await;
        let agent = seeded_agent("agt_checkpoint_input", AgentStatus::Ready, 2, None);
        insert_agent(&store, agent.clone(), vec![]).await;
        let baseline = store.get_agent(&agent.id).await.unwrap();
        let logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
        let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let mut next = sandbox.clone();
        next.begin_checkpoint_capture(NOW.into()).unwrap();
        advance_capture(&store, &mut sandbox, next).await;
        let router = crate::agui::router(state.clone()).merge(crate::responses::router(state));

        let before = tokio::time::Instant::now();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            json_response(
                &router,
                request(Method::POST, path, "owner-a", None, Some(body)),
            ),
        )
        .await
        .expect("checkpoint input must return before the request could become a waiter");
        assert_eq!(before.elapsed(), std::time::Duration::ZERO);
        assert_eq!(result.0, StatusCode::CONFLICT, "{path}: {:?}", result.1);
        assert_eq!(result.1["code"], "sandbox_checkpoint_in_progress");
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), baseline);
        assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap(), logs);

        resume_writers(&store, &mut sandbox).await;
        tokio::time::advance(AGENT_CONTEXT_ADMISSION_BOUND).await;
        tokio::task::yield_now().await;
        assert!(sandbox.lifecycle.checkpoint_capture.is_none());
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), baseline);
        assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap(), logs);
    }
}

async fn resume_writers(store: &InMemorySandboxStore, current: &mut Sandbox) {
    let mut next = current.clone();
    next.claim_checkpoint_capture("capture".into(), NOW.into(), LATER.into())
        .unwrap();
    advance_capture(store, current, next).await;
    let mut next = current.clone();
    next.require_checkpoint_writer_resume("capture", 1, NOW.into())
        .unwrap();
    advance_capture(store, current, next).await;
    let mut next = current.clone();
    next.claim_checkpoint_capture("resume".into(), NOW.into(), LATER.into())
        .unwrap();
    advance_capture(store, current, next).await;
    let mut next = current.clone();
    next.complete_checkpoint_writer_resume("resume", 2, NOW.into())
        .unwrap();
    advance_capture(store, current, next).await;
}

async fn advance_capture(store: &InMemorySandboxStore, current: &mut Sandbox, next: Sandbox) {
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(current),
            sandbox: next.clone(),
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: NOW.into(),
            events: vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        })
        .await
        .unwrap();
    *current = next;
}
