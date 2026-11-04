use super::*;
use crate::rest::agents_tests;
use layer2_core::resource_access::*;
use layer2_core::sandbox::agent_artifacts::{AgentArtifactService, PublishAgentArtifact};
use layer2_core::sandbox::answer_deliverables::AnswerDeliverableService;
use layer2_core::{Authorization, Caller, Capability};

struct Organizations;
#[async_trait::async_trait]
impl OrganizationAuthority for Organizations {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal artifacts require no organization lookup")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}

struct DeliveryAuth {
    stub: Arc<Stub>,
    requester: AgentAccessRequester,
}
#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for DeliveryAuth {
    async fn authenticate_setup(&self, capability: &str) -> Result<(), AgentAccessError> {
        self.stub.authenticate_setup(capability).await
    }
    async fn authenticate(
        &self,
        capability: &str,
    ) -> Result<AgentAccessRequester, AgentAccessError> {
        self.stub.authenticate(capability).await?;
        Ok(self.requester.clone())
    }
}

#[tokio::test]
async fn confirmed_runtime_describes_only_its_authorized_deliverable_and_loses_access_after_revocation(
) {
    let fixture = agents_tests::fixture().await;
    let agent = agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent(
            "agt_delivery",
            layer2_core::sandbox::AgentStatus::Ready,
            2,
            None,
        ),
        vec![],
    )
    .await;
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let lineage = fixture.reads().lineage(&caller, &agent.id).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        Arc::new(Organizations),
    ));
    let artifacts = AgentArtifactService::new(fixture.store.clone(), access);
    let published = artifacts
        .publish(
            &caller,
            None,
            None,
            &agent.id,
            "result",
            PublishAgentArtifact {
                name: "result.txt".into(),
                media_type: "text/plain".into(),
                content_base64: "YWJj".into(),
            },
        )
        .await
        .unwrap();
    let service = AnswerDeliverableService::new(fixture.reads(), artifacts);
    let stub = Stub::new(true);
    let auth = Arc::new(DeliveryAuth {
        stub: stub.clone(),
        requester: AgentAccessRequester {
            agent_id: agent.id.clone(),
            owner: agent.owner.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            conversation: agent.conversation.conversation_id.clone().unwrap(),
            runtime: lineage.sandbox.runtime.clone().unwrap(),
            creation: AgentCreationProvenance {
                schema_version: 1,
                agent_id: agent.id.clone(),
                agent_created_at: agent.created_at.clone(),
                sandbox_id: agent.sandbox_id.clone(),
                sandbox_created_at: lineage.sandbox.created_at,
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
                owner: ResourceOwner::Person {
                    subject: agent.owner.clone(),
                },
                creation_origin: AgentCreationOrigin::Manual,
                created_by: None,
                provisioning_origin: agent.origin,
                creation_fingerprint: agent.creation_fingerprint,
            },
        },
    });
    let app = agent_delivery_tools_router(
        auth,
        stub.clone(),
        None,
        Some(service),
        "https://cma.example",
    );
    let command = rpc(
        "tools/call",
        json!({"name":"describe_deliverable","arguments":{"kind":"agent_artifact","id":published.id}}),
    );
    let response = body(app.clone().oneshot(request(&stub, &command)).await.unwrap()).await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["structuredContent"]["reference"]["sha256"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        response["result"]["structuredContent"]["reference"]["agent_id"],
        "agt_delivery"
    );
    assert!(response["result"]["structuredContent"]["marker"]
        .as_str()
        .unwrap()
        .starts_with("<!--cma-deliverable:v1:"));
    let invalid = rpc(
        "tools/call",
        json!({"name":"describe_deliverable","arguments":{"kind":"document","id":format!("doc_{}", "a".repeat(32)),"revision":1}}),
    );
    let denied = body(app.clone().oneshot(request(&stub, &invalid)).await.unwrap()).await;
    assert_eq!(denied["result"]["isError"], true);
    stub.confirmed.store(false, Ordering::SeqCst);
    assert_eq!(
        app.oneshot(request(&stub, &command))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(stub.proposals.lock().unwrap().is_empty());
}
