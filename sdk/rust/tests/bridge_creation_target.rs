#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{Error, IdempotencyKey};
use serde_json::{Value, json};
use wire::WireServer;

fn target() -> Value {
    json!({"bridge_id":"brg-setup", "owner":{"kind":"person","subject":"owner"},
        "callback_url":"https://cma.example/api/v1/bridge-callbacks/brg-setup"})
}
fn expect(server: &WireServer, organization: Option<&str>, response: Value) {
    let path = organization.map_or("/api/v1/bridge-creation-target".into(), |org| {
        format!("/api/v1/bridge-creation-target?organization={org}")
    });
    let mut request = wire::json("GET", &path, None, 200, response, true);
    request
        .headers
        .insert("idempotency-key".into(), "setup-one".into());
    server.expect(request);
}

#[tokio::test]
async fn previews_person_and_organization_with_original_key_and_no_body() {
    let server = WireServer::new().await;
    let _ = (&server.base_url, &server.anonymous);
    let key = IdempotencyKey::new("setup-one").unwrap();
    for org in [None, Some("org-one")] {
        let mut response = target();
        if let Some(subject) = org {
            response["owner"] = json!({"kind":"organization", "subject":subject});
        }
        expect(&server, org, response.clone());
        let preview = server
            .client
            .bridges()
            .creation_target(org, &key)
            .await
            .unwrap();
        assert_eq!(preview.bridge_id, response["bridge_id"]);
        assert_eq!(preview.callback_url, response["callback_url"]);
    }
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn rejects_wrong_scope_and_unsafe_or_mismatched_callback_without_retry() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("setup-one").unwrap();
    let mut responses = vec![];
    for url in [
        "http://cma.example/brg-setup",
        "https://user:password@cma.example/brg-setup",
        "https://cma.example/brg-other",
        "https://cma.example/brg-setup?key=unexpected",
        "https://cma.example/brg-setup#fragment",
    ] {
        let mut response = target();
        response["callback_url"] = json!(url);
        responses.push(response);
    }
    let mut wrong_owner = target();
    wrong_owner["owner"] = json!({"kind":"organization","subject":"org-other"});
    responses.push(wrong_owner);
    for response in &responses {
        expect(&server, None, response.clone());
        assert!(matches!(
            server.client.bridges().creation_target(None, &key).await,
            Err(Error::Protocol { .. })
        ));
    }
    expect(&server, Some("org-one"), target());
    assert!(matches!(
        server
            .client
            .bridges()
            .creation_target(Some("org-one"), &key)
            .await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(server.request_count(), responses.len() + 1);
    server.assert_drained();
}

#[tokio::test]
async fn rejects_invalid_inputs_before_transport() {
    let server = WireServer::new().await;
    let long_key = IdempotencyKey::new("a".repeat(201)).unwrap();
    assert!(matches!(
        server
            .client
            .bridges()
            .creation_target(None, &long_key)
            .await,
        Err(Error::Validation { .. })
    ));
    let key = IdempotencyKey::new("setup-one").unwrap();
    assert!(matches!(
        server
            .client
            .bridges()
            .creation_target(Some(""), &key)
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 0);
}
