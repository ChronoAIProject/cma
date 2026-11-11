use super::super::tests::{receive_json, stop};
use super::budget_tests::{open_with_json, retained};
use super::*;
use crate::hosted::retained_json::JsonPools;
use futures_util::StreamExt as _;
use std::time::Duration;

const CAPACITY: usize = 64 * 1024;
const NOTICE: &[u8] = br#"{"id":"sign-in","method":"elicitation/create","params":{"url":"https://example.invalid/login","userCode":"first","code":"fallback","message":"Continue sign-in"}}"#;

async fn configure(bridge: &Bridge, startup: bool) -> Arc<Semaphore> {
    let pool = Arc::new(Semaphore::new(CAPACITY));
    let mut state = bridge.output.as_ref().unwrap().lock().await;
    state.json_pools =
        Some(JsonPools::new(1024 * 1024, 1024 * 1024).with_sign_in_pool(pool.clone()));
    if startup {
        state.session = None;
    }
    pool
}

#[tokio::test]
async fn pressure_precedes_cursor_effect_and_acceptance_then_exact_retry_succeeds() {
    let (opened, mut server, _received) = open_with_json(true, true, true).await;
    let pool = configure(&opened.bridge, false).await;
    let blocker = pool
        .clone()
        .try_acquire_many_owned(CAPACITY as u32)
        .unwrap();
    let (capture, mut effects) = UpdateRelay::capture();
    assert!(matches!(
        opened
            .bridge
            .receive_output(&retained(3, NOTICE), &capture)
            .await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(
        opened
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .last_received,
        Some(2)
    );
    assert!(effects.try_recv().is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(25), server.next())
            .await
            .is_err()
    );
    drop(blocker);
    opened
        .bridge
        .receive_output(&retained(3, NOTICE), &capture)
        .await
        .unwrap();
    let accepted = receive_json(&mut server).await;
    assert_eq!(accepted["result"]["action"], "accept");
    let ConversationUpdate::RuntimeOutput { receipt, update } = effects.try_recv().unwrap() else {
        panic!("retained source expected")
    };
    assert_eq!(receipt.source.cursor, 3);
    let update = update.unwrap();
    assert!(
        matches!(update.effect(), ConversationUpdate::SignInRequired { code, .. } if code.as_deref() == Some("first"))
    );
    let used = pool.available_permits();
    assert!(used < CAPACITY);
    drop(receipt);
    let clone = update.clone();
    drop(update);
    assert_eq!(pool.available_permits(), used);
    drop(clone);
    assert_eq!(pool.available_permits(), CAPACITY);
    stop(opened).await;
}

#[tokio::test]
async fn bootstrap_notice_owns_storage_before_activation_and_has_no_invented_receipt() {
    let (opened, mut server, _received) = open_with_json(true, true, true).await;
    let pool = configure(&opened.bridge, true).await;
    let (capture, mut effects) = UpdateRelay::capture();
    opened
        .bridge
        .receive_output(&retained(3, NOTICE), &capture)
        .await
        .unwrap();
    assert_eq!(
        receive_json(&mut server).await["result"]["action"],
        "accept"
    );
    let notice = effects.try_recv().unwrap();
    assert!(matches!(notice, ConversationUpdate::RetainedEffect(_)));
    assert!(matches!(
        notice.effect(),
        ConversationUpdate::SignInRequired { .. }
    ));
    assert!(notice.turn_evidence().is_none());
    assert!(effects.try_recv().is_err());
    let used = pool.available_permits();
    assert!(used < CAPACITY);
    opened
        .bridge
        .activate_output("conversation-1", &capture)
        .await
        .unwrap();
    let receipt = effects.try_recv().unwrap();
    assert!(
        matches!(&receipt, ConversationUpdate::RuntimeOutput { receipt, update:None } if receipt.source.cursor == 3)
    );
    drop(receipt);
    assert_eq!(
        pool.available_permits(),
        used,
        "the early notice outlives its frame and receipt"
    );
    drop(notice);
    assert_eq!(pool.available_permits(), CAPACITY);
    stop(opened).await;
}

#[tokio::test]
async fn cancelled_delivery_releases_the_notice_without_durable_ack() {
    let (opened, mut server, _received) = open_with_json(true, true, true).await;
    let pool = configure(&opened.bridge, false).await;
    let (output, _receiver) = mpsc::channel(1);
    let (relay, relay_task) = crate::hosted::update_relay::spawn(output, true);
    // One output slot, one relay in-flight value and 32 relay queue slots.
    for _ in 0..34 {
        relay.send(ConversationUpdate::Lost { frames: 1 }).await;
    }
    let bridge = opened.bridge.clone();
    let delivery =
        tokio::spawn(async move { bridge.receive_output(&retained(3, NOTICE), &relay).await });
    assert_eq!(
        receive_json(&mut server).await["result"]["action"],
        "accept"
    );
    assert!(pool.available_permits() < CAPACITY);
    assert!(!delivery.is_finished());
    delivery.abort();
    assert!(delivery.await.unwrap_err().is_cancelled());
    assert_eq!(pool.available_permits(), CAPACITY);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), server.next())
            .await
            .is_err()
    );
    relay_task.abort();
    let _ = relay_task.await;
    stop(opened).await;
}

#[tokio::test]
async fn disabled_policy_keeps_legacy_effect_and_malformed_notice_declines() {
    for bounded in [false, true] {
        let (opened, mut server, _received) = open_with_json(true, bounded, bounded).await;
        let (capture, mut effects) = UpdateRelay::capture();
        opened
            .bridge
            .receive_output(&retained(3, NOTICE), &capture)
            .await
            .unwrap();
        assert_eq!(
            receive_json(&mut server).await["result"]["action"],
            "accept"
        );
        let ConversationUpdate::RuntimeOutput {
            update: Some(update),
            ..
        } = effects.try_recv().unwrap()
        else {
            panic!("effect")
        };
        assert_eq!(
            matches!(&*update, ConversationUpdate::RetainedEffect(_)),
            bounded
        );
        let malformed =
            br#"{"id":"bad-sign-in","method":"elicitation/create","params":{"url":false}}"#;
        opened
            .bridge
            .receive_output(&retained(4, malformed), &capture)
            .await
            .unwrap();
        assert_eq!(
            receive_json(&mut server).await["result"]["action"],
            "decline"
        );
        assert!(matches!(
            effects.try_recv().unwrap(),
            ConversationUpdate::RuntimeOutput { update: None, .. }
        ));
        drop(update);
        stop(opened).await;
    }
}
