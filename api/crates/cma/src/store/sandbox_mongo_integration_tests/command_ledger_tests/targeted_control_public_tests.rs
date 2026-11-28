use super::*;
use layer2_core::resource_access::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};
use layer2_core::{Authorization, Caller, Capability};
use layer3_adapter::{ContentBlock, ContentChunk, ConversationUpdate, SessionUpdate};
use std::sync::Arc;

struct Authority;

#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal control cannot enumerate organizations")
    }

    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal control cannot request organization membership")
    }
}

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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn public_control_admits_and_recovers_after_execution_observed() {
    let (client, name) = database("targeted_public_execution_observed").await;
    let (store, agent, _, source) = composed(&name, &client).await;
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let workspace = ready_workspace(&parent, agent.workspace_id.as_str(), "project");
    let (mut worktree, _) = worktree_and_agent(&parent.id);
    worktree.id = agent.worktree_id.clone();
    worktree.workspace_id = workspace.id.clone();
    worktree.agent_id = Some(agent.id.clone());
    store.create_workspace(workspace).await.unwrap();
    store.create_worktree(worktree).await.unwrap();
    let store = Arc::new(store);
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::from("working"),
            ))),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    store
        .reconcile_command(&source.receipt.envelope.command_id)
        .await
        .unwrap();
    let context = CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            agent.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: agent.owner.clone(),
            credential_id: "fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: agent.owner.clone(),
        },
        memberships: vec![],
    };
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(Authority),
    ));
    let service = AgentCommandService::new(store.clone(), access, Arc::new(Authority));
    let observed = service
        .observe_response_controls(
            &context,
            Some("fixture"),
            &agent.id,
            source.receipt.response_id.as_ref().unwrap(),
        )
        .await
        .unwrap();
    assert!(observed.target_active && observed.runtime_support.cancel);
    assert_eq!(
        observed.response.delivery,
        Some(layer2_core::sandbox::AgentOperationDelivery::ExecutionObserved)
    );
    let submission = TargetedControlSubmission {
        agent_id: agent.id.clone(),
        target: observed.target.unwrap(),
        expected_agent_revision: observed.agent_revision,
        command: AgentCommand::Interrupt,
        idempotency_key: "exact-retained-public-control".into(),
    };
    let accepted = service
        .admit_response_control(
            &context,
            "fixture",
            CommandSubmissionSource::Api,
            submission.clone(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.target, submission.target);
    assert_eq!(accepted.control.status, AgentResponseStatus::Queued);
    assert_eq!(
        service
            .admit_response_control(
                &context,
                "fixture",
                CommandSubmissionSource::Api,
                submission
            )
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! { "explicit_control": { "$exists": true } })
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[path = "onboarding_first_message_tests.rs"]
mod onboarding_first_message;
