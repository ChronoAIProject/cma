use super::*;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::resource_access::{
    CallerContext, MembershipError, OrganizationAuthority, OrganizationChoice,
    ResourceAccessService,
};
use layer2_core::{PlatformDefaults, RuntimeCapabilities, RuntimeCapability};
use std::time::{Duration, SystemTime};

struct NoAmbientAuthority;
#[async_trait::async_trait]
impl AgentCommandAuthorizer for NoAmbientAuthority {
    async fn authorize_command(
        &self,
        _: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError> {
        panic!("Trigger execution must preserve its admitted initiator")
    }
}
#[async_trait::async_trait]
impl OrganizationAuthority for NoAmbientAuthority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal Trigger fixture has no organization authority")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("a background command cannot select an organization")
    }
}

fn execution(
    accepted: &Accepted,
    runtime: Arc<Runtime>,
) -> Arc<crate::commands::ResourceCommandExecution> {
    let resources = accepted.f.resources.clone();
    let profiles = Arc::new(InMemoryProfileStore::default());
    let context = AgentContextService::new(
        SandboxService::new(
            resources.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(resources.clone(), profiles.clone()),
        AgentService::new(
            resources.clone(),
            profiles.clone(),
            runtime.clone(),
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        ),
        profiles,
        Arc::new(PlatformDefaults::default()),
    );
    Arc::new(crate::commands::ResourceCommandExecution {
        authority: Arc::new(NoAmbientAuthority),
        access: Arc::new(ResourceAccessService::new(
            resources.clone(),
            Arc::new(NoAmbientAuthority),
        )),
        store: resources.clone(),
        operations: AgentOperationService::new(resources, runtime),
        context,
    })
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_worker_reclaims_expired_undispatched_claim_without_stale_authority() {
    let accepted = Accepted::new().await;
    let runtime = Runtime::new(false, false);
    let execution = execution(&accepted, runtime.clone());
    let stamp = SystemTime::now();
    let claimed = accepted
        .f
        .resources
        .claim_next_command(CommandClaimRequest {
            agent_id: accepted.agent.id.clone(),
            holder: "departed-trigger-worker".into(),
            now: layer2_core::rfc3339_at(stamp),
            expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(3)),
        })
        .await
        .unwrap()
        .unwrap();
    let original_claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    let authority = execution
        .operations
        .authorize_trigger_claim(&claimed)
        .await
        .unwrap();
    assert!(execution
        .operations
        .trigger_ready_for_claim(&claimed)
        .await
        .is_err());
    let worker = AgentCommandWorker::new(
        accepted.f.resources.clone(),
        execution.clone(),
        "replacement-trigger-worker".into(),
    )
    .unwrap();
    let id = &accepted.command.receipt.envelope.command_id;
    worker.run_one(id).await.unwrap();
    assert_eq!(runtime.sends.load(Ordering::SeqCst), 0);
    assert_eq!(accepted.current_command().await, claimed);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(authority.current_proof().await.is_err());
    assert!(execution
        .operations
        .execute_trigger_claim(&claimed)
        .await
        .is_err());
    execution
        .operations
        .trigger_ready_for_claim(&claimed)
        .await
        .unwrap();
    for _ in 0..3 {
        worker.run_one(id).await.unwrap();
    }
    assert_eq!(runtime.sends.load(Ordering::SeqCst), 1);
    let dispatched = accepted.current_command().await;
    assert_eq!(dispatched.receipt, claimed.receipt);
    let dispatch = dispatched.work.dispatch.as_ref().unwrap();
    assert!(dispatch.claim.fence > original_claim.fence);
    assert_eq!(dispatch.claim.holder, "replacement-trigger-worker");
    let agent = accepted.current_agent().await;
    assert_eq!(agent.creation_key, accepted.agent.creation_key);
    assert_eq!(
        agent.conversation.conversation_id,
        accepted.agent.conversation.conversation_id
    );
    assert_eq!(
        agent.conversation.runtime_generation,
        accepted.agent.conversation.runtime_generation
    );
    assert!(execution
        .operations
        .execute_trigger_claim(&claimed)
        .await
        .is_err());
    assert!(execution
        .operations
        .trigger_ready_for_claim(&claimed)
        .await
        .is_err());
    worker.run_one(id).await.unwrap();
    assert_eq!(runtime.sends.load(Ordering::SeqCst), 1);
    accepted.f.close().await;
}
