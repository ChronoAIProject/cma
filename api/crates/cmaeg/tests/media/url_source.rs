//! Custom URL attachments: the allowlist and status policy through a loopback
//! transport, and the address policy of the production transport.
use super::support::{activity, event, far, request, resolved, serve};
use axum::{
    extract::Path,
    http::{header::LOCATION, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use cmaeg::{
    channels::resolver::ResolvedChannel,
    media::{
        source::{
            url::{PinnedTransport, UrlSource, UrlTransport},
            MediaSource,
        },
        FetchError,
    },
};
use cmaeg_core::{
    activity::{Activity, AttachmentSource, SourceKind},
    channel::{CustomAttachments, CustomAuth, Source},
    seal::{EventRefPayload, ReplyAddress},
};
use futures_util::{future::BoxFuture, StreamExt};
use reqwest::Url;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, time::Instant};

const ORIGIN: &str = "https://media.example";

/// Serves every allowlisted HTTPS URL from a loopback server at the same
/// path, recording what was asked.
struct Loopback {
    base: String,
    requested: Mutex<Vec<String>>,
}

impl UrlTransport for Loopback {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        _deadline: Instant,
    ) -> BoxFuture<'a, Result<reqwest::Response, FetchError>> {
        self.requested.lock().unwrap().push(url.to_string());
        Box::pin(async move {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap()
                .get(format!("{}{}", self.base, url.path()))
                .send()
                .await
                .map_err(|_| FetchError::Upstream)
        })
    }
}

async fn status(Path(code): Path<u16>) -> StatusCode {
    StatusCode::from_u16(code).unwrap()
}

async fn redirect() -> Response {
    (StatusCode::FOUND, [(LOCATION, "http://127.0.0.1/inside")]).into_response()
}

struct Custom {
    transport: Arc<Loopback>,
    source: UrlSource,
    channel: ResolvedChannel,
    event: EventRefPayload,
    activity: Activity,
}

impl Custom {
    async fn new(origins: &[&str]) -> Self {
        let base = serve(
            Router::new()
                .route(
                    "/photo.png",
                    get(|| async { Bytes::from_static(b"png-bytes") }),
                )
                .route("/big.bin", get(|| async { Bytes::from(vec![1; 300_000]) }))
                .route("/status/{code}", get(status))
                .route("/redirect", get(redirect)),
        )
        .await;
        let transport = Arc::new(Loopback {
            base,
            requested: Mutex::new(Vec::new()),
        });
        Self {
            source: UrlSource::new(transport.clone()),
            transport,
            channel: channel(origins),
            event: event(ReplyAddress::None),
            activity: activity(SourceKind::Custom, None),
        }
    }

    async fn fetch(&self, url: &str, cap: u64) -> Result<Bytes, FetchError> {
        let locator = AttachmentSource::Url { url: url.into() };
        let request = request(
            &self.channel,
            &self.event,
            &self.activity,
            &locator,
            cap,
            far(),
        );
        self.source.fetch(request).await.map(|body| body.bytes)
    }

    fn requested(&self) -> usize {
        self.transport.requested.lock().unwrap().len()
    }
}

fn channel(origins: &[&str]) -> ResolvedChannel {
    let origins: Vec<String> = origins.iter().map(|origin| (*origin).to_owned()).collect();
    resolved(|definition| {
        definition.sources = vec![Source::Custom {
            auth: CustomAuth::Hmac {
                key_id: "key-1".into(),
            },
            prompt_template: None,
            schema: None,
            attachments: Some(CustomAttachments {
                pointer: "/attachments".into(),
                origins,
            }),
        }];
    })
}

#[tokio::test]
async fn an_allowlisted_url_is_fetched_through_the_transport() {
    let custom = Custom::new(&[ORIGIN]).await;
    assert_eq!(
        custom
            .fetch("https://media.example/photo.png", 1024)
            .await
            .unwrap(),
        Bytes::from_static(b"png-bytes")
    );
    assert_eq!(
        *custom.transport.requested.lock().unwrap(),
        ["https://media.example/photo.png"]
    );
    let locator = AttachmentSource::Url {
        url: "https://media.example/photo.png".into(),
    };
    let opened = custom
        .source
        .open(request(
            &custom.channel,
            &custom.event,
            &custom.activity,
            &locator,
            1024,
            far(),
        ))
        .await
        .unwrap();
    assert_eq!(opened.stream.count().await, 1);
}

#[tokio::test]
async fn a_redirect_is_rejected_and_never_followed() {
    let custom = Custom::new(&[ORIGIN]).await;
    assert_eq!(
        custom.fetch("https://media.example/redirect", 1024).await,
        Err(FetchError::Rejected)
    );
    assert_eq!(
        custom.fetch("https://media.example/status/302", 1024).await,
        Err(FetchError::Rejected)
    );
}

#[tokio::test]
async fn statuses_are_classified() {
    let custom = Custom::new(&[ORIGIN]).await;
    for (code, expected) in [
        (404, FetchError::NotFound),
        (429, FetchError::RateLimited),
        (401, FetchError::Rejected),
        (503, FetchError::Busy),
        (500, FetchError::Upstream),
    ] {
        assert_eq!(
            custom
                .fetch(&format!("https://media.example/status/{code}"), 1024)
                .await,
            Err(expected),
            "{code}"
        );
    }
}

#[tokio::test]
async fn an_origin_removed_from_the_current_definition_is_rejected() {
    let custom = Custom::new(&["https://other.example"]).await;
    assert_eq!(
        custom.fetch("https://media.example/photo.png", 1024).await,
        Err(FetchError::Rejected)
    );
    let mut without_setting = Custom::new(&[ORIGIN]).await;
    without_setting.channel = resolved(|_| {});
    assert_eq!(
        without_setting
            .fetch("https://media.example/photo.png", 1024)
            .await,
        Err(FetchError::Rejected)
    );
    assert_eq!(custom.requested() + without_setting.requested(), 0);
}

#[tokio::test]
async fn an_oversized_body_is_too_large() {
    let custom = Custom::new(&[ORIGIN]).await;
    assert_eq!(
        custom.fetch("https://media.example/big.bin", 100_000).await,
        Err(FetchError::TooLarge)
    );
    assert_eq!(
        custom
            .fetch("https://media.example/big.bin", 300_000)
            .await
            .unwrap()
            .len(),
        300_000
    );
}

#[tokio::test]
async fn only_https_without_userinfo_is_fetched() {
    let custom = Custom::new(&[ORIGIN, "http://media.example"]).await;
    for url in [
        "http://media.example/photo.png",
        "https://user:pass@media.example/photo.png",
        "https://user@media.example/photo.png",
        "not a url",
    ] {
        assert_eq!(
            custom.fetch(url, 1024).await,
            Err(FetchError::Rejected),
            "{url}"
        );
    }
    assert_eq!(custom.requested(), 0);
}

#[tokio::test]
async fn a_url_locator_on_a_non_custom_activity_is_not_retrievable() {
    let mut custom = Custom::new(&[ORIGIN]).await;
    custom.activity = activity(
        SourceKind::NyxidRelay {
            platform: cmaeg_core::channel::Platform::Telegram,
            route_id: "route-1".into(),
        },
        None,
    );
    assert_eq!(
        custom.fetch("https://media.example/photo.png", 1024).await,
        Err(FetchError::NotRetrievable)
    );
    assert_eq!(custom.requested(), 0);
}

#[tokio::test]
async fn the_pinned_transport_refuses_loopback_without_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = Url::parse(&format!("https://localhost:{port}/photo.png")).unwrap();
    assert_eq!(
        PinnedTransport
            .get(&url, Instant::now() + Duration::from_secs(5))
            .await
            .unwrap_err(),
        FetchError::Rejected
    );
    // Through the source too, with the loopback origin allowlisted.
    let origin = format!("https://localhost:{port}");
    let channel = channel(&[&origin]);
    let event = event(ReplyAddress::None);
    let activity = activity(SourceKind::Custom, None);
    let locator = AttachmentSource::Url {
        url: url.to_string(),
    };
    assert_eq!(
        UrlSource::pinned()
            .fetch(request(&channel, &event, &activity, &locator, 1024, far()))
            .await
            .unwrap_err(),
        FetchError::Rejected
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err(),
        "a connection reached the loopback listener"
    );
    for literal in [
        "https://127.0.0.1/x",
        "https://[::1]/x",
        "https://10.0.0.1/x",
    ] {
        assert_eq!(
            PinnedTransport
                .get(&Url::parse(literal).unwrap(), far())
                .await
                .unwrap_err(),
            FetchError::Rejected,
            "{literal}"
        );
    }
}
