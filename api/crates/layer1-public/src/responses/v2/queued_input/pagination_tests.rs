use super::super::controls::fixtures::{call, fixture, request, SOURCE};
use axum::http::{Method, StatusCode};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    resource_access::ResourcePolicyFence,
    sandbox::{
        agent_io::*,
        store::{AgentStore, SandboxMutationGuard, SandboxStore},
    },
    AgentId,
};
use serde_json::Value;
use tower::ServiceExt;
const LIST: &str = "/api/v2/agents/agt_fixture/queued-inputs";
#[tokio::test]
async fn queued_input_http_pages_maximum_text_below_sdk_limit_without_losing_queue_order() {
    let f = fixture(false).await;
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let parent = f
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let source = f
        .store
        .get_command(&AgentCommandId(SOURCE.into()))
        .await
        .unwrap()
        .unwrap();
    let text = "\u{1}".repeat(256 * 1024);
    for index in 1..9 {
        let mut envelope = source.receipt.envelope.clone();
        envelope.command_id = AgentCommandId::generate();
        envelope.command = AgentCommand::message(&text);
        envelope.request_fingerprint = envelope.command.fingerprint().unwrap();
        f.store
            .reserve_command(CommandReservation {
                envelope,
                binding: None,
                key_sha256: command_key_digest(&format!("page-{index}")).unwrap(),
                response_id: Some(AgentResponseId::generate()),
                admitted_at: source.receipt.admitted_at.clone(),
                queue_deadline: source.queue_deadline.clone(),
                queue_capacity: 64,
                expected_agent_revision: agent.revision,
                parent: SandboxMutationGuard::capture(&parent),
                policy: ResourcePolicyFence {
                    sandbox_id: parent.id.clone(),
                    sandbox_created_at: parent.created_at.clone(),
                    policy_revision: 0,
                },
                authority_deadline: None,
                admission_audit: None,
            })
            .await
            .unwrap();
    }
    let mut seen = Vec::new();
    let mut after = None;
    for expected_count in [4, 4, 1] {
        let path = after.map_or_else(|| LIST.to_owned(), |after| format!("{LIST}?after={after}"));
        let response = f
            .router
            .clone()
            .oneshot(request(Method::GET, &path, "owner-a", None, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        assert!(bytes.len() < 8 * 1024 * 1024);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), expected_count);
        seen.extend(
            data.iter()
                .map(|item| item["response"]["receipt"]["agent_order"].as_u64().unwrap()),
        );
        after = body["next_after"].as_u64();
    }
    assert_eq!(seen, (1..=9).collect::<Vec<_>>());
    assert_eq!(after, None);
    for query in [
        "limit=0",
        "limit=5",
        "after=0",
        "after=-1",
        "after=18446744073709551615",
        "limit=4&actor=x",
    ] {
        let (status, _) = call(
            &f.router,
            Method::GET,
            &format!("{LIST}?{query}"),
            "owner-a",
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (status, body) = call(
        &f.router,
        Method::GET,
        &format!("{LIST}?after=9"),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"], serde_json::json!([]));
    assert_eq!(body["next_after"], Value::Null);
}
