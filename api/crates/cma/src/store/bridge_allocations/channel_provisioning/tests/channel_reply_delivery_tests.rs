use super::*;
use layer2_core::bridge::outbox::*;
use layer2_core::sandbox::store::AgentStore;
use std::sync::{Arc, Mutex};

/// The real outbox scheduler reaches the relay port while the original Agent
/// turn is still live. The port records requests without platform traffic.
struct ActiveTurnRelay {
    resources: Arc<crate::store::MongoSandboxStore>,
    command: AdmittedAgentCommand,
    message_id: String,
    calls: Mutex<Vec<serde_json::Value>>,
}

#[async_trait::async_trait]
impl RelayDeliveryPort for ActiveTurnRelay {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        request.require_current().unwrap();
        let command = self
            .resources
            .get_command(&self.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.status, AgentResponseStatus::InProgress);
        assert_eq!(command.receipt, self.command.receipt);
        let agent = self
            .resources
            .get_agent(&command.receipt.envelope.agent_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            agent.conversation.active_turn,
            command.work.dispatch.as_ref().map(|d| d.turn_sequence)
        );
        assert!(agent.conversation.active_turn.is_some());
        assert_eq!(request.path(), "/api/v1/channel-relay/reply");
        assert_eq!(request.item().source.message_id(), self.message_id);
        assert_eq!(request.item().source.chat_id(), "-100123");
        let body = request.body();
        assert_eq!(body["message_id"], self.message_id);
        let mut calls = self.calls.lock().unwrap();
        calls.push(body);
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: format!("fixture-mid-turn-{}", calls.len()),
            platform_message_id: Some(format!("fixture-telegram-{}", calls.len())),
        })
    }
}

async fn await_sent(f: &ReplyFixture, count: usize) {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let items = f.items().await;
            if items.len() == count
                && items.iter().all(|row| {
                    row.get_document("record")
                        .unwrap()
                        .get_document("state")
                        .unwrap()
                        .get_str("kind")
                        .unwrap()
                        == "sent"
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("explicit replies dispatch without ending the Agent turn");
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reply_scheduler_delivers_multiple_explicit_messages_while_original_turn_stays_active() {
    let f = ReplyFixture::new().await;
    let relay = Arc::new(ActiveTurnRelay {
        resources: f.resources.clone(),
        command: f.command.clone(),
        message_id: f.inbox.message_id.clone(),
        calls: Mutex::new(Vec::new()),
    });
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let scheduler = crate::bridge::outbox_scheduler::BridgeOutboxScheduler::new(
        f.replies.clone(),
        relay.clone(),
        "real-mid-turn-delivery".into(),
    )
    .spawn(receiver);
    let context = f.context().await;
    let ack = f.reply(&context, "ack-now", "*I have started.*").await;
    assert_eq!(ack["isError"], false, "{ack}");
    await_sent(&f, 1).await;
    let progress = f
        .reply(&context, "progress-now", "The _first_ step is complete.")
        .await;
    assert_eq!(progress["isError"], false, "{progress}");
    await_sent(&f, 2).await;
    assert_eq!(
        relay
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|body| body["reply"]["text"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        [r"\*I have started.\*", r"The \_first\_ step is complete."]
    );
    // A new message ID flushes the previous assembled message through the
    // actual update recorder while this turn remains active. The second
    // assistant message is then flushed by the real TurnEnded update.
    f.emit_assistant_text("cma-progress-only", "CMA-only progress output.")
        .await;
    f.emit_assistant_text("cma-final-only", "CMA-only final output.")
        .await;
    f.await_assistant_text("CMA-only progress output.").await;
    assert_eq!(
        f.resources
            .get_command(&f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::InProgress
    );
    assert_eq!(f.items().await.len(), 2);
    assert_eq!(relay.calls.lock().unwrap().len(), 2);
    f.finish().await;
    f.await_assistant_text("CMA-only final output.").await;
    let recovered = f.reply(&context, "ack-now", "*I have started.*").await;
    assert_eq!(recovered["isError"], false, "{recovered}");
    assert_eq!(
        recovered["structuredContent"]["receipt_id"],
        ack["structuredContent"]["receipt_id"]
    );
    assert_eq!(recovered["structuredContent"]["status"], "sent");
    assert_eq!(f.items().await.len(), 2);
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), scheduler)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(relay.calls.lock().unwrap().len(), 2);
    f.close().await;
}
