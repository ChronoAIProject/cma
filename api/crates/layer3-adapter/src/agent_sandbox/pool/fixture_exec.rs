use crate::runtime_bootstrap::{RuntimeBootstrapReceipt, RuntimeBootstrapRequest};
use futures_util::{SinkExt, StreamExt};
use std::sync::Mutex;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        Message,
    },
};

pub(super) enum RelayOutcome {
    EarlyExit,
    ReplacedPod,
    InvalidMarker,
    Status,
    DisconnectAfterWrite,
}

pub(super) async fn serve_relay(
    socket: impl AsyncRead + AsyncWrite + Unpin,
    state: &Mutex<super::fixture::State>,
) {
    use crate::wire::{RequestFrame, ResponseFrame};
    use serde_json::json;

    #[allow(clippy::result_large_err)]
    let mut ws = accept_hdr_async(socket, |request: &Request, mut response: Response| {
        let url = reqwest::Url::parse(&format!("http://fixture{}", request.uri())).unwrap();
        let command: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k == "command")
            .map(|(_, v)| v.into_owned())
            .collect();
        assert_eq!(
            command,
            [
                "/usr/bin/python3",
                "-I",
                "-u",
                "-c",
                crate::runtime_control_relay::SCRIPT
            ]
        );
        response.headers_mut().insert(
            "sec-websocket-protocol",
            "v5.channel.k8s.io".parse().unwrap(),
        );
        Ok(response)
    })
    .await
    .unwrap();
    let outcome = {
        let mut state = state.lock().unwrap();
        state.relay_opens += 1;
        let outcome = state
            .relay_outcomes
            .pop_front()
            .expect("unexpected relay open");
        if matches!(outcome, RelayOutcome::ReplacedPod) {
            state
                .rows
                .get_mut("/api/v1/namespaces/cma-test/pods/warm-member")
                .unwrap()["metadata"]["uid"] = json!("replacement-pod");
        }
        outcome
    };
    if matches!(outcome, RelayOutcome::EarlyExit | RelayOutcome::ReplacedPod) {
        ws.send(Message::Binary(b"\x03{\"status\":\"Failure\"}".to_vec()))
            .await
            .unwrap();
        while let Some(Ok(Message::Binary(bytes))) = ws.next().await {
            assert!(
                !bytes.starts_with(&[0]),
                "failed relay received runtime bytes"
            );
        }
        return;
    }
    let marker = if matches!(outcome, RelayOutcome::InvalidMarker) {
        b"\x01BAD-EXEC-1\n"
    } else {
        b"\x01CMA-EXEC-1\n"
    };
    ws.send(Message::Binary(marker.to_vec())).await.unwrap();
    if matches!(outcome, RelayOutcome::InvalidMarker) {
        while let Some(Ok(Message::Binary(bytes))) = ws.next().await {
            assert!(
                !bytes.starts_with(&[0]),
                "invalid relay received runtime bytes"
            );
        }
        return;
    }
    let mut input = Vec::new();
    let request = loop {
        let Some(Ok(Message::Binary(bytes))) = ws.next().await else {
            panic!("relay did not receive a runtime status request");
        };
        assert_eq!(bytes[0], 0);
        input.extend_from_slice(&bytes[1..]);
        if let Some(index) = input.windows(4).position(|chunk| chunk == b"\r\n\r\n") {
            if let Ok(request) = serde_json::from_slice::<RequestFrame>(&input[index + 4..]) {
                break request;
            }
        }
    };
    let RequestFrame::RuntimeStatus(request) = request else {
        panic!("expected runtime status");
    };
    state.lock().unwrap().relay_requests += 1;
    if matches!(outcome, RelayOutcome::DisconnectAfterWrite) {
        ws.close(None).await.unwrap();
        return;
    }
    let reply = ResponseFrame::RuntimeStatus(
        serde_json::from_value(json!({
            "sandbox_id": request.sandbox_id, "generation": request.generation,
            "home_identity": request.home_identity, "boot_id": "a".repeat(32),
            "installation_ready": false, "credential_ready": false,
            "restore_pending": false, "agent_processes": 0
        }))
        .unwrap(),
    );
    let body = serde_json::to_string(&reply).unwrap();
    let response = format!("\x01HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    ws.send(Message::Binary(response.into_bytes()))
        .await
        .unwrap();
    let _ = ws
        .send(Message::Binary(b"\x03{\"status\":\"Success\"}".to_vec()))
        .await;
}

pub(super) async fn serve(
    socket: impl AsyncRead + AsyncWrite + Unpin,
    state: &Mutex<super::fixture::State>,
) {
    let mut inspection = false;
    #[allow(clippy::result_large_err)]
    let mut ws = accept_hdr_async(socket, |request: &Request, mut response: Response| {
        let url = reqwest::Url::parse(&format!("http://fixture{}", request.uri())).unwrap();
        assert_eq!(
            url.path(),
            "/api/v1/namespaces/cma-test/pods/warm-member/exec"
        );
        let commands: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k == "command")
            .map(|(_, v)| v.into_owned())
            .collect();
        inspection = commands.first().is_some_and(|c| c == "/usr/bin/python3");
        if inspection {
            assert_eq!(commands.len(), 4);
            assert_eq!(&commands[..3], ["/usr/bin/python3", "-I", "-c"]);
            assert_eq!(commands[3], include_str!("../idle_bootstrap.py"));
        } else {
            assert_eq!(
                commands,
                ["/usr/local/bin/layer4-runtime", "--assign-runtime"]
            );
        }
        assert!(!request.uri().to_string().contains("CHRONO_"));
        response.headers_mut().insert(
            "sec-websocket-protocol",
            "v5.channel.k8s.io".parse().unwrap(),
        );
        Ok(response)
    })
    .await
    .unwrap();
    let mut input = Vec::new();
    while let Some(message) = ws.next().await {
        let Message::Binary(bytes) = message.unwrap() else {
            continue;
        };
        if bytes.as_ref() == [255, 0] {
            break;
        }
        assert_eq!(bytes[0], 0);
        input.extend_from_slice(&bytes[1..]);
    }
    let (receipt, fail) = {
        let mut state = state.lock().unwrap();
        let receipt = if inspection {
            assert!(input.is_empty());
            state.inspections += 1;
            state
                .inspection_output
                .clone()
                .expect("explicit inspection fixture")
        } else {
            let request: RuntimeBootstrapRequest = serde_json::from_slice(&input).unwrap();
            let receipt = RuntimeBootstrapReceipt::for_request(&request).unwrap();
            state.assignments.push(request);
            serde_json::to_value(receipt).unwrap()
        };
        for (path, pointer, value) in std::mem::take(&mut state.exec_mutations) {
            *state
                .rows
                .get_mut(&path)
                .unwrap()
                .pointer_mut(&pointer)
                .unwrap() = value;
        }
        (receipt, state.fail_exec)
    };
    let mut stdout = vec![1];
    stdout.extend(serde_json::to_vec(&receipt).unwrap());
    ws.send(Message::Binary(stdout)).await.unwrap();
    let mut status = vec![3];
    status.extend(serde_json::to_vec(&serde_json::json!({"apiVersion":"v1","kind":"Status", "status":if fail {"Failure"} else {"Success"}})).unwrap());
    let _ = ws.send(Message::Binary(status)).await;
    let _ = ws.close(None).await;
}
