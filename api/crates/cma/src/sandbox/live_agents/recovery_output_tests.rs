use super::*;
use layer2_core::sandbox::{AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};
use tokio::sync::Notify;

#[derive(Default)]
struct Recorder {
    entered: Notify,
    release: Notify,
}
#[async_trait::async_trait]
impl AgentUpdateRecorder for Recorder {
    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        })
    }
}
fn target() -> LiveAgentTarget {
    LiveAgentTarget {
        agent_id: AgentId("agt_recovery".into()),
        conversation_id: "original".into(),
        runtime_generation: 7,
    }
}
fn input() -> ConversationInputSender {
    ConversationInputSender::spawn(1, |_| async { Ok(()) })
}
fn output() -> (ConversationUpdate, mpsc::Receiver<RuntimeOutputSource>) {
    let (send, received) = mpsc::channel(1);
    (
        ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(
                RuntimeOutputSource {
                    stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                    cursor: 1,
                    native_conversation_id: "original".into(),
                    payload_sha256: "a".repeat(64),
                },
                send,
            ),
            update: None,
        },
        received,
    )
}

#[tokio::test]
async fn recovery_is_unpublished_and_keeps_the_same_pending_record_after_finish() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let target = target();
    let recorder = Arc::new(Recorder::default());
    let (send, received) = mpsc::channel(1);
    let recovery = registry
        .begin_recovery(
            &target,
            9,
            RuntimeCapabilities::default(),
            received,
            recorder.clone(),
        )
        .unwrap();
    let (update, mut ack) = output();
    send.send(update).await.unwrap();
    recorder.entered.notified().await;
    assert!(registry.resolve(&target).is_none());
    assert!(registry.staged_attachment(&target, 9).unwrap().is_none());
    let attachment = recovery.finish(input()).unwrap();
    assert!(registry.resolve(&target).is_none());
    registry.publish_and_activate(&attachment).unwrap();
    assert!(registry.resolve(&target).is_some());
    assert!(ack.try_recv().is_err());
    recorder.release.notify_one();
    assert_eq!(ack.recv().await.unwrap().cursor, 1);
    registry.fence(&target.agent_id, Some(7), 9);
}

#[tokio::test]
async fn cancelled_or_superseded_recovery_never_acks_or_removes_the_replacement() {
    for replace in [false, true] {
        let registry = Arc::new(LiveAgentRegistry::default());
        let target = target();
        let recorder = Arc::new(Recorder::default());
        let (send, received) = mpsc::channel(1);
        let mut recovery = registry
            .begin_recovery(
                &target,
                9,
                RuntimeCapabilities::default(),
                received,
                recorder.clone(),
            )
            .unwrap();
        let (update, mut ack) = output();
        send.send(update).await.unwrap();
        recorder.entered.notified().await;
        if replace {
            let (_send, received) = mpsc::channel(1);
            let successor = registry
                .begin_recovery(
                    &target,
                    10,
                    RuntimeCapabilities::default(),
                    received,
                    recorder.clone(),
                )
                .unwrap();
            recovery.stopped().await;
            drop(recovery);
            let attachment = successor.finish(input()).unwrap();
            registry.publish_and_activate(&attachment).unwrap();
            assert!(registry.resolve(&target).is_some());
            registry.fence(&target.agent_id, Some(7), 10);
        } else {
            drop(recovery);
        }
        send.closed().await;
        recorder.release.notify_one();
        assert!(ack.recv().await.is_none());
    }
}

#[tokio::test]
async fn ended_recovery_cannot_be_published() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let (send, received) = mpsc::channel(1);
    let mut recovery = registry
        .begin_recovery(
            &target(),
            9,
            RuntimeCapabilities::default(),
            received,
            Arc::new(Recorder::default()),
        )
        .unwrap();
    drop(send);
    recovery.stopped().await;
    assert_eq!(
        recovery.finish(input()).unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
    assert!(registry.resolve(&target()).is_none());
}

#[tokio::test]
async fn fenced_handoff_cannot_publish_or_ack_its_pending_write() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let target = target();
    let recorder = Arc::new(Recorder::default());
    let (send, received) = mpsc::channel(1);
    let recovery = registry
        .begin_recovery(
            &target,
            9,
            RuntimeCapabilities::default(),
            received,
            recorder.clone(),
        )
        .unwrap();
    let (update, mut ack) = output();
    send.send(update).await.unwrap();
    recorder.entered.notified().await;
    let attachment = recovery.finish(input()).unwrap();
    registry.fence(&target.agent_id, Some(7), 9);
    assert!(registry.publish_and_activate(&attachment).is_err());
    send.closed().await;
    recorder.release.notify_one();
    assert!(ack.recv().await.is_none());
    assert!(registry.resolve(&target).is_none());
}

#[tokio::test]
async fn manager_revocation_cancels_the_unpublished_output_owner() {
    use layer2_core::agent_ownership::{
        test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, PROTOCOL_VERSION,
    };
    use layer3_adapter::owner_protocol::TransportAuthority;
    let registry = Arc::new(LiveAgentRegistry::default());
    let coordinator = OwnershipCoordinator::register(
        Arc::new(MemoryOwnershipStore::default()),
        Member {
            instance_id: "recovery-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://owner.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 8,
        },
    )
    .await
    .unwrap();
    registry.install_ownership(coordinator.clone());
    let target = target();
    let authority = coordinator
        .acquire(&target.agent_id)
        .await
        .unwrap()
        .unwrap();
    let recorder = Arc::new(Recorder::default());
    let (send, received) = mpsc::channel(1);
    let mut recovery = authority
        .run(async {
            registry.begin_recovery(
                &target,
                9,
                RuntimeCapabilities::default(),
                received,
                recorder.clone(),
            )
        })
        .await
        .unwrap()
        .unwrap();
    let (update, mut ack) = output();
    send.send(update).await.unwrap();
    recorder.entered.notified().await;
    TransportAuthority::revoke(authority.as_ref());
    recovery.stopped().await;
    recorder.release.notify_one();
    assert!(ack.recv().await.is_none());
    assert!(registry.resolve(&target).is_none());
    assert!(recovery.finish(input()).is_err());
}
