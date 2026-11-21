use super::*;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::any,
    Json, Router,
};
use layer2_core::SubjectId;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Peer {
    calls: Mutex<Vec<(Method, HeaderMap, Value)>>,
    queries: Mutex<Vec<Option<String>>>,
    reply: Mutex<Value>,
    pages: Mutex<BTreeMap<Option<String>, Value>>,
    protocol: Mutex<Option<&'static str>>,
    status: Mutex<Option<StatusCode>>,
}
#[derive(Default, Deserialize)]
struct Parameters {
    after: Option<String>,
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
        let peer = Arc::new(Peer {
            protocol: Mutex::new(Some("1")),
            ..Default::default()
        });
        async fn response(
            State(peer): State<Arc<Peer>>,
            Query(parameters): Query<Parameters>,
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
            peer.queries.lock().unwrap().push(parameters.after.clone());
            let value = peer
                .pages
                .lock()
                .unwrap()
                .get(&parameters.after)
                .cloned()
                .unwrap_or_else(|| peer.reply.lock().unwrap().clone());
            let status = peer
                .status
                .lock()
                .unwrap()
                .unwrap_or(if method == Method::DELETE {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::OK
                });
            let mut response = if status == StatusCode::NO_CONTENT {
                status.into_response()
            } else {
                (status, Json(value)).into_response()
            };
            if let Some(protocol) = *peer.protocol.lock().unwrap() {
                response
                    .headers_mut()
                    .insert("cmaeg-protocol-version", HeaderValue::from_static(protocol));
            }
            response
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
    json!({"connection_ref":id,"status":"pending","setup_url":"https://gateway.example/connect/start/opaque","label":null,"url":null})
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

#[tokio::test]
async fn inventory_follows_empty_pages_and_preserves_each_request_owner() {
    let f = Fixture::new().await;
    // This cursor must remain one opaque query parameter, including separators.
    let cursor = "opaque/cursor?after=foreign&owner=someone-else";
    let mut ready = receipt("second-page-ready");
    ready["status"] = "ready".into();
    *f.peer.pages.lock().unwrap() = BTreeMap::from([
        (None, json!({"items":[],"next_after":cursor})),
        (
            Some(cursor.into()),
            json!({"items":[ready],"next_after":null}),
        ),
    ]);
    let personal = ResourceOwner::Person {
        subject: SubjectId::new("visitor"),
    };
    for (owner, token) in [(owner(), "organization-user"), (personal, "personal-user")] {
        let listed = f.client.list(token, &owner).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].connection_ref, "second-page-ready");
    }
    assert_eq!(
        *f.peer.queries.lock().unwrap(),
        [None, Some(cursor.into()), None, Some(cursor.into())]
    );
    let calls = f.peer.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    for (index, (method, headers, body)) in calls.iter().enumerate() {
        assert_eq!(method, &Method::GET);
        assert!(body.is_null());
        assert!(headers.get("x-cmaeg-agent-authorization").is_none());
        if index < 2 {
            assert_eq!(headers["authorization"], "Bearer organization-user");
            assert_eq!(headers["x-cmaeg-owner"], "visitor-org");
        } else {
            assert_eq!(headers["authorization"], "Bearer personal-user");
            assert!(headers.get("x-cmaeg-owner").is_none());
        }
    }
}

#[tokio::test]
async fn inventory_refuses_repeated_cursors_duplicates_and_invalid_later_pages() {
    for mode in ["cursor", "duplicate", "invalid"] {
        let f = Fixture::new().await;
        let first = receipt("first-page");
        let later = match mode {
            "cursor" => json!({"items":[],"next_after":"same-cursor"}),
            "duplicate" => json!({"items":[first.clone()]}),
            _ => {
                let mut invalid = receipt("different-id");
                invalid["setup_url"] = "https://foreign.example/connect".into();
                json!({"items":[invalid]})
            }
        };
        *f.peer.pages.lock().unwrap() = BTreeMap::from([
            (None, json!({"items":[first],"next_after":"same-cursor"})),
            (Some("same-cursor".into()), later),
        ]);
        let error = f.client.list("user", &owner()).await.unwrap_err();
        assert!(
            matches!(error, BotError::Unavailable | BotError::Forbidden),
            "{mode}"
        );
        assert_eq!(f.peer.calls.lock().unwrap().len(), 2, "{mode}");
    }
}

#[tokio::test]
async fn inventory_caps_the_complete_scan_even_when_every_page_is_empty() {
    let f = Fixture::new().await;
    let mut pages = BTreeMap::from([(None, json!({"items":[],"next_after":"page-1"}))]);
    for page in 1..20 {
        pages.insert(
            Some(format!("page-{page}")),
            json!({"items":[],"next_after":format!("page-{}",page+1)}),
        );
    }
    *f.peer.pages.lock().unwrap() = pages;
    assert!(matches!(
        f.client.list("user", &owner()).await,
        Err(BotError::Unavailable)
    ));
    assert_eq!(f.peer.calls.lock().unwrap().len(), 20);
    f.peer.pages.lock().unwrap().clear();
    f.reply(
        json!({"items":(0..1001).map(|id| receipt(&format!("record-{id}"))).collect::<Vec<_>>()}),
    );
    assert!(matches!(
        f.client.list("user", &owner()).await,
        Err(BotError::Unavailable)
    ));
}

#[tokio::test]
async fn successful_connection_responses_require_the_protocol_version() {
    let f = Fixture::new().await;
    f.reply(json!({"items":[]}));
    for protocol in [None, Some("2")] {
        *f.peer.protocol.lock().unwrap() = protocol;
        assert!(matches!(
            f.client.list("user", &owner()).await,
            Err(BotError::Unavailable)
        ));
        assert!(matches!(
            f.client.cancel("user", &owner(), "connection").await,
            Err(BotError::Unavailable)
        ));
    }
    *f.peer.protocol.lock().unwrap() = Some("1");
    *f.peer.status.lock().unwrap() = Some(StatusCode::INTERNAL_SERVER_ERROR);
    assert!(matches!(
        f.client.list("user", &owner()).await,
        Err(BotError::Unavailable)
    ));
}

#[test]
fn setup_links_allow_only_the_exact_legacy_or_canonical_namespace() {
    let client = ChannelConnections::new(
        Client::new(),
        "https://internal.example",
        "https://gateway.example",
    )
    .unwrap()
    .with_public_api("https://new-bot.example/event-gateway")
    .unwrap()
    .with_public_api_aliases(&["https://bot.example/event-gateway".into()])
    .unwrap();
    for valid in [
        "https://gateway.example/connect/start/cmaeg1.cs.opaque",
        "https://new-bot.example/event-gateway/connect/start/cmaeg1.cs.opaque",
        "https://bot.example/event-gateway/connect/start/cmaeg1.cs.opaque",
    ] {
        let mut connection: Connection = serde_json::from_value(receipt("fixture-id")).unwrap();
        connection.setup_url = Some(valid.into());
        assert!(client.validate(&connection).is_ok(), "{valid}");
    }
    for invalid in [
        "https://foreign.example/event-gateway/connect/start/token",
        "https://bot.example/connect/start/token",
        "https://bot.example/event-gateway/connect/start/",
        "https://bot.example/event-gateway/connect/start/token?redirect=evil",
        "https://bot.example/event-gateway/connect/start/token#fragment",
        "https://gateway.example/connect/start/../token",
    ] {
        let mut connection: Connection = serde_json::from_value(receipt("fixture-id")).unwrap();
        connection.setup_url = Some(invalid.into());
        assert!(client.validate(&connection).is_err(), "{invalid}");
    }
}
