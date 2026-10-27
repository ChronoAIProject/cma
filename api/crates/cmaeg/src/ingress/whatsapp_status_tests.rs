use super::*;
use crate::activity::{ActivityStore, ObservedActivity};
use crate::turns::observer::NoopObserver;

struct StatusSetup {
    setup: Setup,
    app: axum::Router,
    activity: Arc<ActivityStore>,
    receipt: Receipt,
}
impl StatusSetup {
    async fn new() -> Self {
        Self::with_group(false).await
    }
    async fn with_group(group: bool) -> Self {
        let mut setup = Setup::new().await;
        if group {
            Arc::make_mut(&mut setup.channel).definition.sources =
                vec![crate::whatsapp::groups::tests::source_group()];
            setup.state = setup.state.with_whatsapp_groups(true);
        }
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        setup.server.memory.clock.store(now, Ordering::SeqCst);
        let activity = Arc::new(ActivityStore::new(
            setup.server.records.clone(),
            Arc::new(fixture::keys()),
        ));
        setup.state = setup
            .state
            .clone()
            .with_activity_observer(Arc::new(ObservedActivity::new(
                activity.clone(),
                Arc::new(NoopObserver),
            )));
        let app = setup.http();
        let message = if group {
            crate::whatsapp::groups::tests::group_text("wamid.status-origin")
        } else {
            text("wamid.status-origin", "hello")
        };
        admitted(setup.launch(setup.verified(current(message), vec![])).await);
        let plan = setup
            .server
            .store()
            .load(&setup.identity("wamid.status-origin"))
            .await
            .unwrap()
            .unwrap();
        let target = &plan.targets[0].launch;
        let group = match &target.event.reply {
            cmaeg_core::seal::ReplyAddress::WhatsappGroup(reply) => Some(reply.clone()),
            _ => None,
        };
        let receipt = Receipt {
            kind: if group.is_some() {
                "native_whatsapp_group_receipt_v1"
            } else {
                "native_whatsapp_receipt_v1"
            }
            .into(),
            recipient: group
                .as_ref()
                .map_or_else(|| "447700900123".into(), |reply| reply.group_id.clone()),
            group,
            original: plan.identity.clone(),
            source: plan.activity.source.clone(),
            message_id: "wamid.status-reply".into(),
            consumer: target.consumer.clone(),
            provider: target.provider.clone(),
            partition_key: target.event.partition_key.clone(),
            conversation_id: target.event.conversation_id.clone(),
            confirmed_at: now,
            retained_until: now + RETENTION,
        };
        setup
            .state
            .whatsapp_receipts
            .as_ref()
            .unwrap()
            .put(&receipt)
            .await
            .unwrap();
        Self {
            setup,
            app,
            activity,
            receipt,
        }
    }
    fn body(&self, kind: &str, age: i64) -> Vec<u8> {
        let mut body = json!({"object":"whatsapp_business_account","entry":[{
            "id":"23456","changes":[{"field":"messages","value":{
                "messaging_product":"whatsapp","metadata":{"phone_number_id":"34567"},
                "statuses":[{"id":self.receipt.message_id,"status":kind,
                    "recipient_id":self.receipt.recipient,
                    "timestamp":(time::OffsetDateTime::now_utc().unix_timestamp()-age).to_string()}]}}]}]});
        if self.receipt.group.is_some() {
            let status = &mut body["entry"][0]["changes"][0]["value"]["statuses"][0];
            status["recipient_type"] = "group".into();
            status["recipient_participant_id"] = "447700900123".into();
        }
        serde_json::to_vec(&body).unwrap()
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
}

#[tokio::test]
async fn authenticated_statuses_update_only_original_activity_and_replay_after_restart() {
    let fixture = StatusSetup::new().await;
    let before = fixture.setup.server.memory.writes.load(Ordering::SeqCst);
    let body = fixture.body("read", 0);
    let response = call(
        fixture.app.clone(),
        fixture.setup.callback(body.clone(), false),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        fixture.setup.server.memory.writes.load(Ordering::SeqCst),
        before
    );
    let response = call(
        fixture.app.clone(),
        fixture.setup.callback(body.clone(), true),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_response(response).await,
        json!({"accepted":0,"refused":0})
    );
    let page = fixture.page().await;
    assert_eq!(page.items.len(), 1);
    let consumer = page.items[0]
        .consumers
        .iter()
        .find(|c| c.consumer_id == fixture.receipt.consumer.as_str())
        .unwrap();
    assert_eq!(consumer.deliveries.len(), 1);
    assert_eq!(consumer.deliveries[0].outcome, "read");
    assert_eq!(
        consumer.deliveries[0].platform_message_id.as_deref(),
        Some("wamid.status-reply")
    );
    let durable = serde_json::to_value(&page.items).unwrap();
    let after = fixture.setup.server.memory.writes.load(Ordering::SeqCst);
    let restored = Arc::new(ActivityStore::new(
        fixture.setup.server.records.clone(),
        Arc::new(fixture::keys()),
    ));
    let state =
        fixture
            .setup
            .state
            .clone()
            .with_activity_observer(Arc::new(ObservedActivity::new(
                restored.clone(),
                Arc::new(NoopObserver),
            )));
    let app = crate::http::whatsapp::router(state);
    for bytes in [body, fixture.body("delivered", 1), fixture.body("sent", 2)] {
        let response = call(app.clone(), fixture.setup.callback(bytes, true)).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(
        fixture.setup.server.memory.writes.load(Ordering::SeqCst),
        after
    );
    assert_eq!(
        serde_json::to_value(fixture.page().await.items).unwrap(),
        durable
    );
    // A failure remains a separate observed fact and grants no resend authority.
    let response = call(app, fixture.setup.callback(fixture.body("failed", 0), true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = fixture.page().await;
    let deliveries = &page.items[0]
        .consumers
        .iter()
        .find(|c| c.consumer_id == fixture.receipt.consumer.as_str())
        .unwrap()
        .deliveries;
    assert_eq!(deliveries.len(), 2);
    assert!(deliveries.iter().any(|d| d.outcome == "read"));
    assert!(deliveries.iter().any(|d| d.outcome == "failed"));
}

#[tokio::test]
async fn receipt_and_original_plan_races_retry_observation_without_launching_or_resending() {
    let fixture = StatusSetup::new().await;
    *fixture
        .setup
        .server
        .memory
        .unavailable_read_id
        .lock()
        .unwrap() = Some(fixture.receipt.original.admission_id());
    let body = fixture.body("delivered", 0);
    let response = call(
        fixture.app.clone(),
        fixture.setup.callback(body.clone(), true),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(fixture.page().await.items[0]
        .consumers
        .iter()
        .all(|c| c.deliveries.is_empty()));
    *fixture
        .setup
        .server
        .memory
        .unavailable_read_id
        .lock()
        .unwrap() = None;
    assert_eq!(
        call(fixture.app.clone(), fixture.setup.callback(body, true))
            .await
            .status(),
        StatusCode::OK
    );
    let before = fixture.setup.server.memory.writes.load(Ordering::SeqCst);
    for (age, expected) in [(0, StatusCode::SERVICE_UNAVAILABLE), (121, StatusCode::OK)] {
        let mut body: Value = serde_json::from_slice(&fixture.body("read", age)).unwrap();
        body["entry"][0]["changes"][0]["value"]["statuses"][0]["recipient_id"] =
            "447700900999".into();
        let response = call(
            fixture.app.clone(),
            fixture
                .setup
                .callback(serde_json::to_vec(&body).unwrap(), true),
        )
        .await;
        assert_eq!(response.status(), expected);
    }
    assert_eq!(
        fixture.setup.server.memory.writes.load(Ordering::SeqCst),
        before
    );
    assert_eq!(fixture.page().await.items.len(), 1);
}

#[tokio::test]
async fn group_statuses_keep_each_participant_under_the_original_group_consumer() {
    let fixture = StatusSetup::with_group(true).await;
    let body = fixture.body("read", 0);
    assert_eq!(
        call(
            fixture.app.clone(),
            fixture.setup.callback(body.clone(), true)
        )
        .await
        .status(),
        StatusCode::OK
    );
    let mut second: Value = serde_json::from_slice(&body).unwrap();
    second["entry"][0]["changes"][0]["value"]["statuses"][0]["recipient_participant_id"] =
        "447700900999".into();
    assert_eq!(
        call(
            fixture.app.clone(),
            fixture
                .setup
                .callback(serde_json::to_vec(&second).unwrap(), true)
        )
        .await
        .status(),
        StatusCode::OK
    );
    let page = fixture.page().await;
    let consumer = page.items[0]
        .consumers
        .iter()
        .find(|c| c.consumer_id == fixture.receipt.consumer.as_str())
        .unwrap();
    assert_eq!(consumer.deliveries.len(), 2);
    for delivery in &consumer.deliveries {
        assert_eq!(delivery.recipients[0].kind, "whatsapp_group");
        assert_eq!(delivery.recipients[0].id, fixture.receipt.recipient);
        assert_eq!(delivery.recipients[1].kind, "whatsapp_participant");
    }
    assert_ne!(
        consumer.deliveries[0].recipients[1].id,
        consumer.deliveries[1].recipients[1].id
    );
    let before = fixture.setup.server.memory.writes.load(Ordering::SeqCst);
    second["entry"][0]["changes"][0]["value"]["statuses"][0]["recipient_type"] =
        "individual".into();
    assert_eq!(
        call(
            fixture.app.clone(),
            fixture
                .setup
                .callback(serde_json::to_vec(&second).unwrap(), true)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture.setup.server.memory.writes.load(Ordering::SeqCst),
        before
    );
}

#[tokio::test]
async fn pending_status_in_mixed_batch_does_not_block_or_duplicate_incoming_message() {
    let fixture = StatusSetup::new().await;
    let mut body: Value = serde_json::from_slice(&fixture.body("sent", 0)).unwrap();
    let value = &mut body["entry"][0]["changes"][0]["value"];
    value["statuses"][0]["id"] = "wamid.not-yet-retained".into();
    value["messages"] = json!([current(text("wamid.mixed-incoming", "new message"))]);
    let body = serde_json::to_vec(&body).unwrap();
    let response = call(
        fixture.app.clone(),
        fixture.setup.callback(body.clone(), true),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let plan = fixture
        .setup
        .server
        .store()
        .load(&fixture.setup.identity("wamid.mixed-incoming"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        plan.activity.event_id,
        fixture.setup.identity("wamid.mixed-incoming").event_id
    );
    assert_eq!(fixture.page().await.items.len(), 2);
    let writes = fixture.setup.server.memory.writes.load(Ordering::SeqCst);
    let response = call(fixture.app.clone(), fixture.setup.callback(body, true)).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        fixture.setup.server.memory.writes.load(Ordering::SeqCst),
        writes
    );
    assert_eq!(fixture.page().await.items.len(), 2);
}
