use super::*;
use crate::bridge::{publish, Pending};
use crate::manager_ownership::ManagerOwnership;
use layer3_adapter::owner_protocol::{OwnerConfirmation, TransportOwner, OWNER_PROTOCOL_VERSION};
use layer3_adapter::{AgentLaunch, AgentSlot, RuntimeHandle};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc, Mutex};

#[test]
fn conversation_input_cannot_submit_private_stop() {
    assert!(is_untrusted_stop(
        br#"{"method":"cma/codex/stop-exact-turn"}"#
    ));
    assert!(is_untrusted_stop(
        br#"{"method":"cma\u002fcodex/stop-exact-turn"}"#
    ));
    assert!(is_untrusted_stop(
        br#"[{"method":"initialize"},{"method":"cma/codex/stop-exact-turn"}]"#
    ));
    assert!(!is_untrusted_stop(
        br#"{"method":"session/prompt","params":{"text":"cma/codex/stop-exact-turn"}}"#
    ));
}

#[tokio::test]
async fn stale_turn_boundary_uses_the_last_acknowledged_payload_before_the_marker() {
    for initial_frame in [
        None,
        Some(br#"{"method":"session/update","params":{}}"#.as_slice()),
    ] {
        let bridge = fixture_bridge(true);
        let target = active_target(&bridge).await;
        let stream = bridge.retained_output.stream_id();
        let prompt = target.prompt().as_str().to_owned();
        let pending_stop = bridge.stale_turn.as_ref().unwrap();
        let observation = bridge.observation.as_ref().unwrap();
        {
            let mut observation = observation.lock().await;
            observation.conversation_id = Some(target.conversation_id.clone());
            observation.native_state = Some(ConfirmedAgentState::Working {
                prompt_id: prompt.clone(),
            });
        }
        let receipt = "a".repeat(32);
        *pending_stop.lock().await = Some(Stop {
            target: target.clone(),
            prompt: prompt.clone(),
            receipt: receipt.clone(),
            expires_at: Instant::now() + RETAIN_FOR,
            cursor: None,
            discarded: Arc::new(AtomicBool::new(false)),
        });

        let (output, mut frames) = broadcast::channel(4);
        let pending = Arc::new(Mutex::new(Pending::default()));
        if let Some(payload) = initial_frame {
            publish(
                payload.to_vec(),
                &output,
                &pending,
                &bridge.retained_output,
                &bridge.observation,
                Some(pending_stop),
            )
            .await;
            assert_eq!(
                frames.recv().await.unwrap(),
                crate::bridge::Outbound::Frame(payload.to_vec())
            );
        }
        let mut marker = serde_json::json!({"jsonrpc":"2.0","method":FENCE,"params":{
            "version":1, "threadId":target.conversation_id, "turnId":"turn", "listenerIncarnation":"listener",
            "dispatchSequence":1, "dispatchKey":prompt, "receipt":"wrong"
        }});
        publish(
            serde_json::to_vec(&marker).unwrap(),
            &output,
            &pending,
            &bridge.retained_output,
            &bridge.observation,
            Some(pending_stop),
        )
        .await;
        assert_eq!(
            bridge.observe_stale_turn_stop(&target).await,
            Err(StaleTurnHold::Unconfirmed)
        );
        marker["params"]["receipt"] = receipt.into();
        publish(
            serde_json::to_vec(&marker).unwrap(),
            &output,
            &pending,
            &bridge.retained_output,
            &bridge.observation,
            Some(pending_stop),
        )
        .await;
        assert!(frames.try_recv().is_err());

        let expected = if let Some(payload) = initial_frame {
            assert_eq!(
                bridge.observe_stale_turn_stop(&target).await,
                Err(StaleTurnHold::Unconfirmed)
            );
            assert!(bridge.retained_output.acknowledge(stream, 1).await);
            SealedOutput::Through {
                source: RuntimeOutputSource {
                    stream_id: stream.to_string(),
                    cursor: 1,
                    native_conversation_id: target.conversation_id.clone(),
                    payload_sha256: format!("{:x}", Sha256::digest(payload)),
                },
            }
        } else {
            SealedOutput::Empty {
                stream_id: stream.to_string(),
            }
        };
        let seal = bridge.observe_stale_turn_stop(&target).await.unwrap();
        assert!(seal.matches(&target));
        assert_eq!(seal.output, expected);

        bridge.retained_output.gap().await;
        assert_eq!(
            bridge.observe_stale_turn_stop(&target).await,
            Err(StaleTurnHold::Unconfirmed)
        );
        bridge.terminate().await.unwrap();
    }
}

fn fixture_bridge(enabled: bool) -> Bridge {
    let launch = AgentLaunch {
        operation_id: "open".into(),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new("agent"),
        cwd: "/tmp".into(),
        home: "/tmp".into(),
        program: "/bin/cat".into(),
        args: vec![],
        env: BTreeMap::new(),
    };
    let mut environment = BTreeMap::from([
        ("CHRONO_CODEX_EXACT_TURN_STOP_V1".into(), "1".into()),
        ("CHRONO_CODEX_EXACT_TURN_SEAL_V1".into(), "1".into()),
    ]);
    if enabled {
        environment.insert(FLAG.into(), "1".into());
    }
    Bridge::start(&launch, &environment).unwrap()
}

async fn active_target(bridge: &Bridge) -> StaleTurnTarget {
    assert_eq!(bridge.select_output_acknowledgement(true), Ok(true));
    let target = StaleTurnTarget {
        sandbox_id: "sandbox".into(),
        slot: AgentSlot::new("agent"),
        runtime: RuntimeHandle::new("runtime"),
        runtime_boot: "b".repeat(32),
        runtime_generation: 7,
        conversation_id: "conversation".into(),
        conversation_generation: 5,
        command_id: "command".into(),
        operation_sequence: 3,
        turn_sequence: 6,
        writer_epoch: 8,
        stream_id: Some(bridge.retained_output.stream_id().to_string()),
    };
    let mut observed = bridge.observation.as_ref().unwrap().lock().await;
    observed.conversation_id = Some(target.conversation_id.clone());
    observed.native_state = Some(ConfirmedAgentState::Working {
        prompt_id: target.prompt().as_str().into(),
    });
    target
}

struct OwnerFixture(std::path::PathBuf);

impl OwnerFixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("cma-stop-owner-{}", uuid::Uuid::new_v4())))
    }
}

impl Drop for OwnerFixture {
    fn drop(&mut self) {
        if self.0.exists() {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

async fn own(registry: &ManagerOwnership, epoch: u64) -> OwnerSession {
    let owner = TransportOwner {
        protocol: OWNER_PROTOCOL_VERSION,
        agent_id: "agent".into(),
        instance_id: format!("instance-{epoch}"),
        epoch,
    };
    let challenge = registry.challenge(owner.clone()).await.unwrap();
    registry
        .confirm(OwnerConfirmation {
            owner: owner.clone(),
            nonce: challenge.nonce,
        })
        .await
        .unwrap();
    registry.session("agent", Some(owner)).await.unwrap()
}

#[tokio::test]
async fn ordinary_input_reserves_private_stop_with_the_runtime_switch_off() {
    let bridge = fixture_bridge(false);
    let mut events = bridge.subscribe();
    assert!(
        !bridge
            .send(br#"{"method":"cma/codex/stop-exact-turn"}"#.to_vec())
            .await
    );
    let mut ambiguous = br#"{"method":"cma/codex/stop-exact-turn","params":{"label":""#.to_vec();
    ambiguous.push(0xff);
    ambiguous.extend_from_slice(b"\"}}");
    assert!(serde_json::from_slice::<serde_json::Value>(&ambiguous).is_err());
    assert!(!bridge.send(ambiguous).await);
    let prompt = br#"{"method":"session/prompt","id":"fixture","params":{"sessionId":"conversation","prompt":[{"type":"text","text":"cma/codex/stop-exact-turn"}]}}"#.to_vec();
    assert!(bridge.send(prompt.clone()).await);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap(),
        crate::bridge::Outbound::Frame(prompt)
    );
    bridge.terminate().await.unwrap();
}

#[tokio::test]
async fn a_gapped_stream_keeps_the_ordinary_terminal_path() {
    let mut bridge = fixture_bridge(true);
    let target = active_target(&bridge).await;
    let root = OwnerFixture::new();
    let owners = ManagerOwnership::new(root.0.clone());
    let (sender, mut writes) = mpsc::channel(4);
    let _original_input = std::mem::replace(&mut bridge.input, sender);
    bridge.retained_output.gap().await;
    assert_eq!(
        bridge
            .request_stale_turn_stop(&target, own(&owners, target.writer_epoch).await)
            .await,
        Err(StaleTurnHold::Unconfirmed)
    );
    assert!(writes.try_recv().is_err());
    assert!(bridge.stale_turn.as_ref().unwrap().lock().await.is_none());

    let terminal = serde_json::to_vec(&serde_json::json!({
        "jsonrpc":"2.0", "id":target.prompt().as_str(), "result":{"stopReason":"end_turn"}
    }))
    .unwrap();
    let mut events = bridge.subscribe();
    publish(
        terminal.clone(),
        &bridge.output,
        &bridge.pending,
        &bridge.retained_output,
        &bridge.observation,
        bridge.stale_turn.as_deref(),
    )
    .await;
    assert_eq!(
        events.recv().await.unwrap(),
        crate::bridge::Outbound::Frame(terminal)
    );
    bridge.terminate().await.unwrap();
}

#[tokio::test]
async fn a_successor_retries_only_the_stop_discarded_by_the_writer() {
    let mut bridge = fixture_bridge(true);
    let mut target = active_target(&bridge).await;
    let root = OwnerFixture::new();
    let owners = ManagerOwnership::new(root.0.clone());
    let (sender, queued) = mpsc::channel(4);
    let _original_input = std::mem::replace(&mut bridge.input, sender);
    bridge
        .request_stale_turn_stop(&target, own(&owners, target.writer_epoch).await)
        .await
        .unwrap();
    let discarded = bridge
        .stale_turn
        .as_ref()
        .unwrap()
        .lock()
        .await
        .as_ref()
        .unwrap()
        .discarded
        .clone();
    target.writer_epoch += 1;
    let successor = own(&owners, target.writer_epoch).await;
    let (writer, peer) = tokio::io::duplex(4096);
    crate::bridge::input::spawn_writer(writer, queued);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !discarded.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    bridge
        .request_stale_turn_stop(&target, successor.clone())
        .await
        .unwrap();
    bridge
        .request_stale_turn_stop(&target, successor)
        .await
        .unwrap();
    let mut peer = BufReader::new(peer).lines();
    let line = tokio::time::timeout(Duration::from_secs(2), peer.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["method"], STOP);
    let state = bridge.stale_turn.as_ref().unwrap().lock().await;
    assert_eq!(frame["params"]["receipt"], state.as_ref().unwrap().receipt);
    drop(state);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), peer.next_line())
            .await
            .is_err()
    );
    bridge.terminate().await.unwrap();
}
