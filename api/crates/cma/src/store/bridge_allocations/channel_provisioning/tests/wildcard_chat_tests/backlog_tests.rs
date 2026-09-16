use super::*;
use layer2_core::bridge::outbox::{management::*, *};

async fn two_sessions() -> (ReplyFixture, ReplyFixture) {
    let setup = fixture::Setup::new().await;
    let router = http::router(&setup).await;
    let mut sessions = Vec::new();
    for (message, chat) in [("backlog-a", -100123), ("backlog-b", -100456)] {
        let signed = http::SignedChat::new(message, chat);
        let (_, inbox) = http::accept(router.clone(), &setup, &signed).await;
        let reservation = setup.reserve_request(&inbox).await;
        let allocation = setup.allocations.reserve(reservation).await.unwrap();
        sessions.push(execution::start(setup.partition(allocation).await, inbox).await);
    }
    let b = sessions.pop().unwrap();
    let a = sessions.pop().unwrap();
    assert!(!Arc::ptr_eq(&a.replies, &b.replies));
    assert_ne!(a.command.receipt.response_id, b.command.receipt.response_id);
    (a, b)
}

/// Invoke the production typed tool store with real session/current-turn proof.
/// Large chunk receipts exceed this fixture's small HTTP response-read bound.
async fn fill(f: &ReplyFixture, context: &str, chunks: usize) -> (String, ReplyToolReceipt) {
    let text = "a".repeat(chunks * 4000);
    let session = f.tools.authenticate(&f.credential).await.unwrap();
    let scope = f.replies.current_turn(&session, context).await.unwrap();
    let receipt = f
        .replies
        .enqueue(
            ChannelReplyToolIntent::prepare(
                scope,
                ChannelReplyToolRequest {
                    request_key: "fill-backlog".into(),
                    text: text.clone(),
                },
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.outbox_ids.len(), chunks);
    (text, receipt)
}

async fn recover_fill(f: &ReplyFixture, text: &str) -> ReplyToolReceipt {
    let session = f.tools.authenticate(&f.credential).await.unwrap();
    f.replies
        .recover(
            &session,
            &ChannelReplyToolRequest {
                request_key: "fill-backlog".into(),
                text: text.into(),
            },
        )
        .await
        .unwrap()
        .unwrap()
}

fn assert_full(reply: &Value) {
    assert_eq!(reply["isError"], true, "{reply}");
    assert_eq!(reply["structuredContent"]["code"], "reply_backlog_full");
}

async fn manage(
    f: &ReplyFixture,
    id: &str,
    key: &str,
    action: OutboxRecoveryAction,
) -> OutboxRecoveryReceipt {
    let service = BridgeOutboxService::new(f.svc.clone(), f.replies.clone());
    let bridge = f
        .bridges
        .bridge(&f.inbox.pin.bridge_id)
        .await
        .unwrap()
        .unwrap();
    let item = service
        .detail(&caller(), None, &bridge.id, id)
        .await
        .unwrap();
    service
        .command(
            &caller(),
            None,
            &bridge.id,
            key,
            OutboxRecoveryRequest {
                item_id: id.into(),
                expected_revision: item.revision,
                expected_bridge_revision: bridge.revision,
                action,
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_distinct_channel_stores_share_atomic_backlog_and_recover_at_capacity() {
    let (a, b) = two_sessions().await;
    let a_context = a.context().await;
    let b_context = b.context().await;
    let limit = crate::store::MongoBridgeReplyToolStore::MAX_OUTSTANDING_ITEMS as usize;
    let (text, retained) = fill(&a, &a_context, limit - 1).await;
    // Two chunks cannot partially occupy the one remaining slot.
    assert_full(&b.reply(&b_context, "two-chunks", &"b".repeat(4001)).await);
    assert_eq!(a.items().await.len(), limit - 1);
    let (first, second) = tokio::join!(
        a.reply(&a_context, "race-a", "First contender."),
        b.reply(&b_context, "race-b", "Second contender."),
    );
    assert_eq!(
        [&first, &second]
            .into_iter()
            .filter(|r| r["isError"] == false)
            .count(),
        1
    );
    let (winner, context, key, message, receipt) = if first["isError"] == false {
        assert_full(&second);
        (&a, &a_context, "race-a", "First contender.", first)
    } else {
        assert_full(&first);
        (&b, &b_context, "race-b", "Second contender.", second)
    };
    assert_eq!(a.items().await.len(), limit);
    assert_eq!(winner.reply(context, key, message).await, receipt);
    assert_eq!(recover_fill(&a, &text).await, retained);
    assert_full(&b.reply(&b_context, "over-limit", "Wait.").await);
    let id = receipt["structuredContent"]["outbox_ids"][0]
        .as_str()
        .unwrap();
    let terminal = manage(winner, id, "release-one", OutboxRecoveryAction::Abandon {}).await;
    assert_eq!(terminal.result.status, OutboxStatus::Abandoned);
    let admitted = b
        .reply(&b_context, "after-release", "One slot is free.")
        .await;
    assert_eq!(admitted["isError"], false, "{admitted}");
    assert_eq!(
        a.items().await.len(),
        limit + 1,
        "terminal history is retained"
    );
    let duplicate = winner.reply(context, key, message).await;
    assert_eq!(duplicate["structuredContent"]["status"], "abandoned");
    assert_eq!(
        duplicate["structuredContent"]["receipt_id"],
        receipt["structuredContent"]["receipt_id"]
    );
    assert_full(&a.reply(&a_context, "still-full", "Wait.").await);
    a.close().await;
}

struct UncertainRelay {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl RelayDeliveryPort for UncertainRelay {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        request.require_current().unwrap();
        self.entered.notify_one();
        self.release.notified().await;
        DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost)
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn sending_and_unknown_keep_backlog_slots_and_explicit_unknown_retry_needs_no_new_slot() {
    let f = ReplyFixture::new().await;
    let context = f.context().await;
    let original = f.reply(&context, "uncertain", "Original.").await;
    assert_eq!(original["isError"], false, "{original}");
    let id = original["structuredContent"]["outbox_ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let relay = Arc::new(UncertainRelay {
        entered: Default::default(),
        release: Default::default(),
    });
    let replies = f.replies.clone();
    let sending_relay = relay.clone();
    let worker = tokio::spawn(async move {
        replies
            .deliver_one("backlog-unknown", sending_relay.as_ref())
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), relay.entered.notified())
        .await
        .unwrap();
    let limit = crate::store::MongoBridgeReplyToolStore::MAX_OUTSTANDING_ITEMS as usize;
    fill(&f, &context, limit - 1).await;
    assert_full(&f.reply(&context, "sending-full", "Wait.").await);
    relay.release.notify_one();
    assert!(worker.await.unwrap().unwrap());
    assert_full(&f.reply(&context, "unknown-full", "Wait.").await);
    let recovered = f.reply(&context, "uncertain", "Original.").await;
    assert_eq!(recovered["structuredContent"]["status"], "unknown");
    assert_eq!(
        recovered["structuredContent"]["receipt_id"],
        original["structuredContent"]["receipt_id"]
    );
    let retried = manage(
        &f,
        &id,
        "acknowledged-retry",
        OutboxRecoveryAction::Retry {
            accept_duplicate_risk: true,
        },
    )
    .await;
    assert_eq!(retried.result.status, OutboxStatus::Pending);
    assert!(retried.result.duplicate_risk);
    assert_full(&f.reply(&context, "retry-full", "Wait.").await);
    assert_eq!(f.items().await.len(), limit);
    f.close().await;
}
