//! Replay an isolated native ACP trace through the authenticated output lane.
use super::*;
use layer3_adapter::owner_protocol::{
    OwnerConfirmation, TransportOwner, OWNER_HEADER, OWNER_PROTOCOL_VERSION,
};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

#[derive(serde::Deserialize)]
struct NativeTrace {
    conversation: String,
    dispatch: String,
    before: Vec<String>,
    marker: String,
    last_payload_sha256: String,
}

#[tokio::test]
#[ignore = "requires the isolated native ACP trace and an evidence output path"]
async fn authenticated_output_ack_retains_the_exact_native_stop_trace_boundary() {
    let trace: NativeTrace = serde_json::from_slice(
        &std::fs::read(std::env::var_os("CMA_NATIVE_STOP_TRACE").unwrap()).unwrap(),
    )
    .unwrap();
    let evidence_path = std::env::var_os("CMA_NATIVE_STOP_ACK_EVIDENCE").unwrap();
    let last = trace.before.last().unwrap().as_bytes();
    assert_eq!(
        format!("{:x}", Sha256::digest(last)),
        trace.last_payload_sha256
    );
    let marker: serde_json::Value = serde_json::from_str(&trace.marker).unwrap();
    assert_eq!(marker["method"], "cma/codex/turn-stop-fence");
    assert_eq!(marker["params"]["threadId"], trace.conversation);
    assert_eq!(marker["params"]["dispatchKey"], trace.dispatch);

    let (root, supervisor) = echo_supervisor("native-stop-trace").await;
    let owner = TransportOwner {
        protocol: OWNER_PROTOCOL_VERSION,
        agent_id: "agent-1".into(),
        instance_id: "trace-owner".into(),
        epoch: 8,
    };
    let challenge = supervisor
        .manager_ownership
        .challenge(owner.clone())
        .await
        .unwrap();
    supervisor
        .manager_ownership
        .confirm(OwnerConfirmation {
            owner: owner.clone(),
            nonce: challenge.nonce,
        })
        .await
        .unwrap();
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let mut request = format!("ws://{address}/runtime/agents/agent-1/conversation?outputAck=1")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(OWNER_HEADER, owner.header().unwrap().parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let ready = next_conversation_message(&mut socket).await;
    let ready: serde_json::Value = serde_json::from_str(&ready.into_text().unwrap()).unwrap();
    let stream =
        uuid::Uuid::parse_str(ready["outputAcknowledgement"]["streamId"].as_str().unwrap())
            .unwrap();
    assert_eq!(
        ready["managerOwnership"]["owner"],
        serde_json::to_value(&owner).unwrap()
    );

    for (index, payload) in trace.before.iter().enumerate() {
        socket
            .send(ClientMessage::Binary(lanes::frame(
                lanes::CONVERSATION,
                payload.as_bytes(),
            )))
            .await
            .unwrap();
        assert_eq!(
            next_conversation_message(&mut socket).await,
            ClientMessage::Binary(lanes::acknowledged_frame(
                stream,
                (index + 1) as u64,
                payload.as_bytes()
            )),
        );
    }
    let final_cursor = trace.before.len() as u64;
    let bridge = supervisor.bridge("agent-1").await.unwrap();
    let prior = bridge.retained_output.recovery_evidence().await.unwrap();
    assert_eq!(prior.boundary.observed_through, final_cursor);
    assert_eq!(prior.boundary.acknowledged_through, 0);
    let mut ack = lanes::acknowledged_frame(stream, final_cursor, b"");
    ack[0] = lanes::OUTPUT_ACKNOWLEDGEMENT;
    socket.send(ClientMessage::Binary(ack)).await.unwrap();

    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = bridge.retained_output.recovery_evidence().await.unwrap();
            if snapshot.boundary.acknowledged_through == final_cursor {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(observed.boundary.stream_id, stream.to_string());
    assert_eq!(observed.boundary.observed_through, final_cursor);
    assert_eq!(
        observed.acknowledged_payload_sha256,
        Some(Sha256::digest(last).into())
    );
    std::fs::write(
        evidence_path,
        serde_json::to_vec(&serde_json::json!({
            "stream_id": stream.to_string(),
            "cursor": final_cursor,
            "native_conversation_id": trace.conversation,
            "payload_sha256": trace.last_payload_sha256,
        }))
        .unwrap(),
    )
    .unwrap();

    socket.close(None).await.unwrap();
    bridge.terminate().await.unwrap();
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(root).unwrap();
}
