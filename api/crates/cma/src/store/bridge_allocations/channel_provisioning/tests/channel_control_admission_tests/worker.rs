use super::*;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::*;
use layer2_core::{PlatformDefaults, RuntimeCapabilities, RuntimeCapability};

struct NoHuman;
#[async_trait::async_trait]
impl AgentCommandAuthorizer for NoHuman {
    async fn authorize_command(
        &self,
        _: &AgentCommandEnvelope,
    ) -> Result<layer2_core::Caller, CommandAuthorizationError> {
        panic!("held Channel control must never synthesize a human actor")
    }
}

pub(super) async fn prove_old_worker_holds(c: &ControlFixture, result: &AdmittedChannelControl) {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let context = AgentContextService::new(
        SandboxService::new(
            c.f.resources.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(c.f.resources.clone(), profiles.clone()),
        AgentService::new(
            c.f.resources.clone(),
            profiles.clone(),
            c.f.registry.clone(),
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        ),
        profiles,
        Arc::new(PlatformDefaults::default()),
    );
    let old_channel = Arc::new(ChannelAgentCommandService::new(Arc::new(
        crate::store::MongoChannelCommandStore::new(c.invokes.clone()),
    )));
    let execution = Arc::new(crate::commands::ResourceCommandExecution {
        channel_controls: None,
        channel_revival: None,
        channel: Some(old_channel.clone()),
        invokes: Some(Arc::new(c.f.invokes.clone())),
        authority: Arc::new(NoHuman),
        access: Arc::new(ResourceAccessService::new(
            c.f.resources.clone(),
            Arc::new(Authority),
        )),
        store: c.f.resources.clone(),
        operations: AgentOperationService::new(c.f.resources.clone(), c.f.registry.clone()),
        context,
    });
    let worker = AgentCommandWorker::new(
        c.f.resources.clone(),
        execution,
        "old-control-worker".into(),
    )
    .unwrap();
    worker.run_one(&result.sidecar.command_id).await.unwrap();
    assert_eq!(
        c.f.resources
            .get_command(&result.sidecar.command_id)
            .await
            .unwrap()
            .unwrap(),
        result.command
    );
    assert!(matches!(
        old_channel
            .authorize_execution(&c.f.invokes, &result.command)
            .await,
        Err(ChannelCommandAdmissionError::SourceMismatch)
    ));
    // The direct generic rejection API also cannot discard an unleased wait.
    assert_eq!(
        c.f.resources
            .reject_unclaimed_command(
                &result.sidecar.command_id,
                result.command.revision,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339()
            )
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    let now = std::time::SystemTime::now();
    assert!(c
        .f
        .resources
        .claim_next_command(CommandClaimRequest {
            agent_id: result.command.receipt.envelope.agent_id.clone(),
            holder: "old-direct-claim".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        c.f.resources
            .get_command(&result.sidecar.command_id)
            .await
            .unwrap()
            .unwrap(),
        result.command
    );
}
