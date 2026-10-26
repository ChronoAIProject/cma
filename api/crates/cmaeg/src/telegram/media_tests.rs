use super::{
    media_fixture::{Answer, Fixture},
    Error,
};
use crate::media::{
    outbound::{self, PreparedFile, MAX_OUTBOUND_BYTES},
    source::url::UrlTransport,
    FetchError,
};
use axum::{body::Body, http::StatusCode, response::Response, routing::get, Router};
use cmaeg_core::{
    channel::Secret,
    message::{NativeAttachment, NativeMediaKind},
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;

fn file() -> PreparedFile {
    PreparedFile {
        bytes: vec![1, 2, 3, 4],
        filename: "diagram.png".into(),
        mime: "image/png".into(),
        caption: Some("图 <literal> & caption".into()),
    }
}
fn item(source: Value) -> NativeAttachment {
    serde_json::from_value(json!({"kind":"image","source":source,"filename":"diagram.png","mime_type":"image/png","caption":"图 <literal> & caption"})).unwrap()
}
#[tokio::test]
async fn native_telegram_all_media_methods_send_actual_multipart_with_exact_topic_bytes_and_metadata(
) {
    let f = Fixture::new().await;
    for (kind, method, field) in [
        (NativeMediaKind::Image, "sendPhoto", "photo"),
        (NativeMediaKind::File, "sendDocument", "document"),
        (NativeMediaKind::Audio, "sendAudio", "audio"),
        (NativeMediaKind::Video, "sendVideo", "video"),
    ] {
        let upload = f
            .api
            .prepare_media("-42", Some("7"), kind, file())
            .await
            .unwrap();
        assert_eq!(
            f.api
                .send_media(&Secret::new("77:fixture"), "77", upload)
                .await,
            Ok("800".into())
        );
        let call = f.calls().pop().unwrap();
        assert_eq!(call.path, format!("/bot77:fixture/{method}"));
        assert!(call
            .content_type
            .unwrap()
            .starts_with("multipart/form-data; boundary="));
        assert!(call.authorization.is_none());
        let text = String::from_utf8_lossy(&call.body);
        for expected in [
            format!("name=\"{field}\"; filename=\"diagram.png\""),
            "Content-Type: image/png".into(),
            "name=\"chat_id\"\r\n\r\n-42".into(),
            "name=\"message_thread_id\"\r\n\r\n7".into(),
            "name=\"caption\"\r\n\r\n图 <literal> & caption".into(),
        ] {
            assert!(text.contains(&expected), "missing {expected}");
        }
        assert!(call.body.windows(4).any(|b| b == [1, 2, 3, 4]));
        assert!(!text.contains("77:fixture"));
        assert!(!text.contains("parse_mode"));
    }
    assert_eq!(f.calls().len(), 4);
}
#[tokio::test]
async fn native_telegram_upload_rejects_wrong_token_or_target_before_any_http() {
    let f = Fixture::new().await;
    for (token, bot) in [
        ("88:fixture", "77"),
        ("77:bad/path", "77"),
        ("77:fixture", "077"),
    ] {
        let upload = f
            .api
            .prepare_media("-42", Some("7"), NativeMediaKind::File, file())
            .await
            .unwrap();
        assert_eq!(
            f.api.send_media(&Secret::new(token), bot, upload).await,
            Err(Error::Rejected)
        );
    }
    for (chat, topic) in [
        ("@other", None),
        ("0", None),
        ("042", None),
        ("-42", Some("0")),
        ("-42", Some("../7")),
    ] {
        assert!(f
            .api
            .prepare_media(chat, topic, NativeMediaKind::File, file())
            .await
            .is_err());
    }
    assert!(f.calls().is_empty());
}
#[tokio::test]
async fn native_telegram_upload_checks_every_receipt_identity_and_never_retries() {
    let f = Fixture::new().await;
    for result in [
        json!({"message_id":800,"chat":{"id":-43},"message_thread_id":7}),
        json!({"message_id":800,"chat":{"id":-42},"message_thread_id":8}),
        json!({"message_id":800,"chat":{"id":-42}}),
        json!({"message_id":0,"chat":{"id":-42},"message_thread_id":7}),
        json!({"message_id":"800","chat":{"id":-42},"message_thread_id":7}),
        Value::Null,
    ] {
        f.answer(Answer::json(
            StatusCode::OK,
            json!({"ok":true,"result":result}),
        ));
        let upload = f
            .api
            .prepare_media("-42", Some("7"), NativeMediaKind::File, file())
            .await
            .unwrap();
        assert_eq!(
            f.api
                .send_media(&Secret::new("77:fixture"), "77", upload)
                .await,
            Err(Error::Unconfirmed)
        );
    }
    assert_eq!(f.calls().len(), 6);
}
#[tokio::test]
async fn native_telegram_upload_http_limits_errors_and_lost_answers_are_typed_without_resend() {
    let f = Fixture::new().await;
    let mut broken = Answer::ok();
    broken.broken = true;
    let mut truncated = Answer::ok();
    truncated.body = br#"{"ok":true,"result":"#.to_vec();
    let mut oversized = Answer::ok();
    oversized.body = vec![b'x'; 32769];
    let mut delayed = Answer::ok();
    delayed.delay = Duration::from_millis(2100);
    let cases = [
        (
            Answer::json(
                StatusCode::TOO_MANY_REQUESTS,
                json!({"ok":false,"error_code":429,"parameters":{"retry_after":9}}),
            ),
            Error::RateLimited(9),
        ),
        (
            Answer::json(StatusCode::FORBIDDEN, json!({"ok":false})),
            Error::Rejected,
        ),
        (
            Answer::json(StatusCode::BAD_GATEWAY, json!({})),
            Error::Unconfirmed,
        ),
        (
            Answer::json(StatusCode::TEMPORARY_REDIRECT, json!({})),
            Error::Unconfirmed,
        ),
        (broken, Error::Unconfirmed),
        (truncated, Error::Unconfirmed),
        (oversized, Error::Unconfirmed),
        (delayed, Error::Unconfirmed),
    ];
    let count = cases.len();
    for (answer, expected) in cases {
        f.answer(answer);
        let upload = f
            .api
            .prepare_media("-42", Some("7"), NativeMediaKind::File, file())
            .await
            .unwrap();
        assert_eq!(
            f.api
                .send_media(&Secret::new("77:fixture"), "77", upload)
                .await,
            Err(expected)
        );
    }
    assert_eq!(f.calls().len(), count);
}
#[tokio::test]
async fn native_telegram_media_text_verifies_topic_and_keeps_plain_text_without_edit_authority() {
    let f = Fixture::new().await;
    assert_eq!(
        f.api
            .send_media_text(
                &Secret::new("77:fixture"),
                "77",
                "-42",
                Some("7"),
                "<hello> & text"
            )
            .await,
        Ok("800".into())
    );
    let calls = f.calls();
    assert_eq!(
        serde_json::from_slice::<Value>(&calls[0].body).unwrap(),
        json!({"chat_id":"-42","message_thread_id":"7","text":"<hello> & text"})
    );
    f.answer(Answer::json(
        StatusCode::OK,
        json!({"ok":true,"result":{"message_id":800,"chat":{"id":-42},"message_thread_id":8}}),
    ));
    assert_eq!(
        f.api
            .send_media_text(&Secret::new("77:fixture"), "77", "-42", Some("7"), "text")
            .await,
        Err(Error::Unconfirmed)
    );
}

#[tokio::test]
async fn native_telegram_upload_without_a_topic_never_invents_one() {
    let f = Fixture::new().await;
    f.answer(Answer::json(
        StatusCode::OK,
        json!({"ok":true,"result":{"message_id":800,"chat":{"id":-42}}}),
    ));
    let upload = f
        .api
        .prepare_media("-42", None, NativeMediaKind::File, file())
        .await
        .unwrap();
    assert_eq!(
        f.api
            .send_media(&Secret::new("77:fixture"), "77", upload)
            .await,
        Ok("800".into())
    );
    assert!(!String::from_utf8_lossy(&f.calls()[0].body).contains("message_thread_id"));
}

struct FetchFixture {
    base: String,
    requests: Arc<Mutex<Vec<axum::http::HeaderMap>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for FetchFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl FetchFixture {
    async fn new() -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let capture = requests.clone();
        let app = Router::new().route(
            "/{path}",
            get(
                move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
                    capture.lock().unwrap().push(headers);
                    async move {
                        match uri.path() {
                            "/redirect" => Response::builder()
                                .status(302)
                                .header("location", "http://127.0.0.1/private")
                                .body(Body::empty())
                                .unwrap(),
                            "/large" => Response::new(Body::from(vec![
                                0u8;
                                MAX_OUTBOUND_BYTES as usize
                                    + 1
                            ])),
                            "/stream-large" => Response::new(Body::from_stream(
                                futures_util::stream::iter((0..9).map(|_| {
                                    Ok::<_, std::io::Error>(bytes::Bytes::from(vec![
                                        0u8;
                                        1024 * 1024
                                    ]))
                                })),
                            )),
                            "/half" => {
                                Response::new(Body::from(vec![
                                    0u8;
                                    MAX_OUTBOUND_BYTES as usize / 2 + 1
                                ]))
                            }
                            "/empty" => Response::new(Body::empty()),
                            _ => Response::new(Body::from(vec![1, 2, 3, 4])),
                        }
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base,
            requests,
            task,
        }
    }
}
impl UrlTransport for FetchFixture {
    fn get<'a>(
        &'a self,
        url: &'a reqwest::Url,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<reqwest::Response, FetchError>> {
        Box::pin(async move {
            assert_eq!(url.host_str(), Some("media.example"));
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap()
                .get(format!("{}{}", self.base, url.path()))
                .timeout(deadline.saturating_duration_since(Instant::now()))
                .send()
                .await
                .map_err(|_| FetchError::Upstream)
        })
    }
}
#[tokio::test]
async fn native_telegram_fetched_and_inline_sources_reach_real_upload_as_identical_bytes() {
    let fetch = FetchFixture::new().await;
    let f = Fixture::new().await;
    let items = [
        item(json!({"type":"url","url":"https://media.example/file"})),
        item(json!({"type":"base64","data":"AQIDBA=="})),
    ];
    let files = outbound::prepare_with(&items, &fetch).await.unwrap();
    for file in files {
        let upload = f
            .api
            .prepare_media("-42", Some("7"), NativeMediaKind::Image, file)
            .await
            .unwrap();
        f.api
            .send_media(&Secret::new("77:fixture"), "77", upload)
            .await
            .unwrap();
    }
    let calls = f.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls
        .iter()
        .all(|c| c.body.windows(4).any(|b| b == [1, 2, 3, 4])));
    let requests = fetch.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].contains_key("authorization"));
    assert!(!requests[0].contains_key("cookie"));
}
#[tokio::test]
async fn native_telegram_materialization_retains_aggregate_limits_and_ssrf_policy() {
    let fetch = FetchFixture::new().await;
    for path in ["redirect", "large", "stream-large", "empty"] {
        let request = item(json!({"type":"url","url":format!("https://media.example/{path}")}));
        assert!(
            outbound::prepare_with(&[request], &fetch).await.is_err(),
            "{path}"
        );
    }
    let half = item(json!({"type":"url","url":"https://media.example/half"}));
    assert!(matches!(
        outbound::prepare_with(&[half.clone(), half], &fetch).await,
        Err(FetchError::TooLarge)
    ));
    for url in [
        "https://127.0.0.1/a",
        "https://[::1]/a",
        "https://169.254.169.254/a",
        "https://10.0.0.1/a",
        "https://[::ffff:127.0.0.1]/a",
        "http://media.example/a",
        "https://name:secret@media.example/a",
        "https://media.example/a#fragment",
    ] {
        assert!(
            outbound::prepare(&[item(json!({"type":"url","url":url}))])
                .await
                .is_err(),
            "{url}"
        );
    }
    for source in [
        json!({"type":"telegram","file_id":"other-bot-file"}),
        json!({"type":"discord","attachment_id":"123"}),
        json!({"type":"base64","data":"AQ==","file_id":"foreign"}),
    ] {
        assert!(
            serde_json::from_value::<NativeAttachment>(json!({"kind":"file","source":source}))
                .is_err()
        );
    }
}
