use super::{
    fake_nyxid_reply::Reply,
    support::{receipt, App},
};
use serde_json::Value;
use std::{sync::atomic::Ordering, time::Duration};

#[tokio::test]
async fn out_of_order_users_keep_original_destinations_and_replay_after_restart() {
    let mut app = App::new(vec![]).await;
    let alice = app.event("alice", "alice-original").await;
    let bob = app.event("bob", "bob-original").await;
    let alice_target = app.target(&alice).await;
    let bob_target = app.target(&bob).await;
    let bob_receipt = receipt(
        app.send(&bob_target, "completion", "Hello Bob")
            .send()
            .await
            .unwrap(),
    )
    .await;
    let alice_receipt = receipt(
        app.send(&alice_target, "completion", "Hello Alice")
            .send()
            .await
            .unwrap(),
    )
    .await;
    for value in [&bob_receipt, &alice_receipt] {
        assert_eq!(value["delivery"]["outcome"], "confirmed");
    }
    {
        let requests = app.relay.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for (request, original) in requests.iter().zip(["bob-original", "alice-original"]) {
            assert_eq!(request.body["message_id"], original);
            assert_eq!(request.authorization, "Bearer fixture-channel-credential");
        }
    }
    app.restart().await;
    assert_eq!(
        receipt(
            app.send(&alice_target, "completion", "Hello Alice")
                .send()
                .await
                .unwrap()
        )
        .await,
        alice_receipt
    );
    assert_eq!(
        receipt(
            app.send(&bob_target, "completion", "Hello Bob")
                .send()
                .await
                .unwrap()
        )
        .await,
        bob_receipt
    );
    let reminted = app.target(&alice).await;
    assert_ne!(reminted, alice_target);
    assert_eq!(
        receipt(
            app.send(&reminted, "completion", "Hello Alice")
                .send()
                .await
                .unwrap()
        )
        .await,
        alice_receipt
    );
    assert_eq!(
        app.send(&reminted, "completion", "changed")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(app.relay.requests.lock().unwrap().len(), 2);
    let interactions = cmaeg::interactions::InteractionStore::new(
        app.records.clone(),
        app.state.keys.clone(),
        app.state.providers.clone(),
    );
    for (id, record) in app.backend.records() {
        if id == app.definition.record_id.as_str() {
            continue;
        }
        assert!(record.blob.starts_with(b"cmaeg1.in."));
        assert!(!String::from_utf8_lossy(&record.blob).contains("Hello Alice"));
        assert!(
            interactions.load(&id).await.is_err(),
            "a delivery receipt must not authorize a button callback"
        );
    }
}

#[tokio::test]
async fn concurrent_retry_has_one_dispatch_and_eventually_replays_the_durable_receipt() {
    let app = App::new(vec![]).await;
    let target = app.target(&app.event("alice", "concurrent").await).await;
    let (a, b) = tokio::join!(
        app.send(&target, "same", "once").send(),
        app.send(&target, "same", "once").send(),
    );
    let a = receipt(a.unwrap()).await;
    let b = receipt(b.unwrap()).await;
    assert!([&a, &b]
        .iter()
        .any(|r| r["delivery"]["outcome"] == "confirmed"));
    let last = receipt(app.send(&target, "same", "once").send().await.unwrap()).await;
    assert_eq!(last["delivery"]["outcome"], "confirmed");
    assert_eq!(app.relay.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_delivery_and_store_outage_never_blindly_resend() {
    let mut slow = Reply::status(200);
    slow.delay = Duration::from_secs(1);
    let mut app = App::new(vec![slow]).await;
    let target = app.target(&app.event("alice", "uncertain").await).await;
    app.backend.fail_put.store(true, Ordering::SeqCst);
    assert_eq!(
        app.send(&target, "same", "once")
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    assert!(app.relay.requests.lock().unwrap().is_empty());
    app.backend.fail_put.store(false, Ordering::SeqCst);
    let first: Value = receipt(app.send(&target, "same", "once").send().await.unwrap()).await;
    assert_eq!(first["delivery"]["outcome"], "unknown");
    app.restart().await;
    assert_eq!(
        receipt(app.send(&target, "same", "once").send().await.unwrap()).await,
        first
    );
    assert_eq!(app.relay.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_lost_final_receipt_leaves_a_durable_claim_that_cannot_dispatch_again() {
    let mut response = Reply::status(200);
    response.delay = Duration::from_millis(100);
    response.body = serde_json::json!({"message_id":"outbound","platform_message_id":"123"});
    let mut app = App::new(vec![response]).await;
    let target = app.target(&app.event("alice", "lost-receipt").await).await;
    let request = app.send(&target, "same", "once");
    let sending = tokio::spawn(async move { request.send().await.unwrap() });
    app.relay.wait(1).await;
    app.backend.fail_put.store(true, Ordering::SeqCst);
    assert_eq!(sending.await.unwrap().status(), 503);
    app.backend.fail_put.store(false, Ordering::SeqCst);
    app.restart().await;
    let replay = receipt(app.send(&target, "same", "once").send().await.unwrap()).await;
    assert_eq!(replay["delivery"]["outcome"], "unknown");
    assert_eq!(
        replay["delivery"]["reason"],
        "delivery_in_progress_or_uncertain"
    );
    assert_eq!(app.relay.requests.lock().unwrap().len(), 1);
}
