use super::*;
use layer2_core::resource_access::*;
use layer2_core::sandbox::agent_io::*;
use layer2_core::Caller;

#[tokio::test]
async fn response_admission_reads_durable_open_before_ready_publication_without_retry() {
    use tower::ServiceExt;
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("response-before-ready", vec![]).await;
    for _ in 0..3 {
        harness.run(&agent.id, harness.now).await;
    }
    let pending = harness.stored(&agent.id).await;
    assert_eq!(pending.status, AgentStatus::Starting);
    assert_eq!(pending.work.step, AgentWorkStep::PublishReady);
    assert!(pending.conversation.conversation_id.is_some());
    assert!(!harness
        .store
        .agent_log(&agent.id, 0, 100)
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry.kind, AgentLogKind::ConversationOpened { .. })));
    let (router, _) = conversation_router(&harness).await;
    let router = router.layer(middleware::from_fn_with_state(
        harness.workspace.owner.clone(),
        authenticate,
    ));
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v2/responses")
                .header("authorization", TEST_BEARER)
                .header("content-type", "application/json")
                .header("idempotency-key", "open-before-ready")
                .body(axum::body::Body::from(
                    json!({"conversation": agent.id, "input": "sdk-helper-text", "stream": true})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.stored(&agent.id).await, pending);
    assert_eq!(
        harness
            .store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    drop(response);
}

struct Authority(SubjectId);
pub(super) fn context(owner: &SubjectId) -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        actor: ResourceActor::Api {
            subject: owner.clone(),
            delegation_id: None,
        },
        credential: CredentialContext {
            issuer: "http://loopback.fixture".into(),
            subject: owner.clone(),
            credential_id: "official-sdk-fixture".into(),
        },
        memberships: vec![],
    }
}

pub(super) fn human_context(owner: &SubjectId) -> CallerContext {
    let mut caller = context(owner);
    caller.actor = ResourceActor::Human {
        subject: owner.clone(),
    };
    caller
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::NotMember)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}
#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        caller: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if ![context(&self.0), human_context(&self.0)].contains(caller)
            || credential != "local-sdk-conformance"
            || owner
                != &(AgentOwner::Person {
                    subject: self.0.clone(),
                })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: "official-sdk-fixture".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}
#[async_trait::async_trait]
impl AgentCommandAuthorizer for Authority {
    async fn authorize_command(
        &self,
        command: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError> {
        if command.authorization.authority_id != "official-sdk-fixture"
            || command.owner
                != (AgentOwner::Person {
                    subject: self.0.clone(),
                })
            || ![context(&self.0).actor, human_context(&self.0).actor].contains(&command.actor)
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(context(&self.0).caller)
    }
}
pub(super) fn compose<S: FixtureStore>(
    harness: &Harness<S>,
    services: AgentProjectionServices,
    context: Arc<AgentContextService>,
    worker_holder: &str,
) -> (Router, Arc<AgentCommandWorker>) {
    let authority = Arc::new(Authority(harness.workspace.owner.clone()));
    compose_with_authority(harness, services, context, worker_holder, authority, None)
}
pub(super) fn compose_with_authority<S: FixtureStore, A>(
    harness: &Harness<S>,
    services: AgentProjectionServices,
    context: Arc<AgentContextService>,
    worker_holder: &str,
    authority: Arc<A>,
    bindings: Option<Arc<layer2_core::sandbox::binding::BindingAccessService>>,
) -> (Router, Arc<AgentCommandWorker>)
where
    A: OrganizationAuthority + AgentCommandAdmissionAuthority + AgentCommandAuthorizer + 'static,
{
    let mut access = ResourceAccessService::new(harness.store.clone(), authority.clone());
    if let Some(bindings) = &bindings {
        access = access.with_bindings(bindings.clone());
    }
    let access = Arc::new(access);
    let services = services.with_resource_access(access.clone());
    let commands =
        AgentCommandService::new(harness.store.clone(), access.clone(), authority.clone());
    let services = services.with_commands(commands.clone());
    let worker = AgentCommandWorker::new(
        harness.store.clone(),
        Arc::new(crate::commands::ResourceCommandExecution {
            authority,
            access,
            store: harness.store.clone(),
            operations: services.operations(),
            context: context.clone(),
        }),
        worker_holder.into(),
    )
    .unwrap();
    let mut responses =
        layer1_public::responses::v2::ResponsesV2State::new(commands, services.reads());
    if let Some(bindings) = bindings {
        responses = responses.with_bindings(bindings);
    }
    let router = services
        .conversation_router(context, harness.updates.clone(), "http://127.0.0.1")
        .merge(services.agent_events_router(Default::default(), harness.updates.clone()))
        .merge(layer1_public::responses::v2::router(responses));
    (router, worker)
}

#[tokio::test]
async fn versioned_responses_fixture_executes_two_conversation_only_turns_through_durable_worker() {
    let server = start_server(0).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let conversation: Value = client
        .post(format!("{}/conversations", server.base_url))
        .header("authorization", TEST_BEARER)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = conversation["id"].as_str().unwrap();
    let base = server.base_url.replace("/api/v1", "/api/v2");
    let mut previous = None;
    for turn in 0..2 {
        let path = if turn == 0 {
            format!("{base}/responses")
        } else {
            format!("{base}/agents/{id}/responses")
        };
        let response = client
            .post(path)
            .header("authorization", TEST_BEARER)
            .header("idempotency-key", format!("turn-{turn}"))
            .json(&json!({"conversation":id,"input":"sdk-helper-text","stream":true}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let wire = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{wire}");
        let events: Vec<Value> = wire
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events[0]["type"], "response.created", "{wire}");
        assert_eq!(
            events.last().unwrap()["type"],
            "response.completed",
            "{wire}"
        );
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event["sequence_number"], index);
        }
        let final_response = &events.last().unwrap()["response"];
        assert_eq!(final_response["conversation"]["id"], id);
        assert_eq!(final_response["previous_response_id"], Value::Null);
        assert_eq!(
            final_response["output"][0]["content"][0]["text"],
            "Hello from CMA"
        );
        let response_id = final_response["id"].as_str().unwrap();
        assert_ne!(Some(response_id), previous.as_deref());
        previous = Some(response_id.to_owned());
        let retrieved: Value = client
            .get(format!("{base}/responses/{response_id}"))
            .header("authorization", TEST_BEARER)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(&retrieved, final_response);
    }
    client
        .post(server.shutdown_url())
        .header("authorization", TEST_BEARER)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    server.task.await.unwrap();
}
