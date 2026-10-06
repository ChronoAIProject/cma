use super::*;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::any,
    Json, Router,
};
use layer2_core::SubjectId;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Peer {
    calls: Mutex<Vec<(Method, HeaderMap, Value)>>,
    reply: Mutex<Value>,
}
struct Fixture {
    peer: Arc<Peer>,
    client: ChannelConnections,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let peer = Arc::new(Peer::default());
        async fn response(
            State(peer): State<Arc<Peer>>,
            method: Method,
            headers: HeaderMap,
            body: String,
        ) -> axum::response::Response {
            peer.calls.lock().unwrap().push((
                method.clone(),
                headers,
                if body.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_str(&body).unwrap()
                },
            ));
            if method == Method::DELETE {
                StatusCode::NO_CONTENT.into_response()
            } else {
                Json(peer.reply.lock().unwrap().clone()).into_response()
            }
        }
        let app = Router::new()
            .route("/v1/channel-connections", any(response))
            .route("/v1/channel-connections/{id}", any(response))
            .with_state(peer.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = ChannelConnections::new(
            Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            &base,
            "https://gateway.example",
        )
        .unwrap();
        Self { peer, client, task }
    }
    fn reply(&self, value: Value) {
        *self.peer.reply.lock().unwrap() = value;
    }
}
fn owner() -> ResourceOwner {
    ResourceOwner::Organization {
        subject: SubjectId::new("visitor-org"),
    }
}
fn receipt(id: &str) -> Value {
    json!({"connection_ref":id,"status":"pending","setup_url":"https://gateway.example/connect/opaque","label":null,"url":null})
}

#[tokio::test]
async fn connection_client_preserves_owner_bearers_and_durable_request_identity() {
    let f = Fixture::new().await;
    let id = uuid::Uuid::new_v4().to_string();
    f.reply(receipt(&id));
    let created = f
        .client
        .create(
            "user-secret",
            &owner(),
            &id,
            "cmaeg1.ev.original",
            "agent-secret",
            Some("Example"),
        )
        .await
        .unwrap();
    assert_eq!(created.connection_ref, id);
    assert!(created.label.is_none());
    f.client.get("user-secret", &owner(), &id).await.unwrap();
    f.client.cancel("user-secret", &owner(), &id).await.unwrap();
    let calls = f.peer.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    for (_, headers, body) in calls.iter() {
        assert_eq!(headers["authorization"], "Bearer user-secret");
        assert_eq!(headers["x-cmaeg-owner"], "visitor-org");
        assert!(!body.to_string().contains("secret"));
    }
    assert_eq!(
        calls[0].1["x-cmaeg-agent-authorization"],
        "Bearer agent-secret"
    );
    assert_eq!(
        calls[0].2,
        json!({"record_id":id,"event_ref":"cmaeg1.ev.original","suggested_name":"Example"})
    );
    assert!(calls[1].1.get("x-cmaeg-agent-authorization").is_none());
}

#[tokio::test]
async fn connection_client_rejects_cross_request_receipts_and_unsafe_setup_origins() {
    let f = Fixture::new().await;
    let id = uuid::Uuid::new_v4().to_string();
    for field in ["reference", "origin", "userinfo", "final_url"] {
        let mut value = receipt(&id);
        match field {
            "reference" => value["connection_ref"] = uuid::Uuid::new_v4().to_string().into(),
            "origin" => value["setup_url"] = "https://attacker.invalid/connect".into(),
            "userinfo" => {
                value["setup_url"] = "https://token:secret@gateway.example/connect".into()
            }
            _ => value["url"] = "javascript:alert(1)".into(),
        }
        f.reply(value);
        assert!(
            matches!(
                f.client
                    .create("user", &owner(), &id, "cmaeg1.ev.original", "agent", None)
                    .await,
                Err(BotError::Forbidden)
            ),
            "{field}"
        );
        assert!(
            matches!(
                f.client.get("user", &owner(), &id).await,
                Err(BotError::Forbidden)
            ),
            "{field}"
        );
    }
}

#[tokio::test]
async fn connection_inventory_filters_pending_and_rejects_duplicate_receipts() {
    let f = Fixture::new().await;
    let mut ready = receipt("ready-id");
    ready["status"] = "ready".into();
    ready["setup_url"] = Value::Null;
    ready["url"] = "https://channel.example/ready".into();
    f.reply(json!({"items":[receipt("pending-id"),ready.clone()]}));
    let personal = ResourceOwner::Person {
        subject: SubjectId::new("visitor"),
    };
    let result = f.client.list("user", &personal).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].connection_ref, "ready-id");
    assert!(f.peer.calls.lock().unwrap()[0]
        .1
        .get("x-cmaeg-owner")
        .is_none());
    f.reply(json!({"items":[ready.clone(),ready]}));
    assert!(matches!(
        f.client.list("user", &personal).await,
        Err(BotError::Unavailable)
    ));
}
