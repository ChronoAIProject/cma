use super::{launch::UnavailableLauncher, routing_plan, routing_store::Candidate, IngressState};
use crate::{
    activity::{ActivityStore, Change, ConsumerState, ObservedActivity},
    channels::resolver::ChannelResolver,
    interactions::native_lark::fixture::{now, Fixture},
    native_lark::{
        message, read_status,
        receipts::{Receipt, Store},
        test_support as native,
    },
    turns::observer::NoopObserver,
};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    response::Response,
};
use cmaeg_core::channel::{LarkRegion, NativeLarkSource, Source};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
    time::Instant,
};

struct Setup {
    native: Fixture,
    source: NativeLarkSource,
    state: IngressState,
    activity: Arc<ActivityStore>,
    receipt: Receipt,
}
impl Setup {
    async fn new(named: bool, group: bool, region: LarkRegion, confirmed: bool) -> Self {
        let native = if group {
            Fixture::new_group(named, region).await
        } else {
            Fixture::new(named, false, region).await
        };
        let Source::NativeLark(source) = native.channel.definition.sources[0].clone() else {
            panic!()
        };
        let mut raw = native::text("om_original", "private source text", now());
        if group {
            raw["event"]["message"]["chat_type"] = "group".into();
        }
        let msg = message::extract(&source, &raw, now()).unwrap().unwrap();
        let request = message::launch(
            native.channel.clone(),
            &source,
            &msg,
            None,
            &routing_plan::fixture::keys(),
            Instant::now(),
            now(),
        )
        .unwrap();
        let routing_plan::OrdinaryPlan::Routed(plan) = routing_plan::plan(
            &request,
            &routing_plan::fixture::keys(),
            now(),
            routing_plan::RoutingOptions {
                enabled: true,
                attributes_signed: true,
                non_human_providers: &BTreeSet::new(),
            },
        )
        .unwrap() else {
            panic!()
        };
        let candidate = Candidate::prepare(
            &request,
            &plan,
            b"native read fixture",
            &routing_plan::fixture::keys(),
            now(),
        )
        .unwrap();
        native.server.store().admit(&candidate).await.unwrap();
        let receipt = native.receipt();
        let activity = Arc::new(ActivityStore::new(
            native.server.records.clone(),
            Arc::new(routing_plan::fixture::keys()),
        ));
        activity.received(&request).await.unwrap();
        activity
            .change(
                receipt.original.record_id.as_str(),
                receipt.original.event_id.as_str(),
                &Change::state(
                    receipt.consumer.as_str(),
                    ConsumerState::Completed,
                    Some("resp_original".into()),
                ),
            )
            .await
            .unwrap();
        let state = IngressState::new(
            ChannelResolver::new(
                Arc::new(native.server.records.clone()),
                Arc::new(routing_plan::fixture::keys()),
                Default::default(),
            ),
            Arc::new(routing_plan::fixture::keys()),
            Default::default(),
            Arc::new(UnavailableLauncher),
        )
        .unwrap()
        .with_native_lark_intake(true)
        .with_activity_observer(Arc::new(ObservedActivity::new(
            activity.clone(),
            Arc::new(NoopObserver),
        )));
        let result = Self {
            native,
            source,
            state,
            activity,
            receipt,
        };
        if confirmed {
            result.store().put(&result.receipt).await.unwrap();
        }
        result
    }
    fn store(&self) -> Store {
        Store::new(
            self.native.server.records.clone(),
            Arc::new(routing_plan::fixture::keys()),
        )
    }
    fn event(&self, age: i64, messages: &[&str]) -> Value {
        native::envelope(
            read_status::EVENT_TYPE,
            "read_event",
            json!({
                "reader":{"reader_id":{"open_id":"ou_human"},"tenant_key":self.source.identity.tenant_key,
                    "read_time":((now()-age)*1000+123).to_string()},"message_id_list":messages
            }),
        )
    }
    async fn call(&self, state: &IngressState, event: &Value, authentic: bool) -> Response {
        let sent = now();
        let (body, signature) = native::signed(event, sent);
        let request = Request::builder()
            .header("content-type", "application/json")
            .header("x-lark-request-timestamp", sent.to_string())
            .header("x-lark-request-nonce", "fixture_nonce")
            .header(
                "x-lark-signature",
                if authentic { signature } else { "0".repeat(64) },
            )
            .body(Body::from(body))
            .unwrap();
        crate::http::native_lark::callback(
            State(state.clone()),
            Path(self.native.channel.id.to_string()),
            request,
        )
        .await
    }
    async fn page(&self) -> crate::activity::ActivityPage {
        self.activity
            .latest(
                &self.receipt.original.owner,
                self.receipt.original.record_id.as_str(),
            )
            .await
            .unwrap()
    }
    async fn assert_unread(&self) {
        assert!(self.page().await.items[0]
            .consumers
            .iter()
            .all(|consumer| consumer.deliveries.is_empty()));
    }
}

#[tokio::test]
async fn signed_read_annotates_only_confirmed_original_consumer_and_replays_after_restart() {
    for (named, region) in [(false, LarkRegion::Lark), (true, LarkRegion::Feishu)] {
        let setup = Setup::new(named, false, region, true).await;
        let raw = setup.event(0, &["om_confirmed", "om_confirmed"]);
        let writes = setup.native.server.memory.writes.load(Ordering::SeqCst);
        assert_eq!(
            setup.call(&setup.state, &raw, false).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            setup.native.server.memory.writes.load(Ordering::SeqCst),
            writes
        );
        for field in ["app_id", "tenant_key"] {
            let mut other = raw.clone();
            other["header"][field] = "other".into();
            assert_eq!(
                setup.call(&setup.state, &other, true).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let mut malformed = raw.clone();
        malformed["event"]["message_id_list"] = json!(["om_confirmed", "invalid/message"]);
        assert_eq!(
            setup.call(&setup.state, &malformed, true).await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            setup.native.server.memory.writes.load(Ordering::SeqCst),
            writes
        );
        let records = setup.native.server.memory.rows.lock().unwrap().len();
        // No routing executor or usable launcher is installed on this state.
        assert_eq!(
            setup.call(&setup.state, &raw, true).await.status(),
            StatusCode::OK
        );
        let page = setup.page().await;
        assert_eq!(page.items.len(), 1);
        let consumer = page.items[0]
            .consumers
            .iter()
            .find(|consumer| consumer.consumer_id == setup.receipt.consumer.as_str())
            .unwrap();
        assert_eq!(consumer.state, ConsumerState::Completed);
        assert_eq!(consumer.response_id.as_deref(), Some("resp_original"));
        assert_eq!(consumer.deliveries.len(), 1);
        assert_eq!(consumer.deliveries[0].outcome, "read");
        assert_eq!(
            consumer.deliveries[0].platform_message_id.as_deref(),
            Some("om_confirmed")
        );
        assert_eq!(consumer.deliveries[0].recipients[0].id, "oc_native_chat");
        assert!(consumer.deliveries[0]
            .confirmed_at
            .as_deref()
            .unwrap()
            .contains(".123"));
        assert!(!serde_json::to_string(&page)
            .unwrap()
            .contains("private source text"));
        assert!(page.items[0]
            .consumers
            .iter()
            .filter(|c| c.consumer_id != setup.receipt.consumer.as_str())
            .all(|c| c.deliveries.is_empty()));
        let restored = Arc::new(ActivityStore::new(
            setup.native.server.records.clone(),
            Arc::new(routing_plan::fixture::keys()),
        ));
        let state = setup
            .state
            .clone()
            .with_activity_observer(Arc::new(ObservedActivity::new(
                restored,
                Arc::new(NoopObserver),
            )));
        let before = serde_json::to_value(&page.items).unwrap();
        let writes = setup.native.server.memory.writes.load(Ordering::SeqCst);
        assert_eq!(
            setup
                .call(&state, &setup.event(1, &["om_confirmed"]), true)
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            serde_json::to_value(setup.page().await.items).unwrap(),
            before
        );
        assert_eq!(
            setup.native.server.memory.writes.load(Ordering::SeqCst),
            writes
        );
        assert_eq!(
            setup.native.server.memory.rows.lock().unwrap().len(),
            records
        );
        assert_eq!(
            setup
                .store()
                .load(
                    &setup.native.channel.id,
                    &setup.receipt.source,
                    "om_confirmed",
                    now()
                )
                .await
                .unwrap()
                .unwrap(),
            setup.receipt
        );
        assert_eq!(
            setup
                .native
                .server
                .store()
                .recover_all_page(None, 32)
                .await
                .unwrap()
                .plans
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn unconfirmed_incoming_other_reader_and_group_messages_never_gain_read_status() {
    let setup = Setup::new(false, false, LarkRegion::Feishu, false).await;
    let event = setup.event(0, &["om_confirmed"]);
    assert_eq!(
        setup.call(&setup.state, &event, true).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    setup.assert_unread().await;
    setup.store().put(&setup.receipt).await.unwrap();
    let mut other = event.clone();
    other["event"]["reader"]["reader_id"]["open_id"] = "ou_other".into();
    assert_eq!(
        setup.call(&setup.state, &other, true).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        setup
            .call(
                &setup.state,
                &setup.event(121, &["om_original", "om_unknown"]),
                true
            )
            .await
            .status(),
        StatusCode::OK
    );
    setup.assert_unread().await;
    let group = Setup::new(false, true, LarkRegion::Feishu, true).await;
    assert_eq!(
        group
            .call(&group.state, &group.event(0, &["om_confirmed"]), true)
            .await
            .status(),
        StatusCode::OK
    );
    group.assert_unread().await;
    assert_eq!(
        setup.call(&setup.state, &event, true).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        setup.page().await.items[0].consumers[0].deliveries[0].outcome,
        "read"
    );
}

#[path = "native_lark_read_status_isolation_tests.rs"]
mod isolation_tests;

#[path = "native_lark_read_status_batch_tests.rs"]
mod batch_tests;
