use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;
pub struct Recorded {
    pub path: String,
    pub authorization: String,
    pub body: Value,
    pub at: Instant,
}
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
    pub retry_after: String,
    pub delay: Duration,
}
impl Reply {
    pub fn status(status: u16) -> Self {
        Self {
            status,
            body: json!({}),
            retry_after: "1".into(),
            delay: Duration::ZERO,
        }
    }
}
#[derive(Clone)]
struct Data {
    requests: Arc<Mutex<Vec<Recorded>>>,
    replies: Arc<Mutex<VecDeque<Reply>>>,
}
pub struct FakeReply {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    task: JoinHandle<()>,
}
impl Drop for FakeReply {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl FakeReply {
    pub async fn start(replies: Vec<Reply>) -> Self {
        let data = Data {
            requests: Arc::new(Mutex::new(vec![])),
            replies: Arc::new(Mutex::new(replies.into())),
        };
        let requests = data.requests.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/api/v1/channel-relay/reply", post(reply))
            // Proactive sends are answered and recorded like replies.
            .route("/api/v1/channel-relay/send", post(reply))
            .with_state(data);
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            base,
            requests,
            task,
        }
    }
    pub async fn wait(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(25), async {
            while self.requests.lock().unwrap().len() < count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    pub fn texts(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.body["reply"]["text"].as_str().unwrap().to_owned())
            .collect()
    }
}
async fn reply(
    State(data): State<Data>,
    uri: Uri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let count = {
        let mut requests = data.requests.lock().unwrap();
        requests.push(Recorded {
            path: uri.path().into(),
            authorization: headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
            body,
            at: Instant::now(),
        });
        requests.len()
    };
    let next = data.replies.lock().unwrap().pop_front();
    if let Some(next) = next {
        tokio::time::sleep(next.delay).await;
        return (
            StatusCode::from_u16(next.status).unwrap(),
            [("retry-after", next.retry_after)],
            Json(next.body),
        )
            .into_response();
    }
    Json(json!({"message_id":format!("reply-{count}"),"platform_message_id":format!("platform-{count}")})).into_response()
}
