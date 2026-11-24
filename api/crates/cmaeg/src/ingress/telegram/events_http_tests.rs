use super::*;
use crate::ingress::routing_store::EventIdentity;

pub(super) struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    pub(super) async fn new(f: &Fixture) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/v1/telegram/{}",
            listener.local_addr().unwrap(),
            f.channel.id
        );
        let router = crate::http::telegram::router(f.state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { url, task }
    }
    pub(super) async fn send(&self, raw: &Value, secret: &str) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(&self.url)
            .header("x-telegram-bot-api-secret-token", secret)
            .json(raw)
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
}

#[tokio::test]
async fn telegram_message_events_http_index_failure_requests_retry_before_acknowledging_original() {
    let f = Fixture::new(false).await;
    let server = Server::new(&f).await;
    assert_eq!(server.send(&f.edit(), "wrong-secret").await.0, 401);
    assert!(f.server.memory.rows.lock().unwrap().is_empty());
    let context = crate::connections::store::index_id(
        "telegram-message-context-v1",
        &[f.channel.definition.record_id.as_str(), "77", "42", "5"],
    );
    f.server.fault(context, false, false);
    let (status, body) = server.send(&f.raw, "fixture-webhook").await;
    assert_eq!(status, 503, "{body}");
    let original = f.plan("telegram:77:101").await.unwrap();
    let (status, body) = server.send(&f.edit(), "fixture-webhook").await;
    assert_eq!(status, 202);
    assert_eq!(
        body,
        json!({"status":"refused","reason":"message_context_unavailable"})
    );
    // Telegram retries because the first request failed. The committed admission
    // remains identical while this retry durably repairs its required locator.
    assert_eq!(server.send(&f.raw, "fixture-webhook").await.0, 202);
    assert_eq!(f.plan("telegram:77:101").await.unwrap(), original);
    let (status, body) = server.send(&f.edit(), "fixture-webhook").await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"], "accepted");
    assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn telegram_message_events_retry_uncommitted_admission_and_recover_exactly_after_http_restart(
) {
    let f = Fixture::new(true).await;
    let server = Server::new(&f).await;
    assert_eq!(server.send(&f.raw, "fixture-webhook").await.0, 202);
    let identity = EventIdentity {
        owner: f.channel.definition.owner.subject.clone(),
        channel_id: f.channel.id.clone(),
        record_id: f.channel.definition.record_id.clone(),
        event_id: EventId::new("telegram:77:103").unwrap(),
    };
    f.server.fault(identity.admission_id(), false, false);
    assert_eq!(server.send(&f.reaction(), "fixture-webhook").await.0, 503);
    assert!(f.plan("telegram:77:103").await.is_none());
    drop(server);
    let restored = Server::new(&f).await;
    let (status, body) = restored.send(&f.reaction(), "fixture-webhook").await;
    assert_eq!(status, 202, "{body}");
    let plan = f.plan("telegram:77:103").await.unwrap();
    let before = f.server.memory.writes.load(Ordering::SeqCst);
    assert_eq!(
        restored.send(&f.reaction(), "fixture-webhook").await,
        (status, body)
    );
    assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), before);
    assert_eq!(f.plan("telegram:77:103").await.unwrap(), plan);
}

#[tokio::test]
async fn telegram_message_events_opt_in_is_required_and_missing_durable_runtime_never_falls_back() {
    let mut f = Fixture::new(false).await;
    if let Source::Telegram { message_events, .. } =
        &mut Arc::make_mut(&mut f.channel).definition.sources[0]
    {
        *message_events = false;
    }
    f.publish_channel();
    let server = Server::new(&f).await;
    assert_eq!(server.send(&f.raw, "fixture-webhook").await.0, 202);
    assert_eq!(f.legacy.0.load(Ordering::SeqCst), 1);
    assert!(f.server.memory.rows.lock().unwrap().is_empty());
    assert_eq!(
        server.send(&f.edit(), "fixture-webhook").await.1["reason"],
        "message_context_unavailable"
    );
    assert!(f.server.memory.rows.lock().unwrap().is_empty());
    drop(server);
    let mut f = Fixture::new(false).await;
    f.state.routed = None;
    let server = Server::new(&f).await;
    assert_eq!(server.send(&f.raw, "fixture-webhook").await.0, 503);
    assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn telegram_message_events_locator_conflicts_require_exact_identity_revision_and_version() {
    for field in ["original", "revision", "version"] {
        let f = Fixture::new(false).await;
        let server = Server::new(&f).await;
        assert_eq!(server.send(&f.raw, "fixture-webhook").await.0, 202);
        let context = crate::connections::store::index_id(
            "telegram-message-context-v1",
            &[f.channel.definition.record_id.as_str(), "77", "42", "5"],
        );
        if field == "version" {
            f.server
                .memory
                .rows
                .lock()
                .unwrap()
                .get_mut(&context)
                .unwrap()
                .version = 2;
        } else {
            f.server.change(&context, |value| {
                if field == "original" {
                    value["original"]["event_id"] = "telegram:77:99".into();
                } else {
                    value["revision"] = "different-source-revision".into();
                }
            });
        }
        let retained = f.server.memory.rows.lock().unwrap()[&context].blob.clone();
        let (status, body) = server.send(&f.raw, "fixture-webhook").await;
        assert_eq!(status, 503, "{field}: {body}");
        assert_eq!(
            f.server.memory.rows.lock().unwrap()[&context].blob,
            retained
        );
        assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn telegram_anonymous_counts_authenticate_before_lookup_and_recover_without_duplicate_turns()
{
    let f = Fixture::new(true).await;
    let raw = super::anonymous_tests::aggregate(&f);
    let server = Server::new(&f).await;
    assert_eq!(server.send(&raw, "wrong-secret").await.0, 401);
    assert!(f.server.memory.rows.lock().unwrap().is_empty());
    assert_eq!(server.send(&f.raw, "fixture-webhook").await.0, 202);
    let (status, body) = server.send(&raw, "fixture-webhook").await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"], "accepted");
    let plan = f.plan("telegram:77:301").await.unwrap();
    let before = f.server.memory.writes.load(Ordering::SeqCst);
    drop(server);
    let restored = Server::new(&f).await;
    assert_eq!(restored.send(&raw, "fixture-webhook").await, (status, body));
    assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), before);
    assert_eq!(f.plan("telegram:77:301").await.unwrap(), plan);
    assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_channel_posts_authenticate_admit_durably_and_refuse_human_allowlists() {
    for mode in 0..4 {
        let mut f = super::channel_post_tests::channel_fixture().await;
        let Source::Telegram {
            channel_posts,
            admission,
            ..
        } = &mut Arc::make_mut(&mut f.channel).definition.sources[0]
        else {
            panic!()
        };
        match mode {
            1 => *channel_posts = false,
            2 => *admission = serde_json::from_value(json!({"type":"allowlist","sender_ids":["telegram:chat:-100","42"]})).unwrap(),
            3 => *admission = serde_json::from_value(json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"allowlist","ids":["-101"]},"groups":"all"})).unwrap(),
            _ => {}
        }
        f.publish_channel();
        let server = Server::new(&f).await;
        assert_eq!(server.send(&f.raw, "wrong-secret").await.0, 401);
        assert!(f.plan("telegram:77:101").await.is_none());
        let (status, body) = server.send(&f.raw, "fixture-webhook").await;
        assert_eq!(status, 202, "{body}");
        if mode == 0 {
            assert_eq!(body["status"], "accepted");
            let original = f.plan("telegram:77:101").await.unwrap();
            assert_eq!(original.activity.actor.kind, ActorKind::Unknown);
            let (_, body) = server
                .send(&super::channel_post_tests::edit(&f), "fixture-webhook")
                .await;
            assert_eq!(body["status"], "accepted");
            assert!(f.plan("telegram:77:102").await.is_some());
        } else {
            assert_eq!(body["status"], "refused");
            assert!(f.plan("telegram:77:101").await.is_none());
            assert!(f.server.memory.rows.lock().unwrap().is_empty());
        }
        assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
    }
}
