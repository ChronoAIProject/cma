use super::super::tests::stop;
use super::budget_tests::{open_with_json, retained};
use super::*;
use crate::hosted::retained_json::JsonPools;
use futures_util::StreamExt as _;
use std::time::Duration;

const CAPACITY: usize = 64 * 1024;
const METADATA: &[u8] = br#"{"method":"session/update","params":{"sessionId":"conversation-1","update":{"sessionUpdate":"session_info_update","_meta":{"cma":{"runtimeMetadata":{"description":"title","descriptionInput":"exact\ninput","descriptionInputPosition":"first"}}}}}}"#;

async fn configure(bridge: &Bridge, pool: Arc<Semaphore>, startup: bool) {
    let mut state = bridge.output.as_ref().unwrap().lock().await;
    state.json_pools = Some(JsonPools::new(1024 * 1024, 1024 * 1024).with_observation_pool(pool));
    if startup {
        state.session = None;
    }
}

fn effect(update: ConversationUpdate) -> crate::RuntimeOutputUpdate {
    let ConversationUpdate::RuntimeOutput { receipt, update } = update else {
        panic!("retained source expected")
    };
    assert_eq!(receipt.source.cursor, 3);
    let update = update.unwrap();
    assert!(
        matches!(update.effect(), ConversationUpdate::Metadata(metadata)
        if metadata.description.as_deref() == Some("title")
        && metadata.description_input == Some(crate::ConversationDescriptionInput::First("exact\ninput".into())))
    );
    assert!(update.turn_evidence().is_none());
    update
}

#[tokio::test]
async fn pressure_precedes_cursor_effect_and_ack_then_exact_retry_succeeds() {
    // General shared-output is OFF; JSON retention independently owns the effect.
    let (opened, mut server, _received) = open_with_json(true, false, true).await;
    let pool = Arc::new(Semaphore::new(CAPACITY));
    configure(&opened.bridge, pool.clone(), false).await;
    let blocker = pool
        .clone()
        .try_acquire_many_owned(CAPACITY as u32)
        .unwrap();
    let (capture, mut effects) = UpdateRelay::capture();
    assert!(matches!(
        opened
            .bridge
            .receive_output(&retained(3, METADATA), &capture)
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
        .receive_output(&retained(3, METADATA), &capture)
        .await
        .unwrap();
    let update = effect(effects.try_recv().unwrap());
    assert!(update.is_shared());
    let used = pool.available_permits();
    assert!(used < CAPACITY);
    let clone = update.clone();
    drop(update);
    assert_eq!(pool.available_permits(), used);
    drop(clone);
    assert_eq!(pool.available_permits(), CAPACITY);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), server.next())
            .await
            .is_err()
    );
    stop(opened).await;
}

#[tokio::test]
async fn startup_keeps_the_same_owner_until_activation_or_final_teardown() {
    for activate in [false, true] {
        let (opened, _server, _received) = open_with_json(true, true, true).await;
        let pool = Arc::new(Semaphore::new(CAPACITY));
        configure(&opened.bridge, pool.clone(), true).await;
        let (capture, mut effects) = UpdateRelay::capture();
        opened
            .bridge
            .receive_output(&retained(3, METADATA), &capture)
            .await
            .unwrap();
        assert!(effects.try_recv().is_err());
        let held = pool.available_permits();
        assert!(held < CAPACITY);
        let owner_address = {
            let state = opened.bridge.output.as_ref().unwrap().lock().await;
            &**state.queued.front().unwrap().observation.as_ref().unwrap()
                as *const ConversationUpdate as usize
        };
        if activate {
            opened
                .bridge
                .activate_output("conversation-1", &capture)
                .await
                .unwrap();
            let update = effect(effects.try_recv().unwrap());
            assert_eq!(
                &*update as *const ConversationUpdate as usize,
                owner_address
            );
            assert_eq!(pool.available_permits(), held);
            drop(update);
        }
        let lifetime = Arc::downgrade(&opened.bridge);
        stop(opened).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while lifetime.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("final bridge owner releases startup storage");
        assert_eq!(pool.available_permits(), CAPACITY);
    }
}

#[tokio::test]
async fn cancelled_blocked_delivery_releases_observation_without_durable_ack() {
    let (opened, mut server, _received) = open_with_json(true, true, true).await;
    let pool = Arc::new(Semaphore::new(CAPACITY));
    configure(&opened.bridge, pool.clone(), false).await;
    let (output, _receiver) = mpsc::channel(1);
    let (relay, relay_task) = crate::hosted::update_relay::spawn(output, true);
    for _ in 0..34 {
        relay.send(ConversationUpdate::Lost { frames: 1 }).await;
    }
    let bridge = opened.bridge.clone();
    let delivery =
        tokio::spawn(async move { bridge.receive_output(&retained(3, METADATA), &relay).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        // The output lock is held through blocked relay delivery.
        loop {
            if pool.available_permits() < CAPACITY
                && opened.bridge.output.as_ref().unwrap().try_lock().is_err()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
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
async fn two_attachments_share_one_observation_limit_and_release_for_original_cursor_retry() {
    let (first, _server1, _received1) = open_with_json(true, true, true).await;
    let (second, _server2, _received2) = open_with_json(true, true, true).await;
    let bytes =
        crate::RuntimeOutputUpdate::ACCOUNTED_STORAGE_BYTES + "title".len() + "exact\ninput".len();
    let pool = Arc::new(Semaphore::new(bytes));
    configure(&first.bridge, pool.clone(), false).await;
    configure(&second.bridge, pool.clone(), false).await;
    let (capture, mut effects) = UpdateRelay::capture();
    first
        .bridge
        .receive_output(&retained(3, METADATA), &capture)
        .await
        .unwrap();
    let update = effect(effects.try_recv().unwrap());
    assert_eq!(pool.available_permits(), 0);
    assert!(matches!(
        second
            .bridge
            .receive_output(&retained(3, METADATA), &capture)
            .await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(
        second
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
    drop(update);
    second
        .bridge
        .receive_output(&retained(3, METADATA), &capture)
        .await
        .unwrap();
    drop(effect(effects.try_recv().unwrap()));
    assert_eq!(pool.available_permits(), bytes);
    stop(first).await;
    stop(second).await;
}

#[tokio::test]
async fn disabled_behavior_and_enabled_foreign_or_malformed_frames_preserve_effects() {
    for bounded in [false, true] {
        let (opened, _server, _received) = open_with_json(true, false, bounded).await;
        let pool = Arc::new(Semaphore::new(CAPACITY));
        if bounded {
            configure(&opened.bridge, pool.clone(), false).await;
        }
        let (capture, mut effects) = UpdateRelay::capture();
        opened
            .bridge
            .receive_output(&retained(3, METADATA), &capture)
            .await
            .unwrap();
        let update = effect(effects.try_recv().unwrap());
        assert_eq!(update.is_shared(), bounded);
        drop(update);
        let mut foreign: serde_json::Value = serde_json::from_slice(METADATA).unwrap();
        foreign["params"]["sessionId"] = serde_json::json!("other");
        assert!(matches!(
            opened
                .bridge
                .receive_output(
                    &retained(4, &serde_json::to_vec(&foreign).unwrap()),
                    &capture
                )
                .await,
            Err(AdapterError::Rejected(_))
        ));
        assert!(effects.try_recv().is_err());
        assert_eq!(pool.available_permits(), CAPACITY);
        let malformed = br#"{"method":"session/update","params":{"sessionId":"conversation-1","update":{"sessionUpdate":"unknown","_meta":{"cma":{"runtimeMetadata":{"description":"ignored"}}}}}}"#;
        opened
            .bridge
            .receive_output(&retained(5, malformed), &capture)
            .await
            .unwrap();
        assert!(matches!(
            effects.try_recv().unwrap(),
            ConversationUpdate::RuntimeOutput { update: None, .. }
        ));
        assert_eq!(pool.available_permits(), CAPACITY);
        stop(opened).await;
    }
}
