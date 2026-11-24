use super::*;
use crate::bridge::{publish, Pending};
use layer3_adapter::{AgentLaunch, AgentSlot, RuntimeHandle};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

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
        let bridge = Bridge::start(&launch, &BTreeMap::from([(FLAG.into(), "1".into())])).unwrap();
        assert_eq!(bridge.select_output_acknowledgement(true), Ok(true));
        let stream = bridge.retained_output.stream_id();
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
            stream_id: Some(stream.to_string()),
        };
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
