use super::*;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::sandbox::store::AgentStore;
use layer2_core::sandbox::*;
use layer2_core::{PlatformDefaults, RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::ConversationInputSender;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};

#[path = "writer_snapshot_tests.rs"]
mod writer_snapshot_tests;

struct NoHumanAuthority;
#[async_trait::async_trait]
impl AgentCommandAuthorizer for NoHumanAuthority {
    async fn authorize_command(
        &self,
        _: &AgentCommandEnvelope,
    ) -> Result<layer2_core::Caller, CommandAuthorizationError> {
        panic!("Channel must not request or synthesize a human Caller")
    }
}

struct Runtime {
    sender: ConversationInputSender,
    revoke: Mutex<Option<(BridgeInvokeService, RevokeBridgeInvokeGrant)>>,
    resolves: AtomicUsize,
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn fence(&self, _: &layer2_core::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("Channel message authority cannot retire or revive a runtime")
    }
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        // The common message service first checks runtime capabilities, then
        // resolves again after the durable dispatch binding for ordered input.
        let revoke = (self.resolves.fetch_add(1, Ordering::SeqCst) == 1)
            .then(|| self.revoke.lock().unwrap().take())
            .flatten();
        if let Some((service, request)) = revoke {
            service.revoke(&caller(), None, request).await.unwrap();
        }
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: self.sender.clone(),
        })
    }
}

pub(super) fn execution(
    c: &ChannelFixture,
    revoke_at_writer: bool,
) -> (
    Arc<crate::commands::ResourceCommandExecution>,
    Arc<AtomicUsize>,
) {
    let sends = Arc::new(AtomicUsize::new(0));
    let sent = sends.clone();
    let runtime = Arc::new(Runtime {
        resolves: AtomicUsize::new(0),
        sender: ConversationInputSender::spawn(4, move |_| {
            sent.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        }),
        revoke: Mutex::new(
            revoke_at_writer.then(|| (c.f.service.clone(), c.f.revoke(&c.grant, "writer-revoke"))),
        ),
    });
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
            runtime.clone(),
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        ),
        profiles,
        Arc::new(PlatformDefaults::default()),
    );
    (
        Arc::new(crate::commands::ResourceCommandExecution {
            channel_controls: None,
            channel_revival: None,
            channel: Some(Arc::new(ChannelAgentCommandService::new(
                c.commands.clone(),
            ))),
            invokes: Some(Arc::new(c.f.service.clone())),
            authority: Arc::new(NoHumanAuthority),
            access: Arc::new(ResourceAccessService::new(
                c.f.resources.clone(),
                c.f.authority.clone(),
            )),
            store: c.f.resources.clone(),
            operations: AgentOperationService::new(c.f.resources.clone(), runtime),
            context,
        }),
        sends,
    )
}

async fn held(c: &ChannelFixture) -> AdmittedAgentCommand {
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap()
}
async fn claimed(
    c: &ChannelFixture,
    execution: &crate::commands::ResourceCommandExecution,
    held: &AdmittedAgentCommand,
) -> AdmittedAgentCommand {
    execution.release_held(held).await;
    let now = SystemTime::now();
    c.f.resources
        .claim_next_command(CommandClaimRequest {
            agent_id: c.f.target.agent_id.clone(),
            holder: "channel-worker".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn common_worker_releases_channel_hold_and_dispatches_original_input_once_across_restart() {
    let c = ChannelFixture::new().await;
    let record = held(&c).await;
    let (execution, sends) = execution(&c, false);
    let worker = AgentCommandWorker::new(
        c.f.resources.clone(),
        execution.clone(),
        "channel-one".into(),
    )
    .unwrap();
    worker
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let current =
        c.f.resources
            .get_command(&record.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.receipt, record.receipt);
    assert_eq!(current.status, AgentResponseStatus::InProgress);
    assert_eq!(
        current.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::WriteConfirmed
    );
    let logs =
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 100)
            .await
            .unwrap();
    assert_eq!(logs.iter().filter(|entry| matches!(&entry.kind, AgentLogKind::OwnerMessage { content } if content == "Explain the result.")).count(), 1);
    assert_eq!(logs.iter().filter(|entry| matches!(&entry.kind, AgentLogKind::TurnStarted { origin_key: Some(key), .. } if key == &format!("command:{}", record.receipt.envelope.command_id.as_str()))).count(), 1);
    // Admission replay is still the same permanent command after it progresses.
    assert_eq!(
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        current
    );
    AgentCommandWorker::new(c.f.resources.clone(), execution, "channel-two".into())
        .unwrap()
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let document =
        c.f.resources
            .agent_commands
            .find_one(doc! {"_id": record.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        document
            .get_document("channel_admission")
            .unwrap()
            .get_str("execution_hold")
            .unwrap(),
        "released"
    );
    assert!(document.get_document("channel_dispatch_authority").is_ok());
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_dispatch_transaction_refuses_revoked_proof_without_input_log_or_binding() {
    let c = ChannelFixture::new().await;
    let held = held(&c).await;
    let (execution, sends) = execution(&c, false);
    let claimed = claimed(&c, &execution, &held).await;
    let authority = execution
        .channel
        .as_ref()
        .unwrap()
        .authorize_execution(&c.f.service, &claimed)
        .await
        .unwrap();
    let before =
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 100)
            .await
            .unwrap();
    c.f.service
        .revoke(&caller(), None, c.f.revoke(&c.grant, "revoke-before-bind"))
        .await
        .unwrap();
    assert!(execution
        .operations
        .execute_channel_claim(&authority, &claimed)
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(
        c.f.resources
            .agent_log(&c.f.target.agent_id, 0, 100)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        c.f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(claimed)
    );
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_writer_refresh_refuses_revocation_after_binding_and_records_truthful_prewrite_failure(
) {
    let c = ChannelFixture::new().await;
    let record = held(&c).await;
    let (execution, sends) = execution(&c, true);
    let worker =
        AgentCommandWorker::new(c.f.resources.clone(), execution, "writer-guard".into()).unwrap();
    worker
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let current =
        c.f.resources
            .get_command(&record.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(current.status, AgentResponseStatus::Failed);
    assert_eq!(
        current.work.observation.as_ref().unwrap().delivery,
        AgentOperationDelivery::PreWriteFailed
    );
    let agent =
        c.f.resources
            .get_agent(&c.f.target.agent_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(agent.status, AgentStatus::Ready);
    assert_eq!(agent.conversation.active_turn, None);
    c.close().await;
}
