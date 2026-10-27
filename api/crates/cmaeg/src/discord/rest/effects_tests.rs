use super::{DiscordRest, RestError};
use axum::{
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use cmaeg_core::{channel::Secret, message_effect::MessageReactionRequest};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub(crate) struct Memory {
    pub message: Value,
    pub media_patch: bool,
    pub media_wrong_response: bool,
    pub bodies: Vec<(String, Vec<u8>)>,
    pub user: Value,
    pub emoji: Value,
    pub guild: Value,
    pub media_channel_status: Option<StatusCode>,
    pub writes: Vec<(Method, String)>,
    pub reject_write: Option<(usize, StatusCode)>,
    pub exhaust_write: Option<usize>,
    pub fail_receipt: Option<Arc<AtomicBool>>,
    pub message_gate: Option<(Arc<tokio::sync::Semaphore>, Arc<tokio::sync::Semaphore>)>,
}
pub(crate) struct Fixture {
    pub api: DiscordRest,
    pub token: Secret,
    pub memory: Arc<Mutex<Memory>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    pub async fn new() -> Self {
        let memory = Arc::new(Mutex::new(Memory {
            message: json!({"id":"800","channel_id":"99","author":{"id":"77"},"content":"updated"}),
            media_patch: false,
            media_wrong_response: false,
            bodies: vec![],
            user: json!({"id":"77","bot":true}),
            emoji: json!({"id":"500","name":"party","available":true}),
            guild: json!({"id":"99","guild_id":"88"}),
            media_channel_status: None,
            writes: vec![],
            reject_write: None,
            exhaust_write: None,
            fail_receipt: None,
            message_gate: None,
        }));
        let app = Router::new().fallback(handle).with_state(memory.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = DiscordRest::fixture(&format!(
            "http://{}/api/v10/",
            listener.local_addr().unwrap()
        ));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            api,
            token: Secret::new("discord-effects-fixture"),
            memory,
            task,
        }
    }
    async fn react(&self, request: Value) -> Result<(), RestError> {
        let request: MessageReactionRequest = serde_json::from_value(request).unwrap();
        self.api
            .set_message_reaction(&self.token, "77", "99", "800", &request)
            .await
    }
}
async fn handle(
    State(memory): State<Arc<Mutex<Memory>>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    assert_eq!(headers["authorization"], "Bot discord-effects-fixture");
    let gate = memory.lock().unwrap().message_gate.clone();
    if method == Method::GET && uri.path() == "/api/v10/channels/99/messages/800" {
        if let Some((entered, release)) = gate {
            entered.add_permits(1);
            release.acquire().await.unwrap().forget();
        }
    }
    let mut memory = memory.lock().unwrap();
    if method == Method::GET {
        return match uri.path() {
            "/api/v10/users/@me" => Json(memory.user.clone()).into_response(),
            "/api/v10/channels/99/messages/800" => Json(memory.message.clone()).into_response(),
            "/api/v10/channels/99" => match memory.media_channel_status {
                Some(status) => status.into_response(),
                None => Json(memory.guild.clone()).into_response(),
            },
            "/api/v10/guilds/88/emojis/500" => Json(memory.emoji.clone()).into_response(),
            _ => StatusCode::NOT_FOUND.into_response(),
        };
    }
    if method == Method::POST || method == Method::PATCH {
        memory.bodies.push((
            headers
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("")
                .into(),
            body.to_vec(),
        ));
        if memory.media_patch && method == Method::PATCH {
            memory.writes.push((method, uri.path().into()));
            if let Some(fail) = &memory.fail_receipt {
                fail.store(true, Ordering::SeqCst);
            }
            if let Some((at, status)) = memory.reject_write {
                if memory.writes.len() == at {
                    return (status, Json(json!({"retry_after":2}))).into_response();
                }
            }
            let patch: Value = if headers
                .get("content-type")
                .is_some_and(|v| v.to_str().unwrap().starts_with("application/json"))
            {
                serde_json::from_slice(&body).unwrap()
            } else {
                let body = String::from_utf8_lossy(&body);
                let field = body
                    .split("name=\"payload_json\"\r\n\r\n")
                    .nth(1)
                    .unwrap()
                    .split("\r\n--")
                    .next()
                    .unwrap();
                serde_json::from_str(field).unwrap()
            };
            if let Some(text) = patch.get("content") {
                memory.message["content"] = text.clone();
            }
            if let Some(files) = patch["attachments"].as_array() {
                let item = &files[0];
                if item["id"] == 0 {
                    memory.message["attachments"] = json!([{"id":"901","filename":item["filename"],"size":4,"content_type":"application/octet-stream","description":item["description"]}]);
                } else {
                    memory.message["attachments"][0]["description"] = item["description"].clone();
                }
            }
            let mut response = memory.message.clone();
            if memory.media_wrong_response {
                response["id"] = "801".into();
            }
            return Json(response).into_response();
        }
        memory.writes.push((method, uri.path().into()));
        return Json(memory.message.clone()).into_response();
    }
    assert!(matches!(method, Method::DELETE | Method::PUT));
    assert!(uri.path().starts_with("/api/v10/channels/99/messages/800"));
    memory.writes.push((method, uri.path().into()));
    let index = memory.writes.len();
    if let Some(fail) = &memory.fail_receipt {
        fail.store(true, Ordering::SeqCst);
    }
    if let Some((at, status)) = memory.reject_write {
        if index == at {
            return (status, Json(json!({"retry_after":2}))).into_response();
        }
    }
    if memory.exhaust_write == Some(index) {
        return (
            StatusCode::NO_CONTENT,
            [
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset-after", "2"),
            ],
        )
            .into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

#[tokio::test]
async fn deletion_checks_exact_author_and_confirms_only_native_204() {
    let f = Fixture::new().await;
    f.api
        .delete_message(&f.token, "77", "99", "800")
        .await
        .unwrap();
    assert_eq!(
        f.memory.lock().unwrap().writes,
        vec![(Method::DELETE, "/api/v10/channels/99/messages/800".into())]
    );
    f.memory.lock().unwrap().message["author"]["id"] = "42".into();
    assert_eq!(
        f.api.delete_message(&f.token, "77", "99", "800").await,
        Err(RestError::Rejected)
    );
    assert_eq!(f.memory.lock().unwrap().writes.len(), 1);
    assert_eq!(
        f.api
            .delete_message(&f.token, "77", "../users", "800")
            .await,
        Err(RestError::Rejected)
    );
}

#[tokio::test]
async fn mismatched_bot_and_message_cannot_mutate() {
    let f = Fixture::new().await;
    f.memory.lock().unwrap().user["id"] = "78".into();
    assert_eq!(
        f.api.delete_message(&f.token, "77", "99", "800").await,
        Err(RestError::Rejected)
    );
    assert!(f.memory.lock().unwrap().writes.is_empty());
    let f = Fixture::new().await;
    f.memory.lock().unwrap().message["channel_id"] = "100".into();
    assert_eq!(
        f.react(json!({"reaction":[{"type":"emoji","emoji":"👍"}]}))
            .await,
        Err(RestError::Rejected)
    );
    assert!(f.memory.lock().unwrap().writes.is_empty());
}

#[tokio::test]
async fn replace_and_clear_remove_only_own_reactions_using_encoded_segments() {
    let f = Fixture::new().await;
    f.memory.lock().unwrap().message["reactions"] = json!([
        {"me":true,"emoji":{"id":null,"name":"👍"}},
        {"me":false,"emoji":{"id":null,"name":"👀"}}
    ]);
    f.react(json!({"reaction":[{"type":"emoji","emoji":"🎉"}]}))
        .await
        .unwrap();
    let calls = f.memory.lock().unwrap().writes.clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0],
        (
            Method::DELETE,
            "/api/v10/channels/99/messages/800/reactions/%F0%9F%91%8D/@me".into()
        )
    );
    assert_eq!(
        calls[1],
        (
            Method::PUT,
            "/api/v10/channels/99/messages/800/reactions/%F0%9F%8E%89/@me".into()
        )
    );
    f.memory.lock().unwrap().writes.clear();
    f.react(json!({"reaction":[]})).await.unwrap();
    assert_eq!(f.memory.lock().unwrap().writes, vec![calls[0].clone()]);
}

#[tokio::test]
async fn unchanged_reaction_is_a_noop_and_custom_emoji_is_resolved_in_exact_guild() {
    let f = Fixture::new().await;
    f.memory.lock().unwrap().message["reactions"] =
        json!([{"me":true,"emoji":{"id":null,"name":"👍"}}]);
    f.react(json!({"reaction":[{"type":"emoji","emoji":"👍"}]}))
        .await
        .unwrap();
    assert!(f.memory.lock().unwrap().writes.is_empty());
    f.memory
        .lock()
        .unwrap()
        .message
        .as_object_mut()
        .unwrap()
        .remove("reactions");
    f.react(json!({"reaction":[{"type":"custom_emoji","custom_emoji_id":"500"}]}))
        .await
        .unwrap();
    assert_eq!(
        f.memory.lock().unwrap().writes,
        vec![(
            Method::PUT,
            "/api/v10/channels/99/messages/800/reactions/party:500/@me".into()
        )]
    );
}

#[tokio::test]
async fn malformed_reaction_state_or_custom_metadata_refuses_before_effects() {
    for state in [
        json!([{"emoji":{"name":"👍"}}]),
        json!([{"me":true,"emoji":{"id":"500","name":"../users"}}]),
        json!([{"me":true,"emoji":{"name":"👍"}},{"me":true,"emoji":{"name":".."}}]),
        json!({}),
    ] {
        let f = Fixture::new().await;
        f.memory.lock().unwrap().message["reactions"] = state;
        assert_eq!(
            f.react(json!({"reaction":[]})).await,
            Err(RestError::Rejected)
        );
        assert!(f.memory.lock().unwrap().writes.is_empty());
    }
    let f = Fixture::new().await;
    f.memory.lock().unwrap().emoji["id"] = "501".into();
    assert_eq!(
        f.react(json!({"reaction":[{"type":"custom_emoji","custom_emoji_id":"500"}]}))
            .await,
        Err(RestError::Rejected)
    );
    assert!(f.memory.lock().unwrap().writes.is_empty());
    assert_eq!(
        f.react(json!({"reaction":[],"is_big":true})).await,
        Err(RestError::Rejected)
    );
}

#[tokio::test]
async fn partial_reaction_mutation_is_unknown_and_never_retried() {
    for failure in [
        StatusCode::BAD_GATEWAY,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::FORBIDDEN,
    ] {
        let f = Fixture::new().await;
        f.memory.lock().unwrap().message["reactions"] =
            json!([{"me":true,"emoji":{"id":null,"name":"👍"}}]);
        f.memory.lock().unwrap().reject_write = Some((2, failure));
        assert_eq!(
            f.react(json!({"reaction":[{"type":"emoji","emoji":"🎉"}]}))
                .await,
            Err(RestError::Unknown)
        );
        assert_eq!(f.memory.lock().unwrap().writes.len(), 2);
    }
}

#[tokio::test]
async fn native_effects_honor_cooldowns_and_ambiguous_delete_outcomes() {
    let f = Fixture::new().await;
    f.memory.lock().unwrap().reject_write = Some((1, StatusCode::BAD_GATEWAY));
    assert_eq!(
        f.api.delete_message(&f.token, "77", "99", "800").await,
        Err(RestError::Unknown)
    );
    assert_eq!(f.memory.lock().unwrap().writes.len(), 1);
    let f = Fixture::new().await;
    f.memory.lock().unwrap().message["reactions"] =
        json!([{"me":true,"emoji":{"id":null,"name":"👍"}}]);
    f.memory.lock().unwrap().exhaust_write = Some(1);
    assert_eq!(
        f.react(json!({"reaction":[{"type":"emoji","emoji":"🎉"}]}))
            .await,
        Err(RestError::Unknown)
    );
    assert_eq!(f.memory.lock().unwrap().writes.len(), 1);
    assert!(matches!(
        f.api.delete_message(&f.token, "77", "99", "800").await,
        Err(RestError::RateLimited(_))
    ));
}

#[path = "received_effects_tests.rs"]
mod received;
