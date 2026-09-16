use super::*;
use layer2_core::bridge::outbox::*;
use std::sync::Mutex;

struct EditRelay {
    unknown_edit: bool,
    calls: Mutex<Vec<(String, serde_json::Value)>>,
}
#[async_trait::async_trait]
impl RelayDeliveryPort for EditRelay {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        request.require_current().unwrap();
        self.calls
            .lock()
            .unwrap()
            .push((request.path().into(), request.body()));
        let receipt = DeliveryReceipt {
            nyxid_message_id: "retained-outbound".into(),
            platform_message_id: Some("om_confirmed_reply".into()),
        };
        match request.item().operation {
            RelayOperation::InitialReply => assert!(request.edit_original_receipt().is_none()),
            RelayOperation::Edit { .. } => {
                assert_eq!(request.edit_original_receipt(), Some(&receipt));
                assert_eq!(request.body()["message_id"], "om_confirmed_reply");
                if self.unknown_edit {
                    return DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost);
                }
            }
        }
        DeliveryOutcome::Confirmed(receipt)
    }
}
async fn edit(
    f: &ReplyFixture,
    context: &str,
    original: &str,
    key: &str,
    text: &str,
) -> serde_json::Value {
    f.call(
        "edit_channel_reply",
        json!({
            "reply_context":context, "original_outbox_id":original, "request_key":key, "text":text
        }),
    )
    .await
}
fn outbox_id(receipt: &serde_json::Value) -> String {
    receipt["structuredContent"]["outbox_ids"][0]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn deliver_until(f: &ReplyFixture, relay: &EditRelay, calls: usize) {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            f.replies
                .deliver_one("explicit-edit-fixture", relay)
                .await
                .unwrap();
            if relay.calls.lock().unwrap().len() == calls {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("durable reply/edit dispatched within bound");
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn explicit_lark_and_feishu_edits_reuse_confirmed_reply_and_recover_after_turn_completion() {
    for platform in [BridgePlatform::Lark, BridgePlatform::Feishu] {
        let f = ReplyFixture::with_platform(platform).await;
        let relay = EditRelay {
            unknown_edit: false,
            calls: Mutex::new(Vec::new()),
        };
        let context = f.context().await;
        let first = f.reply(&context, "original", "Working.").await;
        assert_eq!(first["isError"], false, "{first}");
        let original = outbox_id(&first);
        assert_eq!(
            edit(&f, &context, &original, "unsent-edit", "Premature.").await["isError"],
            true
        );
        assert_eq!(f.items().await.len(), 1);
        deliver_until(&f, &relay, 1).await;
        let sent = f.reply(&context, "original", "Working.").await;
        assert_eq!(sent["structuredContent"]["status"], "sent");
        assert_eq!(outbox_id(&sent), original);
        let queued = edit(
            &f,
            &context,
            &original,
            "intentional-edit",
            "The first step is done.",
        )
        .await;
        assert_eq!(queued["isError"], false, "{queued}");
        assert_eq!(queued["structuredContent"]["status"], "queued");
        assert_eq!(
            edit(
                &f,
                &context,
                &original,
                "intentional-edit",
                "The first step is done."
            )
            .await,
            queued
        );
        assert_eq!(
            edit(&f, &context, &original, "intentional-edit", "Different.").await["isError"],
            true
        );
        assert_eq!(
            edit(&f, &context, &original, "original", "Working.").await["isError"],
            true
        );
        assert_eq!(
            edit(&f, &context, &original, "oversized", &"x".repeat(1901)).await["isError"],
            true
        );
        assert_eq!(f.items().await.len(), 2);
        deliver_until(&f, &relay, 2).await;
        assert_eq!(
            edit(&f, &context, &outbox_id(&queued), "edit-an-edit", "No.").await["isError"],
            true
        );
        let command = f
            .resources
            .get_command(&f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.status, AgentResponseStatus::InProgress);
        let second = edit(
            &f,
            &context,
            &original,
            "intentional-second-edit",
            "Both steps are done.",
        )
        .await;
        assert_eq!(second["isError"], false, "{second}");
        deliver_until(&f, &relay, 3).await;
        f.emit_assistant_text("edit-final", "CMA-only final output.")
            .await;
        f.finish().await;
        let recovered = edit(
            &f,
            &context,
            &original,
            "intentional-edit",
            "The first step is done.",
        )
        .await;
        assert_eq!(recovered["isError"], false, "{recovered}");
        assert_eq!(
            recovered["structuredContent"]["receipt_id"],
            queued["structuredContent"]["receipt_id"]
        );
        assert_eq!(recovered["structuredContent"]["status"], "sent");
        assert_eq!(
            edit(&f, &context, &original, "after-turn", "Late.").await["isError"],
            true
        );
        assert_eq!(f.items().await.len(), 3);
        let calls = relay.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, "/api/v1/channel-relay/reply");
        assert_eq!(calls[1].0, "/api/v1/channel-relay/reply/update");
        assert_eq!(calls[1].1["reply"]["text"], "The first step is done.");
        assert_eq!(calls[2].1["message_id"], calls[1].1["message_id"]);
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn unknown_edit_recovers_without_resend_and_blocks_following_output() {
    let f = ReplyFixture::with_platform(BridgePlatform::Lark).await;
    let relay = EditRelay {
        unknown_edit: true,
        calls: Mutex::new(Vec::new()),
    };
    let context = f.context().await;
    let original = outbox_id(&f.reply(&context, "original", "Working.").await);
    deliver_until(&f, &relay, 1).await;
    let queued = edit(&f, &context, &original, "uncertain-edit", "Updated.").await;
    assert_eq!(queued["isError"], false, "{queued}");
    deliver_until(&f, &relay, 2).await;
    let recovered = edit(&f, &context, &original, "uncertain-edit", "Updated.").await;
    assert_eq!(recovered["isError"], false, "{recovered}");
    assert_eq!(recovered["structuredContent"]["status"], "unknown");
    assert_eq!(
        recovered["structuredContent"]["receipt_id"],
        queued["structuredContent"]["receipt_id"]
    );
    let next = edit(&f, &context, &original, "new-edit", "Should wait.").await;
    assert_eq!(next["isError"], false, "{next}");
    let reply = f.reply(&context, "later-reply", "Should also wait.").await;
    assert_eq!(reply["isError"], false, "{reply}");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    for _ in 0..4 {
        f.replies
            .deliver_one("uncertain-edit-worker", &relay)
            .await
            .unwrap();
    }
    assert_eq!(relay.calls.lock().unwrap().len(), 2);
    assert_eq!(f.items().await.len(), 4);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn telegram_session_cannot_edit_even_a_confirmed_reply() {
    let f = ReplyFixture::new().await;
    let relay = EditRelay {
        unknown_edit: false,
        calls: Mutex::new(Vec::new()),
    };
    let context = f.context().await;
    let original = outbox_id(&f.reply(&context, "original", "Working.").await);
    deliver_until(&f, &relay, 1).await;
    assert_eq!(
        edit(&f, &context, &original, "unsupported", "No.").await["isError"],
        true
    );
    assert_eq!(f.items().await.len(), 1);
    assert_eq!(relay.calls.lock().unwrap().len(), 1);
    f.close().await;
}
