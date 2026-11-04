use crate::runtime_bootstrap::{RuntimeBootstrapReceipt, RuntimeBootstrapRequest};
use futures_util::{SinkExt, StreamExt};
use std::sync::Mutex;
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        Message,
    },
};

pub(super) async fn serve(socket: tokio::net::TcpStream, state: &Mutex<super::fixture::State>) {
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
