//! The strict fixture and real Mongo retain the same WhatsApp receipt/fence identities.
use super::*;
use crate::records::RecordClient;
#[path = "whatsapp_retention_mongo_tests.rs"]
mod mongo_tests;

#[tokio::test]
async fn strict_storage_keeps_ordinary_seven_days_and_whatsapp_thirty_days_separate() {
    let fake = crate::tools::fake_cma::FakeCma::start(false).await;
    let records = RecordClient::new(&fake.base_url, "cma-service")
        .unwrap()
        .with_whatsapp_reaction_fixture();
    assert_retention_limits(records).await;
}

async fn assert_retention_limits(records: RecordClient) {
    let ordinary = records.clone().interactions();
    let reaction = records.whatsapp_reaction_records();
    let seven_day_id = cmaeg_core::ids::RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap();
    ordinary
        .put_expiring(
            &seven_day_id,
            "owner",
            b"ordinary",
            None,
            Some(now() + 7 * 86400),
        )
        .await
        .unwrap();
    let expiry = now();
    let id = cmaeg_core::ids::RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap();
    for days in [8, 31] {
        assert!(ordinary
            .put_expiring(&id, "owner", b"ordinary", None, Some(expiry + days * 86400))
            .await
            .is_err());
    }
    assert_eq!(
        reaction
            .put_expiring(&id, "owner", b"reaction", None, Some(expiry + 30 * 86400))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        ordinary
            .get(ordinary.record_url(id.as_str()).unwrap().as_str())
            .await
            .unwrap()
            .blob,
        b"reaction"
    );
    let invalid = cmaeg_core::ids::RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap();
    assert!(reaction
        .put_expiring(
            &invalid,
            "owner",
            b"invalid",
            None,
            Some(expiry + 31 * 86400)
        )
        .await
        .is_err());
    // An ordinary handle cannot renew a thirty-day record using the privileged class.
    assert!(ordinary
        .put_expiring(
            &id,
            "owner",
            b"ordinary",
            Some(1),
            Some(expiry + 30 * 86400)
        )
        .await
        .is_err());
}

#[tokio::test]
async fn legacy_http_never_receives_a_retention_override_or_customer_send() {
    let (mut app, graph) = fixture().await;
    let reference = reference(&app).await;
    let legacy = RecordClient::new(&app.backend.base_url, "cma-service").unwrap();
    assert!(!legacy.whatsapp_reaction_storage_available());
    app.state.resolver = crate::channels::resolver::ChannelResolver::new(
        Arc::new(legacy),
        app.state.keys.clone(),
        Default::default(),
    );
    let before = app.backend.records().len();
    assert!(context(&app).await.received_message.is_none());
    assert!(
        !app.state
            .message_effect_capabilities(app.caller(), &reference)
            .await
            .unwrap()
            .reaction
    );
    let problem = app
        .state
        .set_message_reaction(app.caller(), &reference, "legacy", request("👍"))
        .await
        .unwrap_err();
    assert_eq!(problem.status, 503);
    assert_eq!(
        problem.detail.as_deref(),
        Some("whatsapp_reaction_retention_unsupported")
    );
    assert_eq!(app.backend.records().len(), before);
    assert!(graph.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn legacy_http_can_replay_existing_confirmed_and_unknown_receipts() {
    for uncertain in [false, true] {
        let (mut app, graph) = fixture().await;
        let reference = reference(&app).await;
        graph.uncertain.store(uncertain, Ordering::SeqCst);
        let first = app
            .state
            .set_message_reaction(app.caller(), &reference, "existing", request("👍"))
            .await
            .unwrap();
        let before = app.backend.records().len();
        app.state.resolver = crate::channels::resolver::ChannelResolver::new(
            Arc::new(RecordClient::new(&app.backend.base_url, "cma-service").unwrap()),
            app.state.keys.clone(),
            Default::default(),
        );
        app.restart();
        assert_eq!(
            app.state
                .set_message_reaction(app.caller(), &reference, "existing", request("👍"))
                .await
                .unwrap(),
            first
        );
        assert_eq!(app.backend.records().len(), before);
        assert_eq!(graph.calls.lock().unwrap().len(), 1);
    }
}
