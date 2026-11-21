use super::*;
use crate::turns::routed::test_support::{supported, Setup};
use axum::{
    body::Bytes as HttpBytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
type Seen = Arc<Mutex<Vec<(Method, Uri, HeaderMap, Vec<u8>)>>>;
async fn serve(
    State(seen): State<Seen>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: HttpBytes,
) -> (StatusCode, Json<Value>) {
    seen.lock()
        .unwrap()
        .push((method.clone(), uri.clone(), headers, body.to_vec()));
    if uri.path().ends_with("/supplement") {
        (
            StatusCode::ACCEPTED,
            Json(
                json!({"command_id":"cmd_supplement","response_id":"resp_supplement","response_url":"/api/v2/responses/resp_supplement","status":"queued","queue_deadline":"2030-01-01T00:00:00Z","metadata":{"cma_parent_command_id":"cmd_parent","cma_parent_response_id":"resp_parent","cma_parent_fingerprint":"fingerprint_parent","cma_artifact_id":"0:original","cma_command_fingerprint":"fingerprint_command"}}),
            ),
        )
    } else {
        (
            if method == Method::GET {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            },
            Json(
                json!({"id":"att_fixture","agent_id":"agt_existing","name":"original.txt","path":"/home/attachments/agt_existing/att_fixture/original.txt","size_bytes":4,"media_type":"text/plain","sha256":sha256(b"file")}),
            ),
        )
    }
}
fn original() -> MediaArtifactClaim {
    MediaArtifactClaim {
        attachment_index: 0,
        kind: MediaContinuationKind::Original,
        content_sha256: sha256(b"file"),
        source_sha256: sha256(b"file"),
        name: "original.txt".into(),
        media_type: "text/plain".into(),
        size_bytes: 4,
        text_sha256: None,
    }
}
#[tokio::test]
async fn exact_media_transport_uses_current_consumer_key_and_checks_file_receipts() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(serve).with_state(seen.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let registry=super::super::ProviderRegistry::parse(&json!([{"slug":"cma_codex","base_url":origin,"kind":"responses_http","max_inflight":2,"paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"}}]).to_string()).unwrap();
    let mut client =
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 2).unwrap();
    client.management_auth = Some(
        super::super::management_auth::CmaManagementAuth::new(
            &origin,
            Secret::new("service-fixture-not-used"),
        )
        .unwrap(),
    );
    let setup = Setup::new(supported()).await;
    let mut target = setup.target().clone();
    target.launch.provider.target = cmaeg_core::channel::ProviderTarget::ExistingAgent {
        agent_id: "agt_existing".into(),
    };
    let credential = Secret::new("consumer-fixture-current");
    let artifact = original();
    let uploaded = client
        .upload_event_media(
            &target,
            None,
            &artifact,
            bytes::Bytes::from_static(b"file"),
            &credential,
        )
        .await
        .unwrap();
    assert_eq!(uploaded.agent_id, "agt_existing");
    assert!(client
        .event_media_upload_receipt(&target, &artifact, &credential)
        .await
        .unwrap()
        .is_some());
    let mut supplement = artifact.clone();
    supplement.text_sha256 = Some(sha256(
        cmaeg_core::media_continuation::supplement_text(&artifact, &uploaded.path, "")
            .unwrap()
            .as_bytes(),
    ));
    let accepted = client
        .supplement_event_media(&target, None, &supplement, "", &credential)
        .await
        .unwrap();
    assert_eq!(accepted.response_id, "resp_supplement");
    assert_eq!(accepted.metadata["cma_parent_response_id"], "resp_parent");
    {
        let observed = seen.lock().unwrap();
        assert_eq!(observed.len(), 3);
        for (_, _, headers, _) in observed.iter() {
            assert_eq!(headers["authorization"], "Bearer consumer-fixture-current");
        }
        assert_eq!(observed[0].3, b"file");
        assert_eq!(observed[0].2["content-length"], "4");
        assert!(observed[0].1.query().unwrap().contains("sha256="));
        assert!(observed[1].1.query().is_none());
    }
    let mut wrong = target.clone();
    wrong.launch.provider.target = cmaeg_core::channel::ProviderTarget::ExistingAgent {
        agent_id: "agt_other".into(),
    };
    assert!(client
        .event_media_upload_receipt(&wrong, &artifact, &credential)
        .await
        .is_err());
    client.management_auth = Some(
        super::super::management_auth::CmaManagementAuth::new(
            "https://other.example",
            Secret::new("service-fixture"),
        )
        .unwrap(),
    );
    let before = seen.lock().unwrap().len();
    assert!(client
        .event_media_upload_receipt(&target, &artifact, &credential)
        .await
        .is_err());
    assert_eq!(seen.lock().unwrap().len(), before);
    task.abort();
}
