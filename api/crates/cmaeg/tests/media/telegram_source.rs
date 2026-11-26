//! Native Telegram files through a fake Bot API.
use super::support::{
    activity, assert_absent, capture_logs, event, far, isolated, request, resolved, serve,
    TRACE_ENV,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use cmaeg::{
    channels::resolver::ResolvedChannel,
    media::{
        source::{telegram::TelegramFileSource, MediaSource},
        FetchError,
    },
    telegram::{FileError, TelegramClient},
};
use cmaeg_core::{
    activity::{Activity, AttachmentSource, SourceKind},
    channel::{AdmissionPolicy, Platform, Secret, Source},
    seal::{EventRefPayload, ReplyAddress},
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicU16, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::Instant;

const TOKEN: &str = "77:fixture-telegram-token";
const BOT_ID: &str = "77";
const FILE_PATH: &str = "photos/file_12.jpg";

/// The fake Bot API's per-test answers and what it was asked.
struct Api {
    get_file: Mutex<(u16, Value)>,
    download_status: AtomicU16,
    body: Mutex<Bytes>,
    get_file_calls: AtomicUsize,
    downloads: Mutex<Vec<(String, String)>>,
}

impl Default for Api {
    fn default() -> Self {
        Self {
            get_file: Mutex::new((
                200,
                json!({"ok": true, "result": {"file_id": "file-12", "file_unique_id": "unique-12", "file_size": 11, "file_path": FILE_PATH}}),
            )),
            download_status: AtomicU16::new(200),
            body: Mutex::new(Bytes::from_static(b"photo-bytes")),
            get_file_calls: AtomicUsize::new(0),
            downloads: Mutex::new(Vec::new()),
        }
    }
}

async fn method(
    State(api): State<Arc<Api>>,
    Path((_token, method)): Path<(String, String)>,
) -> Response {
    if method != "getFile" {
        return StatusCode::NOT_FOUND.into_response();
    }
    api.get_file_calls.fetch_add(1, Ordering::SeqCst);
    let (status, value) = api.get_file.lock().unwrap().clone();
    (StatusCode::from_u16(status).unwrap(), Json(value)).into_response()
}

async fn file(
    State(api): State<Arc<Api>>,
    Path((token, path)): Path<(String, String)>,
) -> Response {
    api.downloads.lock().unwrap().push((token, path));
    let status = StatusCode::from_u16(api.download_status.load(Ordering::SeqCst)).unwrap();
    (status, api.body.lock().unwrap().clone()).into_response()
}

struct Files {
    api: Arc<Api>,
    client: Arc<TelegramClient>,
    source: TelegramFileSource,
    channel: ResolvedChannel,
    event: EventRefPayload,
    activity: Activity,
    locator: AttachmentSource,
}

impl Files {
    fn new(edit: impl FnOnce(&mut cmaeg_core::channel::ChannelDefinition)) -> Self {
        let api = Arc::new(Api::default());
        let state = api.clone();
        // Its own thread, so the fake's tracing never mixes with the gateway's.
        let origin = isolated(move || {
            serve(
                Router::new()
                    .route("/{token}/{method}", post(method))
                    .route("/file/{token}/{*path}", get(file))
                    .with_state(state),
            )
        });
        let client = Arc::new(TelegramClient::new(&origin, Duration::from_secs(5)).unwrap());
        let channel = resolved(|definition| {
            definition.sources = vec![Source::Telegram {
                topic_lifecycle: false,
                topic_creation: false,
                message_events: false,
                channel_posts: false,
                member_join_chat_ids: Vec::new(),
                bot_id: BOT_ID.into(),
                bot_username: None,
                admission: AdmissionPolicy::All {},
            }];
            definition.secrets.telegram_bot_token = Some(Secret::new(TOKEN));
            edit(definition);
        });
        Self {
            api,
            source: TelegramFileSource::new(client.clone()),
            client,
            channel,
            event: event(ReplyAddress::None),
            activity: activity(
                SourceKind::Telegram {
                    platform: Platform::Telegram,
                    bot_id: BOT_ID.into(),
                },
                None,
            ),
            locator: AttachmentSource::Telegram {
                file_id: "file-12".into(),
                file_unique_id: "unique-12".into(),
            },
        }
    }

    async fn fetch(&self, declared_size: Option<u64>) -> Result<Bytes, FetchError> {
        let mut request = request(
            &self.channel,
            &self.event,
            &self.activity,
            &self.locator,
            25_000_000,
            far(),
        );
        request.declared_size = declared_size;
        self.source.fetch(request).await.map(|body| body.bytes)
    }

    fn calls(&self) -> (usize, usize) {
        (
            self.api.get_file_calls.load(Ordering::SeqCst),
            self.api.downloads.lock().unwrap().len(),
        )
    }

    fn answer_get_file(&self, status: u16, value: Value) {
        *self.api.get_file.lock().unwrap() = (status, value);
    }
}

#[tokio::test]
async fn a_file_is_resolved_then_downloaded_with_the_channel_token() {
    let files = Files::new(|_| {});
    assert_eq!(
        files.fetch(Some(11)).await.unwrap(),
        Bytes::from_static(b"photo-bytes")
    );
    assert_eq!(files.calls(), (1, 1));
    assert_eq!(
        files.api.downloads.lock().unwrap()[0],
        (format!("bot{TOKEN}"), FILE_PATH.to_owned())
    );
}

#[tokio::test]
async fn telegram_refusals_are_classified() {
    for (status, value, expected) in [
        (
            400,
            json!({"ok": false, "error_code": 400, "description": "Bad Request: file is too big"}),
            FetchError::TooLarge,
        ),
        (
            400,
            json!({"ok": false, "error_code": 400, "description": "Bad Request: wrong file_id"}),
            FetchError::NotFound,
        ),
        (
            429,
            json!({"ok": false, "error_code": 429, "description": "Too Many Requests"}),
            FetchError::RateLimited,
        ),
        (
            500,
            json!({"ok": false, "error_code": 500, "description": "Internal Server Error"}),
            FetchError::Upstream,
        ),
        // Telegram omits the path for files it will not serve to bots.
        (
            200,
            json!({"ok": true, "result": {"file_id": "file-12", "file_unique_id": "unique-12"}}),
            FetchError::TooLarge,
        ),
    ] {
        let files = Files::new(|_| {});
        files.answer_get_file(status, value.clone());
        assert_eq!(files.fetch(None).await, Err(expected), "{value}");
        assert_eq!(files.calls(), (1, 0), "{value}");
    }
}

#[tokio::test]
async fn download_statuses_are_classified() {
    for (status, expected) in [
        (404, FetchError::NotFound),
        (429, FetchError::RateLimited),
        (502, FetchError::Upstream),
    ] {
        let files = Files::new(|_| {});
        files.api.download_status.store(status, Ordering::SeqCst);
        assert_eq!(files.fetch(None).await, Err(expected), "{status}");
        assert_eq!(files.calls(), (1, 1));
    }
}

#[tokio::test]
async fn a_declared_size_above_the_bot_limit_sends_no_request() {
    let files = Files::new(|_| {});
    assert_eq!(
        files.fetch(Some(30_000_000)).await,
        Err(FetchError::TooLarge)
    );
    assert_eq!(files.calls(), (0, 0));
}

#[tokio::test]
async fn unsafe_file_paths_are_rejected_without_a_download() {
    for path in ["../secrets", "photos/../../x", "/etc/passwd", "%2e%2e/x"] {
        let files = Files::new(|_| {});
        files.answer_get_file(
            200,
            json!({"ok": true, "result": {"file_id": "file-12", "file_unique_id": "unique-12", "file_path": path}}),
        );
        assert_eq!(files.fetch(None).await, Err(FetchError::Rejected), "{path}");
        assert_eq!(files.calls(), (1, 0), "{path}");
        assert_eq!(
            files
                .client
                .download_file(&Secret::new(TOKEN), path, far())
                .await
                .unwrap_err(),
            FileError::Rejected
        );
        assert_eq!(files.calls(), (1, 0), "{path}");
    }
}

#[tokio::test]
async fn a_bot_no_longer_configured_or_without_a_token_is_not_retrievable() {
    let other_bot = Files::new(|definition| {
        definition.sources = vec![Source::Telegram {
            topic_lifecycle: false,
            topic_creation: false,
            message_events: false,
            channel_posts: false,
            member_join_chat_ids: Vec::new(),
            bot_id: "78".into(),
            bot_username: None,
            admission: AdmissionPolicy::All {},
        }];
    });
    assert_eq!(other_bot.fetch(None).await, Err(FetchError::NotRetrievable));
    assert_eq!(other_bot.calls(), (0, 0));
    let without_token = Files::new(|definition| {
        definition.secrets.telegram_bot_token = None;
    });
    assert_eq!(
        without_token.fetch(None).await,
        Err(FetchError::NotRetrievable)
    );
    assert_eq!(without_token.calls(), (0, 0));
    let mut relay_activity = Files::new(|_| {});
    relay_activity.activity.source = SourceKind::NyxidRelay {
        platform: Platform::Telegram,
        route_id: "route-1".into(),
    };
    assert_eq!(
        relay_activity.fetch(None).await,
        Err(FetchError::NotRetrievable)
    );
    assert_eq!(relay_activity.calls(), (0, 0));
}

#[tokio::test]
async fn files_are_capped_at_the_bot_limit_and_can_be_streamed() {
    let files = Files::new(|_| {});
    *files.api.body.lock().unwrap() = Bytes::from(vec![1; 50_000]);
    let mut request = request(
        &files.channel,
        &files.event,
        &files.activity,
        &files.locator,
        40_000,
        far(),
    );
    assert_eq!(
        files.source.fetch(request).await.unwrap_err(),
        FetchError::TooLarge
    );
    assert_eq!(
        files.source.open(request).await.unwrap_err(),
        FetchError::TooLarge
    );
    request.cap = 50_000;
    let opened = files.source.open(request).await.unwrap();
    assert_eq!(opened.content_length, Some(50_000));
    let received: usize = opened
        .stream
        .map(|chunk| chunk.unwrap().len())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .sum();
    assert_eq!(received, 50_000);
}

#[tokio::test]
async fn an_expired_deadline_is_timed_out() {
    let files = Files::new(|_| {});
    // Well in the past: a deadline of exactly now only fires at the timer's
    // next millisecond tick, which a loopback answer can beat.
    let expired = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    let request = request(
        &files.channel,
        &files.event,
        &files.activity,
        &files.locator,
        1_000,
        expired,
    );
    assert_eq!(
        files.source.fetch(request).await.unwrap_err(),
        FetchError::TimedOut
    );
}

#[tokio::test]
async fn the_token_never_reaches_logs_or_errors() {
    let (guard, logs) = capture_logs(TRACE_ENV);
    let files = Files::new(|_| {});
    files.fetch(None).await.unwrap();
    files.api.download_status.store(500, Ordering::SeqCst);
    let upstream = files.fetch(None).await.unwrap_err();
    let unreachable = TelegramClient::new("http://127.0.0.1:9", Duration::from_secs(1))
        .unwrap()
        .download_file(&Secret::new(TOKEN), FILE_PATH, far())
        .await
        .unwrap_err();
    let lookup = TelegramClient::new("http://127.0.0.1:9", Duration::from_secs(1))
        .unwrap()
        .get_file(&Secret::new(TOKEN), "file-12")
        .await
        .unwrap_err();
    drop(guard);
    assert_eq!(upstream, FetchError::Upstream);
    assert_eq!(
        (unreachable, lookup),
        (FileError::Unavailable, FileError::Unavailable)
    );
    let text = logs.text();
    assert_absent(&text, &[TOKEN, "fixture-telegram-token", "/file/bot"]);
    for shown in [
        format!("{upstream:?}"),
        format!("{unreachable:?}"),
        format!("{lookup:?}"),
    ] {
        assert_absent(&shown, &[TOKEN, "fixture-telegram-token", "http"]);
    }
}
