use layer3_adapter::agent_capacity_recovery::{CapacityRecoveryRequest, ConfirmedAgentState, RetainedAgentTarget};
use layer3_adapter::AgentSlot;
use layer3_adapter::RuntimeHandle;

use super::*;

fn launch() -> AgentLaunch {
    AgentLaunch {
        operation_id: "agent:test-agent:open:v1".into(),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new("test-agent"),
        cwd: "/tmp".into(),
        home: "/tmp".into(),
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "while IFS= read -r line; do printf '%s\\n' \"$line\"; done".into(),
        ],
        env: BTreeMap::new(),
    }
}

fn recovery_request(challenge: &str, writer_fence: u64) -> CapacityRecoveryRequest {
    CapacityRecoveryRequest {
        operation_key: "agent:test-agent:open:v1".into(),
        target: RetainedAgentTarget {
            sandbox_id: "sandbox".into(),
            agent_id: "test-agent".into(),
            runtime: RuntimeHandle::new("runtime"),
            boot_id: "a".repeat(32),
            provision_generation: 1,
            home_identity: "a".repeat(43),
            conversation_id: "conversation".into(),
            conversation_generation: 1,
            original_open_operation: "agent:test-agent:open:v1".into(),
            original_open_sha256: "b".repeat(64),
            active_turn: None,
        },
        writer_fence,
        challenge: challenge.into(),
    }
}

#[tokio::test]
async fn separate_bridges_have_independent_streams() {
    let first = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    let second = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    let mut first_output = first.subscribe();
    let mut second_output = second.subscribe();
    assert!(first.send(b"one".to_vec()).await);
    assert!(second.send(b"two".to_vec()).await);
    assert_eq!(
        first_output.recv().await.unwrap(),
        Outbound::Frame(b"one".to_vec())
    );
    assert_eq!(
        second_output.recv().await.unwrap(),
        Outbound::Frame(b"two".to_vec())
    );
    first.terminate().await.unwrap();
    second.terminate().await.unwrap();
}

#[tokio::test]
async fn detached_output_is_replayed() {
    let bridge = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    assert!(bridge.send(b"held".to_vec()).await);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let backlog = bridge.backlog().await;
        if !backlog.is_empty() {
            assert_eq!(backlog, vec![Outbound::Frame(b"held".to_vec())]);
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    bridge.terminate().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn primary_exit_remains_observable_without_active_subscribers() {
    let mut launch = launch();
    launch.args = vec!["-c".into(), "exit 0".into()];
    let bridge = Bridge::start(&launch, &BTreeMap::new()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !bridge.is_ended() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("exit state remains observable without a waiting subscriber");
    tokio::time::timeout(Duration::from_millis(100), bridge.ended())
        .await
        .expect("late subscribers observe the retained exit state");
}

#[cfg(unix)]
#[tokio::test]
async fn close_terminates_descendants_after_the_primary_has_exited() {
    let mut launch = launch();
    launch.args = vec!["-c".into(), "sleep 30 & printf '%s\\n' $!".into()];
    let bridge = Bridge::start(&launch, &BTreeMap::new()).unwrap();
    let mut output = bridge.subscribe();
    let descendant = match tokio::time::timeout(Duration::from_secs(2), output.recv())
        .await
        .unwrap()
        .unwrap()
    {
        Outbound::Frame(bytes) => String::from_utf8(bytes).unwrap().parse::<i32>().unwrap(),
        other => panic!("expected descendant PID, got {other:?}"),
    };
    tokio::time::timeout(Duration::from_secs(2), bridge.ended())
        .await
        .expect("bridge primary exited");
    assert_eq!(unsafe { libc::kill(descendant, 0) }, 0);
    bridge.terminate().await.unwrap();
    assert_ne!(unsafe { libc::kill(descendant, 0) }, 0);
}

#[tokio::test]
async fn native_stdout_replay_over_both_windows_preserves_every_original_frame() {
    let mut launch = launch();
    let payload = "x".repeat(1024);
    let count = 16_421_u64;
    launch.args = vec!["-c".into(), format!("i=1; while [ \"$i\" -le {count} ]; do printf '%s:%s\\n' \"$i\" '{payload}'; i=$((i+1)); done")];
    let bridge = Bridge::start(&launch, &BTreeMap::new()).unwrap();
    assert_eq!(bridge.select_output_acknowledgement(true), Ok(true));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut changed = bridge.retained_output.subscribe();
    let mut cursor = 0;
    let mut bytes = 0;
    tokio::time::timeout(Duration::from_secs(20), async {
        while cursor < count {
            let snapshot = bridge.retained_output.snapshot(cursor).await.unwrap();
            if snapshot.frames.is_empty() {
                changed.changed().await.unwrap();
                continue;
            }
            for frame in snapshot.frames {
                cursor += 1;
                assert_eq!(frame.cursor, cursor);
                assert_eq!(
                    frame.bytes.as_ref(),
                    format!("{cursor}:{payload}").as_bytes()
                );
                bytes += frame.bytes.len();
                assert!(
                    bridge
                        .retained_output
                        .acknowledge(bridge.retained_output.stream_id(), cursor)
                        .await
                );
            }
        }
        bridge.ended().await;
    })
    .await
    .expect("native stdout must drain without retention loss");
    assert!(bytes > BUFFER_BYTES);
    bridge.terminate().await.unwrap();
}

#[tokio::test]
async fn retiring_a_detached_bridge_cancels_a_backpressured_stdout_reader() {
    let mut launch = launch();
    launch.args = vec![
        "-c".into(),
        format!("while :; do printf '%s\\n' '{}'; done", "x".repeat(1024)),
    ];
    let bridge = Bridge::start(&launch, &BTreeMap::new()).unwrap();
    assert_eq!(bridge.select_output_acknowledgement(true), Ok(true));
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::timeout(Duration::from_secs(5), bridge.terminate())
        .await
        .unwrap()
        .unwrap();
    let reader = bridge.reader.clone();
    drop(bridge);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !reader.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retired detached bridge must not leave a capacity waiter behind");
}

#[tokio::test]
async fn recovery_lease_is_single_use_and_binds_the_writer_fence() {
    let bridge = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    let request = recovery_request("a".repeat(32).as_str(), 7);
    assert!(bridge
        .reserve_capacity_recovery(&request.challenge, request.writer_fence)
        .await
        .is_ok());
    assert!(matches!(
        bridge
            .reserve_capacity_recovery(&request.challenge, request.writer_fence + 1)
            .await,
        Err(layer3_adapter::agent_capacity_recovery::CapacityRecoveryError::IdentityConflict)
    ));
    assert!(bridge.consume_capacity_recovery(&request.challenge).await);
    assert!(!bridge.consume_capacity_recovery(&request.challenge).await);
    bridge.terminate().await.unwrap();
}

#[tokio::test]
async fn recovery_proof_requires_the_reserved_lease_and_preserves_no_input_path() {
    let bridge = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    let request = recovery_request("c".repeat(32).as_str(), 9);
    assert!(matches!(
        bridge
            .capacity_recovery_proof(&request, "/tmp".into())
            .await,
        Err(layer3_adapter::agent_capacity_recovery::CapacityRecoveryError::IdentityConflict)
    ));
    bridge
        .reserve_capacity_recovery(&request.challenge, request.writer_fence)
        .await
        .unwrap();
    observe_input(&bridge, serde_json::json!({"id":"load", "method":"session/load", "params":{"sessionId":"conversation"}})).await;
    observe_output(&bridge, serde_json::json!({"id":"load", "result":{}})).await;
    bridge.retained_output.retain(b"retained").await;
    assert_eq!(bridge.select_output_acknowledgement(true), Ok(true));
    let proof = bridge
        .capacity_recovery_proof(&request, "/tmp".into())
        .await
        .unwrap();
    assert_eq!(proof.confirmation.request, request);
    assert_eq!(proof.confirmation.output.acknowledged_through, 0);
    assert_eq!(proof.confirmation.output.first_retained, 1);
    assert_eq!(proof.confirmation.output.observed_through, 1);
    bridge.terminate().await.unwrap();
}

async fn observe_input(bridge: &Bridge, frame: serde_json::Value) {
    bridge
        .observation
        .lock()
        .await
        .input(&serde_json::to_vec(&frame).unwrap());
}

async fn observe_output(bridge: &Bridge, frame: serde_json::Value) {
    bridge
        .observation
        .lock()
        .await
        .output(&serde_json::to_vec(&frame).unwrap());
}

#[tokio::test]
async fn recovery_failed_load_cannot_authorize_a_proof() {
    let bridge = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    let request = recovery_request(&"d".repeat(32), 10);
    observe_input(&bridge, serde_json::json!({"id":"load", "method":"session/load", "params":{"sessionId":"conversation"}})).await;
    observe_output(
        &bridge,
        serde_json::json!({"id":"load", "error":{"code":-32603, "message":"load failed"}}),
    )
    .await;
    bridge
        .reserve_capacity_recovery(&request.challenge, request.writer_fence)
        .await
        .unwrap();
    let proof = bridge
        .capacity_recovery_proof(&request, "/tmp".into())
        .await;
    bridge.terminate().await.unwrap();
    assert!(proof.is_err(), "failed load authorized proof: {proof:?}");
}

#[tokio::test]
async fn recovery_unrelated_response_preserves_a_working_prompt() {
    let bridge = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    observe_input(&bridge, serde_json::json!({"id":"load", "method":"session/load", "params":{"sessionId":"conversation"}})).await;
    observe_output(&bridge, serde_json::json!({"id":"load", "result":{}})).await;
    let prompt = layer3_adapter::PromptIdentity::new("conversation", 1, 1);
    observe_input(&bridge, serde_json::json!({"id":prompt.as_str(), "method":"session/prompt", "params":{"sessionId":"conversation"}})).await;
    observe_output(&bridge, serde_json::json!({"id":"initialize", "error":{"code":-32603, "message":"Already initialized"}})).await;
    let state = bridge.observation.lock().await.native_state.clone();
    bridge.terminate().await.unwrap();
    assert_eq!(
        state,
        Some(ConfirmedAgentState::Working {
            prompt_id: prompt.as_str().into()
        })
    );
}
