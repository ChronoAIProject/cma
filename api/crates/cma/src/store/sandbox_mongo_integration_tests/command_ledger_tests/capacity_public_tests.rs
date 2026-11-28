use super::*;
#[path = "capacity_public_tests/capture_advance.rs"]
mod capture_advance;
#[path = "capacity_public_tests/retained_output.rs"]
mod retained_output;
use crate::store::sandbox_mongo_integration_tests::capacity_fixtures::{
    fill_normal_capacity, fingerprint,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use layer1_public::{AuthenticatedSubject, DelegatedCredential};
use layer3_adapter::{
    ConversationUpdate, PromptIdentity, PromptResponse, RuntimeOutputReceipt, RuntimeOutputSource,
    SessionUpdate, ToolCallUpdate, ToolCallUpdateFields, TurnStopReason,
};
use tower::ServiceExt;

#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Ok(AgentAuthorizationRevision {
            replay_principal: None,
            authority_id: "personal-owner".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

struct NoResources;
#[async_trait::async_trait]
impl SandboxResourceReader for NoResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("Response read must not inspect or execute a runtime")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn count_capacity_settlement_projects_exact_command_and_response_without_resend() {
    for refused in [false, true] {
        public_case(false, refused).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn byte_capacity_settlement_projects_exact_command_and_response_without_resend() {
    for refused in [false, true] {
        public_case(true, refused).await;
    }
}

async fn public_case(bytes: bool, refused: bool) {
    let (client, name) = database("capacity_public_terminal").await;
    let (store, agent, parent, execution, authority, sends) = composed_with_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        active_sandbox("sbx_commands"),
        Some((100, true)),
    )
    .await;
    let command = record(
        store
            .reserve_command(live_request(&agent, &parent, "capacity-public"))
            .await
            .unwrap(),
    );
    let id = &command.receipt.envelope.command_id;
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "capacity-public-worker".into()).unwrap();
    worker.run_one(id).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &agent.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(
                    "Preserved answer",
                )),
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    recorder
        .record_update(
            &agent.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                "recorded-tool",
                ToolCallUpdateFields::new().title("Recorded calculation"),
            ))),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let before = store.get_agent(&agent.id).await.unwrap().unwrap();
    let full = fill_normal_capacity(&store, before, bytes).await;
    let history = fingerprint(&store, &agent.id, full.conversation.last_seq).await;
    // Prepare this exact complete history so capacity cannot evade the modern
    // trusted-metadata writer/read paths by using only a legacy fixture.
    let prepared = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    let prepared = Arc::new(prepared);
    let recorder = DurableAgentUpdateRecorder::new(prepared.clone());
    recorder
        .record_update(
            &agent.id,
            1,
            &mut assembly,
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(
                    "Visible partial reply before settlement",
                )),
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let terminal = ConversationUpdate::TurnEndedForPrompt {
        prompt_id: PromptIdentity::new(
            full.conversation.conversation_id.as_deref().unwrap(),
            1,
            full.conversation.active_turn.unwrap(),
        ),
        response: PromptResponse::new(TurnStopReason::EndTurn),
    };
    let update = if refused {
        ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "missing-output",
            ToolCallUpdateFields::new().title("Unrecorded output"),
        )))
    } else {
        terminal
    };
    let source = RuntimeOutputSource {
        stream_id: "83111111-1111-4111-8111-111111111111".into(),
        cursor: 1,
        native_conversation_id: full.conversation.conversation_id.clone().unwrap(),
        payload_sha256: "c".repeat(64),
    };
    let (acks, _receiver) = tokio::sync::mpsc::channel(1);
    let started = std::time::Instant::now();
    recorder
        .record_update(
            &agent.id,
            1,
            &mut assembly,
            ConversationUpdate::RuntimeOutput {
                receipt: RuntimeOutputReceipt::new(source, acks),
                update: Some(Box::new(update).into()),
            },
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    for _ in 0..2 {
        worker.run_one(id).await.unwrap();
    }
    let settled = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(
        settled.status,
        if refused {
            AgentResponseStatus::Failed
        } else {
            AgentResponseStatus::Completed
        }
    );
    assert_eq!(settled.receipt, command.receipt);
    assert!(settled.work.lease.is_none());
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(saved.status, AgentStatus::Failed);
    assert_eq!(
        saved.failure.as_ref().unwrap().class,
        "agent_log_capacity_exceeded"
    );
    assert!(saved.work.lease.is_none());
    assert_eq!(saved.work.step, AgentWorkStep::None);
    assert_eq!(
        saved.conversation.observations.usage,
        full.conversation.observations.usage
    );
    let response = response(
        prepared,
        &agent,
        authority,
        command.receipt.response_id.as_ref().unwrap(),
    )
    .await;
    assert_eq!(
        response["status"],
        if refused { "failed" } else { "completed" }
    );
    if refused {
        assert_eq!(response["error"]["code"], "agent_log_capacity_exceeded");
    } else {
        assert!(response["error"].is_null());
    }
    assert!(response.to_string().contains("Preserved answer"));
    assert!(response
        .to_string()
        .contains("Visible partial reply before settlement"));
    assert!(!response.to_string().contains("Unrecorded output"));
    assert!(started.elapsed() < Duration::from_secs(120));
    assert_eq!(
        fingerprint(&store, &agent.id, full.conversation.last_seq).await,
        history
    );
    let tail = store
        .agent_log(&agent.id, full.conversation.last_seq, 200)
        .await
        .unwrap();
    assert!(
        matches!(&tail[0].kind, AgentLogKind::AgentMessage {content} if content == "Visible partial reply before settlement")
    );
    assert_eq!(tail.iter().filter(|e| matches!(&e.kind, AgentLogKind::AgentMessage {content} if content == "Visible partial reply before settlement")).count(), 1);
    assert!(!store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap()
        .contains_key("agent_capacity_prefix_v1"));
    assert_eq!(
        tail.iter()
            .filter(|entry| matches!(
                entry.kind,
                AgentLogKind::TurnCompleted { .. } | AgentLogKind::TurnFailed { .. }
            ))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

pub(super) async fn response(
    store: Arc<MongoSandboxStore>,
    agent: &Agent,
    authority: Arc<Authority>,
    response_id: &AgentResponseId,
) -> serde_json::Value {
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    let reads = AgentReadService::new(store, Arc::new(NoResources)).with_resource_access(access);
    let router =
        layer1_public::responses::v2::router(layer1_public::responses::v2::ResponsesV2State::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
            commands,
            reads,
        ));
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let context = CallerContext {
        credential_source: CredentialSource::Retained,
        caller: caller.clone(),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: agent.owner.clone(),
            credential_id: "capacity-fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: agent.owner.clone(),
        },
        memberships: vec![],
    };
    let mut request = Request::builder()
        .uri(format!("/api/v2/responses/{response_id}"))
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(agent.owner.clone()));
    request.extensions_mut().insert(caller.authorization);
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(DelegatedCredential::new("capacity-fixture".into()));
    let result = router.oneshot(request).await.unwrap();
    let status = result.status();
    let body = to_bytes(result.into_body(), 2 * 1024 * 1024).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}
