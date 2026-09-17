use super::*;
use layer2_core::sandbox::AgentUpdateOutcome;
use tokio::sync::Notify;

struct StartupRecorder {
    events: mpsc::UnboundedSender<&'static str>,
    release: Notify,
    refuse: bool,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for StartupRecorder {
    async fn restore_assembly(
        &self,
        id: &AgentId,
        generation: u64,
        assembly: &mut AgentMessageAssembly,
    ) -> Result<(), AgentConversationError> {
        assert_eq!(id.as_str(), "agt_restore");
        assert_eq!(generation, 19);
        self.events.send("restore").unwrap();
        self.release.notified().await;
        if self.refuse {
            return Err(AgentConversationError::Corrupt("invalid anchor".into()));
        }
        *assembly = serde_json::from_value(serde_json::json!({
            "message_id": "reply", "content": "retained prefix", "prompt_evidence": true,
            "live_message": null, "live_thought": null,
        }))
        .unwrap();
        self.events.send("restored").unwrap();
        Ok(())
    }

    async fn record_update(
        &self,
        _id: &AgentId,
        _generation: u64,
        assembly: &mut AgentMessageAssembly,
        _update: ConversationUpdate,
        _at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        assert!(!assembly.is_empty(), "restore must precede native updates");
        self.events.send("record").unwrap();
        Ok(AgentUpdateOutcome {
            appended: Vec::new(),
            turn_closed: false,
        })
    }
}

fn attachment() -> LiveAgentAttachmentIdentity {
    LiveAgentAttachmentIdentity {
        agent_id: AgentId("agt_restore".into()),
        conversation_id: "retained-session".into(),
        runtime_generation: 19,
        authority_fence: 1,
        nonce: 1,
    }
}

async fn next(events: &mut mpsc::UnboundedReceiver<&'static str>) -> &'static str {
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("attachment task must make progress")
        .expect("recorder event")
}

#[tokio::test]
async fn quiet_attachment_restores_before_waiting_for_any_native_update() {
    let (events, mut received) = mpsc::unbounded_channel();
    let recorder = Arc::new(StartupRecorder {
        events,
        release: Notify::new(),
        refuse: false,
    });
    let (activate, activated) = oneshot::channel();
    let (send, updates) = mpsc::channel(1);
    let task = spawn_update_task(
        Weak::new(),
        attachment(),
        activated,
        updates,
        recorder.clone(),
    );
    activate.send(()).unwrap();
    assert_eq!(next(&mut received).await, "restore");
    // The runtime has emitted nothing, but durable restoration has already begun.
    send.send(ConversationUpdate::Lost { frames: 1 })
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert!(received.try_recv().is_err());
    recorder.release.notify_one();
    assert_eq!(next(&mut received).await, "restored");
    assert_eq!(next(&mut received).await, "record");
    task.abort();
}

#[tokio::test]
async fn invalid_restoration_does_not_record_a_queued_native_update() {
    let (events, mut received) = mpsc::unbounded_channel();
    let recorder = Arc::new(StartupRecorder {
        events,
        release: Notify::new(),
        refuse: true,
    });
    let (activate, activated) = oneshot::channel();
    let (send, updates) = mpsc::channel(1);
    let task = spawn_update_task(
        Weak::new(),
        attachment(),
        activated,
        updates,
        recorder.clone(),
    );
    activate.send(()).unwrap();
    assert_eq!(next(&mut received).await, "restore");
    send.send(ConversationUpdate::Lost { frames: 1 })
        .await
        .unwrap();
    recorder.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("invalid restoration must close the attachment");
    assert!(received.try_recv().is_err());
}
