use super::*;
use crate::bridge::ingress::reply_addressing::{ConfirmedReply, ReplyAddressingStore, ReplyLookup};
use layer2_core::bridge::{outbox::OutboxItem, partition::GroupAdmission};
use std::sync::atomic::AtomicBool;

struct SentStore {
    item: OutboxItem,
    available: AtomicBool,
    unavailable: AtomicBool,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl ReplyAddressingStore for SentStore {
    async fn confirmed_reply(
        &self,
        lookup: &ReplyLookup,
    ) -> Result<Option<ConfirmedReply>, CallbackError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(CallbackError::Unavailable);
        }
        if self.available.load(Ordering::SeqCst) {
            lookup.confirm(&self.item)
        } else {
            Ok(None)
        }
    }
}

#[tokio::test]
async fn signed_group_reply_uses_confirmed_evidence_and_replay_never_changes_prior_admission() {
    for initially_confirmed in [true, false] {
        let mut f = HttpFixture::new(false);
        f.protocol.body["reply_to_platform_message_id"] = json!("40");
        f.protocol.raw_mut()["message"]["reply_to_message"] =
            json!({"message_id":40,"chat":{"id":-100123,"type":"supergroup"}});
        if let BridgeAdmissionPolicy::Scoped { policy } = &mut f.protocol.config_mut().admission {
            policy.groups = GroupAdmission::ReplyToBot;
        }
        Arc::get_mut(&mut f.provider).unwrap().bridge = f.protocol.bridge.clone();
        let store = Arc::new(SentStore {
            item: crate::bridge::ingress::reply_addressing::tests::sent(&f.protocol),
            available: AtomicBool::new(initially_confirmed),
            unavailable: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        f.provider.commits.add_permits(2);
        let first = value(
            f.router_with_reply_addressing(Some(store.clone()))
                .oneshot(f.request())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            first["receipt"]["admission"] == json!("accepted"),
            initially_confirmed
        );
        store
            .available
            .store(!initially_confirmed, Ordering::SeqCst);
        let replay = value(
            f.router_with_reply_addressing(Some(store.clone()))
                .oneshot(f.request())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            replay, first,
            "receipt recovery cannot turn old denial into fresh admission or erase acceptance"
        );
        assert_eq!(f.provider.rows.lock().await.len(), 1);
        assert_eq!(store.calls.load(Ordering::SeqCst), 2);
        store.unavailable.store(true, Ordering::SeqCst);
        let recovered = value(
            f.router_with_reply_addressing(Some(store.clone()))
                .oneshot(f.request())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            recovered, first,
            "outbox lookup failure must allow exact receipt recovery"
        );
        let mut fresh = f.protocol.clone();
        fresh.body["message_id"] = json!("fresh-during-outbox-outage");
        let request = fixture::request(&fresh);
        assert_eq!(
            f.router_with_reply_addressing(Some(store.clone()))
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(f.provider.rows.lock().await.len(), 1);
        let mut invalid = f.request();
        invalid.headers_mut().insert(
            "x-nyxid-callback-token",
            "invalid-signature".parse().unwrap(),
        );
        assert_ne!(
            f.router_with_reply_addressing(Some(store.clone()))
                .oneshot(invalid)
                .await
                .unwrap()
                .status(),
            StatusCode::ACCEPTED
        );
        assert_eq!(
            store.calls.load(Ordering::SeqCst),
            4,
            "unverified body must not query stored reply evidence"
        );
    }
}
