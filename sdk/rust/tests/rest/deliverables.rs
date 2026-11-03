use super::*;

fn item() -> Value {
    json!({
        "id":format!("delivery_{}", "d".repeat(64)),
        "reference":{
            "agent_id":"agent-1", "resource":{"kind":"live_file","path":"result.bin"},
            "name":"result.bin", "media_type":"application/octet-stream", "size_bytes":3,
            "sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        },
        "status":"unchecked",
    })
}

#[tokio::test]
async fn answer_deliverables_preserve_the_requested_answer_and_original_version() {
    let server = Server::new().await;
    server.expect(
        "GET",
        "/agents/agent-1/answers/event:5/deliverables",
        None,
        200,
        Some(
            json!({"agent_id":"agent-1","answer_id":"event:5","turn_sequence":3,"items":[item()]}),
        ),
    );
    let answer = server
        .client
        .agents()
        .answer_deliverables("agent-1", "event:5", None)
        .await
        .unwrap();
    assert_eq!(answer.answer_id, "event:5");
    assert_eq!(answer.items[0].reference.size_bytes, 3);
    assert_eq!(
        answer.items[0].reference.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    server.expect(
        "GET",
        "/agents/agent-1/answers/event:5/deliverables",
        None,
        200,
        Some(
            json!({"agent_id":"agent-1","answer_id":"event:9","turn_sequence":7,"items":[item()]}),
        ),
    );
    assert!(matches!(
        server
            .client
            .agents()
            .answer_deliverables("agent-1", "event:5", None)
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn answer_deliverables_download_exact_bytes_and_recheck_after_buffering() {
    use axum::{Json, Router, extract::State, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let reads = Arc::new(AtomicUsize::new(0));
    let id = item()["id"].as_str().unwrap().to_owned();
    let root = format!("/api/v1/agents/agent-1/answers/event:5/deliverables/{id}");
    let router = Router::new().route(&root, get(|State(reads): State<Arc<AtomicUsize>>| async move {
        reads.fetch_add(1, Ordering::SeqCst);
        Json(json!({"agent_id":"agent-1","answer_id":"event:5","turn_sequence":3,"item":item()}))
    })).route(&format!("{root}/download"), get(|| async {
        ([("content-type", "application/octet-stream")], "abc")
    })).with_state(reads.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = cma_sdk::Client::builder(format!("http://{address}"))
        .unwrap()
        .build()
        .unwrap();
    let expected = serde_json::from_value(item()).unwrap();
    assert_eq!(
        client
            .agents()
            .download_answer_deliverable("agent-1", "event:5", &expected, None)
            .await
            .unwrap(),
        b"abc"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn answer_deliverables_describe_explicit_resources_without_publishing() {
    use axum::{Json, Router, routing::post};
    use base64::Engine as _;
    let reference: DeliverableReference =
        serde_json::from_value(item()["reference"].clone()).unwrap();
    let marker = format!(
        "<!--cma-deliverable:v1:{}-->",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&reference).unwrap())
    );
    let response = json!({"reference":reference,"marker":marker});
    let router = Router::new().route(
        "/api/v1/agents/agent-1/deliverable-references",
        post(move |Json(selected): Json<Value>| {
            let response = response.clone();
            async move {
                assert_eq!(selected, json!({"kind":"live_file","path":"result.bin"}));
                Json(response)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = cma_sdk::Client::builder(format!("http://{address}"))
        .unwrap()
        .build()
        .unwrap();
    let resource = DeliverableResource::LiveFile {
        path: "result.bin".into(),
    };
    let described = client
        .agents()
        .describe_deliverable("agent-1", &resource, None)
        .await
        .unwrap();
    assert_eq!(described.reference.size_bytes, 3);
    assert_eq!(described.marker, marker);
    assert!(matches!(
        client
            .agents()
            .describe_deliverable(
                "agent-1",
                &DeliverableResource::LiveFile {
                    path: "../private".into()
                },
                None
            )
            .await,
        Err(Error::Validation { .. })
    ));
    task.abort();
}

#[tokio::test]
async fn answer_deliverables_reject_corrupt_bytes_revocation_and_response_confusion() {
    use axum::{Json, Router, extract::State, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for mode in ["corrupt", "revoked", "wrong_answer", "oversized", "html"] {
        let reads = Arc::new(AtomicUsize::new(0));
        let id = item()["id"].as_str().unwrap().to_owned();
        let root = format!("/api/v1/agents/agent-1/answers/event:5/deliverables/{id}");
        let router = Router::new().route(&root, get(|State((reads, mode)): State<(Arc<AtomicUsize>, &'static str)>| async move {
            let count = reads.fetch_add(1, Ordering::SeqCst);
            let mut wire_item = item();
            if mode == "revoked" && count > 0 { wire_item["status"] = "denied".into(); }
            Json(json!({"agent_id":"agent-1","answer_id":if mode == "wrong_answer" {"event:9"} else {"event:5"},"turn_sequence":3,"item":wire_item}))
        })).route(&format!("{root}/download"), get(move || async move {
            let bytes = match mode { "oversized" => vec![b'a'; 8193], "corrupt" => b"abd".to_vec(), _ => b"abc".to_vec() };
            ([("content-type", if mode == "html" { "text/html" } else { "application/octet-stream" })], bytes)
        })).with_state((reads, mode));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = cma_sdk::Client::builder(format!("http://{address}"))
            .unwrap()
            .build()
            .unwrap();
        let expected = serde_json::from_value(item()).unwrap();
        assert!(
            matches!(
                client
                    .agents()
                    .download_answer_deliverable("agent-1", "event:5", &expected, None)
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{mode}"
        );
        task.abort();
    }
}
