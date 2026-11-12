use super::super::tests::stop;
use super::budget_tests::{open_with_json, retained};
use super::*;
use crate::hosted::retained_json::JsonPools;
use futures_util::StreamExt as _;
use std::time::Duration;

const CAPACITY: usize = 1024 * 1024;

async fn stop_after_final_owner(opened: super::super::OpenedBridge) {
    let lifetime = Arc::downgrade(&opened.bridge);
    stop(opened).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while lifetime.strong_count() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retired attachment releases the writer and context observation");
}

async fn configure(bridge: &Bridge, startup: bool) -> JsonPools {
    let pools = JsonPools::new(CAPACITY, CAPACITY);
    let mut state = bridge.output.as_ref().unwrap().lock().await;
    state.json_pools = Some(pools.clone());
    if startup {
        state.session = None;
    }
    pools
}

fn payload(goal: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"method":"session/update","params":{"sessionId":"conversation-1","update":{"sessionUpdate":"session_info_update","_meta":{"goal":goal}}}})).unwrap()
}

#[tokio::test]
async fn retained_goal_owns_its_frame_after_receipt_then_replacement_and_teardown_release_it() {
    let (opened, mut server, _received) = open_with_json(true, true, true).await;
    let first = configure(&opened.bridge, false).await;
    let (capture, mut effects) = UpdateRelay::capture();
    opened
        .bridge
        .receive_output(&retained(3, &payload(serde_json::Value::Null)), &capture)
        .await
        .unwrap();
    drop(effects.try_recv().unwrap());
    assert!(opened
        .bridge
        .context_goal
        .lock()
        .await
        .as_ref()
        .unwrap()
        .is_safe_for("conversation-1"));
    assert!(first.available().0 < CAPACITY);
    let second = configure(&opened.bridge, false).await;
    opened
        .bridge
        .receive_output(
            &retained(4, &payload(serde_json::json!({"status":"active"}))),
            &capture,
        )
        .await
        .unwrap();
    drop(effects.try_recv().unwrap());
    assert_eq!(first.available(), (CAPACITY, CAPACITY));
    assert!(second.available().0 < CAPACITY);
    opened
        .bridge
        .receive_output(&retained(5, &payload(serde_json::Value::Null)), &capture)
        .await
        .unwrap();
    drop(effects.try_recv().unwrap());
    assert!(!opened
        .bridge
        .context_goal
        .lock()
        .await
        .as_ref()
        .unwrap()
        .is_safe_for("conversation-1"));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), server.next())
            .await
            .is_err(),
        "observing or dropping a receipt cannot fabricate a durable ACK"
    );
    stop_after_final_owner(opened).await;
    assert_eq!(second.available(), (CAPACITY, CAPACITY));
}

#[tokio::test]
async fn startup_observation_transfers_its_existing_owner_after_activation() {
    let (opened, _server, _received) = open_with_json(true, true, true).await;
    let pools = configure(&opened.bridge, true).await;
    let (capture, mut effects) = UpdateRelay::capture();
    opened
        .bridge
        .receive_output(&retained(3, &payload(serde_json::Value::Null)), &capture)
        .await
        .unwrap();
    assert!(effects.try_recv().is_err());
    assert!(opened.bridge.context_goal.lock().await.is_none());
    let held = pools.available();
    opened
        .bridge
        .activate_output("conversation-1", &capture)
        .await
        .unwrap();
    let ConversationUpdate::RuntimeOutput { receipt, .. } = effects.try_recv().unwrap() else {
        panic!("retained receipt expected")
    };
    assert_eq!(receipt.source.cursor, 3);
    drop(receipt);
    assert_eq!(pools.available(), held);
    assert!(opened
        .bridge
        .context_goal
        .lock()
        .await
        .as_ref()
        .unwrap()
        .is_safe_for("conversation-1"));
    stop_after_final_owner(opened).await;
    assert_eq!(pools.available(), (CAPACITY, CAPACITY));
}
