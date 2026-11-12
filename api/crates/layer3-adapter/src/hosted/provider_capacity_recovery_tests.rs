use super::*;
use crate::agent_capacity_recovery::{
    tests::{confirmation, request},
    CapacityRecoveryNativeProof,
};
use tokio::sync::Notify;

struct Pause {
    stage: usize,
    reached: Notify,
    release: Notify,
}

impl Pause {
    async fn at(&self, stage: usize) {
        if self.stage == stage {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }
}

async fn recovery_fixture(
    pause: Arc<Pause>,
    shared: Option<Arc<Mutex<TransportState>>>,
) -> (
    Arc<ProviderHostedControlTransport>,
    tokio::task::JoinHandle<()>,
) {
    let runtime = listener().await;
    let provider = listener().await;
    let address = runtime.local_addr().unwrap();
    let mut transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
            format!("http://{}", provider.local_addr().unwrap()),
            "fixture",
        ))
        .unwrap();
    if let Some(state) = shared { transport.state = state; }
    let transport = Arc::new(transport);
    let task = tokio::spawn(async move {
        let (mut stream, _) = provider.accept().await.unwrap();
        read_request(&mut stream).await;
        pause.at(0).await;
        stream
            .write_all(&json_response(&serde_json::json!({
                "endpoint": format!("{address}/proxy/3001")
            })))
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        let (mut stream, _) = runtime.accept().await.unwrap();
        let proof_request = request_text(read_request(&mut stream).await);
        assert!(
            proof_request.starts_with("POST /proxy/3001/runtime/agents/agent/capacity-recovery ")
        );
        let request: crate::agent_capacity_recovery::CapacityRecoveryRequest =
            serde_json::from_str(proof_request.split_once("\r\n\r\n").unwrap().1).unwrap();
        let mut confirmed = confirmation(&request);
        confirmed.output.stream_id = uuid::Uuid::from_u128(1).to_string();
        confirmed.output.acknowledged_through = 0;
        confirmed.output.first_retained = 1;
        confirmed.output.observed_through = 0;
        pause.at(1).await;
        stream
            .write_all(&json_response(&CapacityRecoveryNativeProof {
                confirmation: confirmed,
                cwd: "/workspace".into(),
            }))
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        let (stream, _) = runtime.accept().await.unwrap();
        pause.at(2).await;
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket
            .send(Message::Text(
                serde_json::json!({
                    "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent",
                    "outputAcknowledgement":{"version":1,"streamId":uuid::Uuid::from_u128(1)}
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let initialize = receive_websocket_json(&mut socket).await;
        assert_eq!(initialize["method"], "initialize");
        pause.at(3).await;
        let mut frame = vec![0x04];
        frame.extend_from_slice(uuid::Uuid::from_u128(1).as_bytes());
        frame.extend_from_slice(&1_u64.to_be_bytes());
        frame.extend(serde_json::to_vec(&serde_json::json!({
            "jsonrpc":"2.0", "id":initialize["id"], "error":{
                "code":-32603, "message":"Internal error", "data":{"details":"Already initialized"}
            }
        })).unwrap());
        socket.send(Message::Binary(frame)).await.unwrap();
        while let Some(frame) = socket.next().await {
            match frame {
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(Message::Binary(frame)) => assert_ne!(
                    frame[0], 0x01,
                    "recovery must not replay input or load another session"
                ),
                _ => {}
            }
        }
    });
    (transport, task)
}

#[tokio::test]
async fn capacity_recovery_success_attaches_without_replaying_input() {
    let pause = Arc::new(Pause {
        stage: usize::MAX,
        reached: Notify::new(),
        release: Notify::new(),
    });
    let (transport, server) = recovery_fixture(pause, None).await;
    let request = request(None);
    let (updates, _received) = tokio::sync::mpsc::channel(8);
    let (channel, proof) = transport
        .reattach_existing_capacity_recovery(&request, updates)
        .await
        .unwrap();
    assert_eq!(
        channel.conversation_id.as_deref(),
        Some(request.target.conversation_id.as_str())
    );
    proof.validate_for(&request).unwrap();
    transport.retire(&request.target.runtime).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn capacity_recovery_fences_every_opening_await_against_close_and_retire() {
    for retire in [false, true] {
        for stage in 0..4 {
            let pause = Arc::new(Pause {
                stage,
                reached: Notify::new(),
                release: Notify::new(),
            });
            let (transport, server) = recovery_fixture(pause.clone(), None).await;
            let request = request(None);
            let (updates, _received) = tokio::sync::mpsc::channel(8);
            let opening = {
                let transport = transport.clone();
                let request = request.clone();
                tokio::spawn(async move {
                    transport
                        .reattach_existing_capacity_recovery(&request, updates)
                        .await
                })
            };
            tokio::time::timeout(Duration::from_secs(5), pause.reached.notified())
                .await
                .unwrap();
            if retire {
                transport.retire(&request.target.runtime).await.unwrap();
            } else {
                transport
                    .close_link(
                        &request.target.runtime,
                        &AgentSlot::new(&request.target.agent_id),
                    )
                    .await;
            }
            let unpaused = Arc::new(Pause { stage: usize::MAX, reached: Notify::new(), release: Notify::new() });
            let (newer, newer_server) = recovery_fixture(unpaused, Some(transport.state.clone())).await;
            let (newer_updates, _newer_received) = tokio::sync::mpsc::channel(8);
            newer.reattach_existing_capacity_recovery(&request, newer_updates).await.unwrap();
            let key = (request.target.runtime.clone(), AgentSlot::new(&request.target.agent_id));
            let newer_id = transport.state.lock().await.links[&key].id;
            pause.release.notify_one();
            let result = opening.await.unwrap();
            let accepted = result.is_ok();
            let retained_id = transport.state.lock().await.links.get(&key).map(|link| link.id);
            transport.retire(&request.target.runtime).await.unwrap();
            server.await.unwrap();
            newer_server.await.unwrap();
            assert_eq!(retained_id, Some(newer_id), "stale recovery replaced a newer link");
            assert!(
                !accepted,
                "retire={retire}, stage={stage}: cancelled recovery installed a link"
            );
            assert!(transport.state.lock().await.links.is_empty());
        }
    }
}
