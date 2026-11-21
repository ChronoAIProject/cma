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
async fn concurrent_replacement_key_is_fenced_but_later_distinct_results_remain_sendable() {
    let mut slow = Reply::status(200);
    slow.delay = Duration::from_millis(100);
    slow.body = serde_json::json!({"message_id":"outbound","platform_message_id":"123"});
    let mut app = App::new(vec![slow]).await;
    let target = app
        .target(&app.event("alice", "concurrent-keys").await)
        .await;
    let first = app.send(&target, "first-result", "first result");
    let sending = tokio::spawn(async move { first.send().await.unwrap() });
    app.relay.wait(1).await;
    let blocked = receipt(
        app.send(
            &target,
            "replacement-key",
            "even changed text cannot bypass uncertainty",
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(blocked["delivery"]["outcome"], "unknown");
    assert_eq!(
        blocked["delivery"]["reason"],
        "reply_target_delivery_uncertain"
    );
    assert_eq!(
        receipt(sending.await.unwrap()).await["delivery"]["outcome"],
        "confirmed"
    );
    assert_eq!(app.relay.requests.lock().unwrap().len(), 1);
    app.restart().await;
    let distinct = receipt(
        app.send(&target, "second-result", "a separate completed result")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(distinct["delivery"]["outcome"], "confirmed");
    assert_eq!(app.relay.requests.lock().unwrap().len(), 2);
    // The refused occurrence itself remains effect-free under its original key.
    let replay = receipt(
        app.send(
            &target,
            "replacement-key",
            "even changed text cannot bypass uncertainty",
        )
        .send()
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(replay, blocked);
    assert_eq!(app.relay.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn unknown_delivery_and_store_outage_never_blindly_resend() {
    let mut slow = Reply::status(200);
    slow.delay = Duration::from_secs(1);
    let mut app = App::new(vec![slow]).await;
    let event = app.event("alice", "uncertain").await;
    let target = app.target(&event).await;
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
    let reminted = app.target(&event).await;
    // Reminting the same occurrence or replacing its request key cannot resend it.
    let replacement = receipt(
        app.send(&reminted, "replacement", "once")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replacement["delivery"]["outcome"], "unknown");
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
    let replacement = receipt(
        app.send(&target, "replacement", "once")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replacement["delivery"]["outcome"], "unknown");
    assert_eq!(
        replay["delivery"]["reason"],
        "delivery_in_progress_or_uncertain"
    );
    assert_eq!(app.relay.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn durable_target_preserves_thread_choice_and_original_message_after_restart() {
    let mut app = App::new(vec![]).await;
    if let cmaeg_core::channel::Source::NyxidRelay {
        platform, bot_id, ..
    } = &mut app.definition.sources[0]
    {
        *platform = cmaeg_core::channel::Platform::Lark;
        *bot_id = Some("ou_bot".into());
    }
    app.definition.reply.thread_contract = Some(cmaeg_core::thread::ThreadContract {
        version: 1,
        follow_chat_ids: vec!["chat-alice".into()],
    });
    app.update().await;
    let event = app
        .event_with_thread("alice", "thread-original", true)
        .await;
    let target = app.target(&event).await;
    app.restart().await;
    let first = receipt(
        app.send(&target, "thread-answer", "answer")
            .json(&serde_json::json!({"text":"answer", "thread_reply":true}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first["delivery"]["outcome"], "confirmed");
    app.restart().await;
    let replay = receipt(
        app.send(&target, "thread-answer", "answer")
            .json(&serde_json::json!({"text":"answer", "thread_reply":true}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first, replay);
    let requests = app.relay.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["thread_reply"], true);
    assert_eq!(requests[0].body["message_id"], "thread-original");
}
