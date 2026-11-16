//! Finite public mutation forwarding, including delegated and media routes.
use super::*;

#[tokio::test]
async fn finite_agent_mutations_route_original_inputs_and_require_receiver_policy() {
    let (store, instances, probes) = replicas().await;
    let id = AgentId("agt-finite-mutations".into());
    let (owner, authority) = acquire(&store, &instances, &id).await;
    let paths = [
        (Method::POST, format!("/api/v1/agents/{id}/artifacts")),
        (Method::POST, format!("/api/v1/agents/{id}/share-links")),
        (
            Method::DELETE,
            format!("/api/v1/agents/{id}/share-links/retained?expected_revision=3"),
        ),
        (
            Method::POST,
            format!("/api/v1/agents/{id}/capacity-recovery/admit"),
        ),
        (
            Method::POST,
            format!("/api/v1/agents/{id}/capacity-recovery/resume"),
        ),
        (Method::POST, format!("/api/v1/resource-shares/agent/{id}")),
        (
            Method::PATCH,
            format!("/api/v1/resource-shares/agent/{id}/retained"),
        ),
        (
            Method::DELETE,
            format!("/api/v1/resource-shares/agent/{id}/retained?expected_revision=4"),
        ),
    ];
    for (method, path) in &paths {
        for ingress in [1 - owner, owner] {
            let body = br#"{"expected_revision":4,"original":"unchanged"}"#;
            let response = client()
                .request(
                    method.clone(),
                    format!("{}{path}", instances[ingress].address),
                )
                .headers(headers())
                .body(body.to_vec())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED, "{method} {path}");
            let accepted: Value = response.json().await.unwrap();
            assert_eq!(accepted["owner"], authority.token.instance_id);
            assert_eq!(accepted["epoch"], authority.token.epoch);
            assert_eq!(accepted["uri"], *path);
            assert_eq!(accepted["body"], json!(body.to_vec()));
            assert_eq!(accepted["idempotency"], "acceptance-operation");
        }
    }
    assert_eq!(probes[1 - owner].effects.load(Ordering::SeqCst), 0);
    assert_eq!(
        probes[owner].effects.load(Ordering::SeqCst),
        paths.len() * 2
    );
    probes[owner].allowed.store(false, Ordering::SeqCst);
    for (method, path) in paths {
        let response = client()
            .request(method, format!("{}{path}", instances[1 - owner].address))
            .headers(headers())
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    assert_eq!(probes[owner].effects.load(Ordering::SeqCst), 16);
}

pub(super) async fn accept_media(
    State(probe): State<Arc<Probe>>,
    body: Bytes,
) -> impl IntoResponse {
    let owner = layer2_core::agent_ownership::execution_owner().expect("media on owner");
    probe.effects.fetch_add(1, Ordering::SeqCst);
    axum::Json(json!({"owner":owner.instance_id,"bytes":body.len()}))
}

#[tokio::test]
async fn delegated_delete_outer_request_preserves_event_body_and_checks_both_peers() {
    let (store, instances, probes) = replicas().await;
    let id = AgentId("agt-delegated-delete".into());
    let (owner, _) = acquire(&store, &instances, &id).await;
    let bytes = serde_json::to_vec(&json!({"method":"DELETE","path":format!("/api/v1/agents/{id}"),"event_ref":"cmaeg1.ev.retained","idempotency_key":"original-delete"})).unwrap();
    let response = client()
        .post(format!(
            "{}/api/v1/agent-delegations/operations",
            instances[1 - owner].address
        ))
        .headers(headers())
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let receipt: Value = response.json().await.unwrap();
    assert_eq!(receipt["body"], json!(bytes));
    assert_eq!(receipt["owner"], instances[owner].coordinator.instance_id);
    assert_eq!(probes[1 - owner].effects.load(Ordering::SeqCst), 0);
    probes[owner].allowed.store(false, Ordering::SeqCst);
    let response = client()
        .post(format!(
            "{}/api/v1/agent-delegations/operations",
            instances[1 - owner].address
        ))
        .headers(headers())
        .body(bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(probes[owner].effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn media_relay_preserves_twenty_mib_body_and_decoded_parent_path_on_both_peers() {
    let (store, instances, probes) = replicas().await;
    let id = AgentId("agt-media-parent".into());
    let (owner, _) = acquire(&store, &instances, &id).await;
    let path = "/api/v1/cmaeg/bindings/binding-test/conversations/conversation%20decoded/events/event-test/media/0/attachment?name=retained.txt";
    let response = client()
        .post(format!("{}{path}", instances[1 - owner].address))
        .headers(headers())
        .body(vec![b'x'; 20 * 1024 * 1024])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let receipt: Value = response.json().await.unwrap();
    assert_eq!(receipt["bytes"], 20 * 1024 * 1024);
    assert_eq!(receipt["owner"], instances[owner].coordinator.instance_id);
    assert_eq!(probes[1 - owner].effects.load(Ordering::SeqCst), 0);
    probes[owner].allowed.store(false, Ordering::SeqCst);
    let response = client()
        .post(format!("{}{path}", instances[1 - owner].address))
        .headers(headers())
        .body(b"small".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(probes[owner].effects.load(Ordering::SeqCst), 1);
}
