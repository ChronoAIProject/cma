use super::*;
use serde_json::json;

async fn issued() -> (Bridge, CapacityRecoveryRequest) {
    let bridge = Bridge::start_with_recovery(&launch(), &BTreeMap::new(), true).unwrap();
    let request = recovery_request(&"e".repeat(32), 11);
    observe_input(
        &bridge,
        json!({"id":"load","method":"session/load","params":{"sessionId":"conversation"}}),
    )
    .await;
    observe_output(&bridge, json!({"id":"load","result":{}})).await;
    bridge.retained_output.retain(b"retained").await;
    bridge
        .reserve_capacity_recovery(&request.challenge, request.writer_fence)
        .await
        .unwrap();
    let first = bridge
        .capacity_recovery_proof(&request, "/tmp".into())
        .await
        .unwrap();
    assert_eq!(
        bridge
            .capacity_recovery_proof(&request, "/tmp".into())
            .await
            .unwrap(),
        first
    );
    (bridge, request)
}

#[tokio::test]
async fn recovery_snapshot_change_refuses_attachment_and_cannot_reissue_the_challenge() {
    for change in 0..7 {
        let (bridge, request) = issued().await;
        match change {
            0 => {
                let prompt = layer3_adapter::PromptIdentity::new("conversation", 1, 1);
                observe_input(&bridge,json!({"id":prompt.as_str(),"method":"session/prompt","params":{"sessionId":"conversation"}})).await;
            }
            1 => {
                bridge.retained_output.retain(b"new-output").await;
            }
            2 => {
                assert!(
                    bridge
                        .retained_output
                        .acknowledge(bridge.retained_output.stream_id(), 1)
                        .await
                );
            }
            3 => {
                bridge.retained_output.gap().await;
            }
            4 => {
                bridge
                    .observation
                    .as_ref()
                    .unwrap()
                    .lock()
                    .await
                    .pending_output = true;
            }
            5 => {
                bridge.observation.as_ref().unwrap().lock().await.revision = u64::MAX;
            }
            _ => {
                bridge.terminate().await.unwrap();
            }
        }
        assert!(
            !bridge.consume_capacity_recovery(&request.challenge).await,
            "change={change}"
        );
        assert!(
            bridge
                .capacity_recovery_proof(&request, "/tmp".into())
                .await
                .is_err(),
            "change={change}"
        );
        bridge.terminate().await.unwrap();
    }
}

#[tokio::test]
async fn recovery_writer_fences_remain_monotonic_after_consumption_expiry_and_supersession() {
    for disposition in 0..3 {
        let (bridge, request) = issued().await;
        match disposition {
            0 => {
                assert!(bridge.consume_capacity_recovery(&request.challenge).await);
            }
            1 => {
                bridge
                    .capacity_recovery
                    .lock()
                    .await
                    .lease
                    .as_mut()
                    .unwrap()
                    .expires_at = Instant::now();
                assert!(!bridge.consume_capacity_recovery(&request.challenge).await);
            }
            _ => {
                bridge
                    .reserve_capacity_recovery(&"f".repeat(32), request.writer_fence + 1)
                    .await
                    .unwrap();
                assert!(!bridge.consume_capacity_recovery(&request.challenge).await);
            }
        }
        for fence in [0, request.writer_fence - 1, request.writer_fence, u64::MAX] {
            assert!(bridge
                .reserve_capacity_recovery(&"a".repeat(32), fence)
                .await
                .is_err());
        }
        let mut newer = request.clone();
        newer.writer_fence += 2;
        newer.challenge = "b".repeat(32);
        bridge
            .reserve_capacity_recovery(&newer.challenge, newer.writer_fence)
            .await
            .unwrap();
        bridge
            .capacity_recovery_proof(&newer, "/tmp".into())
            .await
            .unwrap();
        assert!(!bridge.consume_capacity_recovery(&request.challenge).await);
        assert!(bridge.consume_capacity_recovery(&newer.challenge).await);
        assert!(!bridge.consume_capacity_recovery(&newer.challenge).await);
        bridge.terminate().await.unwrap();
    }
}

#[tokio::test]
async fn recovery_full_output_window_refuses_promptly_then_rebinds_after_drain() {
    let (bridge, request) = issued().await;
    for _ in 0..2 {
        bridge
            .retained_output
            .retain(&vec![b'x'; MAX_FRAME_BYTES - 128])
            .await;
    }
    let bridge = Arc::new(bridge);
    let publishing = {
        let bridge = bridge.clone();
        tokio::spawn(async move {
            let frame = serde_json::to_vec(&json!({
                "method": "session/update",
                "params": {"sessionId": "conversation", "padding": "x".repeat(4096)}
            }))
            .unwrap();
            publish(
                frame,
                &bridge.output,
                &bridge.pending,
                &bridge.retained_output,
                &bridge.observation,
                None,
            )
            .await;
        })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if bridge
                .observation
                .as_ref()
                .unwrap()
                .lock()
                .await
                .pending_output
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!tokio::time::timeout(
        Duration::from_secs(1),
        bridge.consume_capacity_recovery(&request.challenge)
    )
    .await
    .unwrap());
    assert!(
        bridge
            .retained_output
            .acknowledge(bridge.retained_output.stream_id(), 3)
            .await
    );
    publishing.await.unwrap();
    let mut newer = request;
    newer.writer_fence += 1;
    newer.challenge = "c".repeat(32);
    bridge
        .reserve_capacity_recovery(&newer.challenge, newer.writer_fence)
        .await
        .unwrap();
    bridge
        .capacity_recovery_proof(&newer, "/tmp".into())
        .await
        .unwrap();
    assert!(bridge.consume_capacity_recovery(&newer.challenge).await);
    bridge.terminate().await.unwrap();
}
