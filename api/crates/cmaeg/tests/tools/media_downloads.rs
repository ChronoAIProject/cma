//! GET /v1/media/content with links the tests seal themselves: the exact
//! bytes and hardened head from every source, single use, header-only
//! authority, revocation, quotas, the forwarding deadline and log hygiene.
use super::{
    download_support::{
        autonomous, autonomous_custom, autonomous_native, download, download_with, link, link_at,
        redefine, relay, served,
    },
    media::support::{assert_absent, capture_logs, TRACE_ENV},
    media_support::{native, relayed, MediaSetup},
    relay_support::now,
    support::{problem, App},
};
use cmaeg::{
    media::{source::SourceCredential, FetchError},
    tools::{DOWNLOAD_STREAMS_GLOBAL, DOWNLOAD_STREAMS_PER_CHANNEL},
};
use cmaeg_core::{
    activity::{AttachmentKind, AttachmentSource},
    channel::{ChannelDefinition, Source},
    profile::ActionMode,
};
use std::{sync::atomic::Ordering, time::Duration};

const KIB: usize = 1024;

/// Distinct bytes, so a reordered or truncated body cannot pass.
fn original(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 251) as u8).collect()
}

/// An autonomous relay app with one file of `length` bytes at index 0.
fn relay_file(length: usize) -> MediaSetup {
    let mut file = relayed(0, AttachmentKind::File);
    file.filename = Some("report.pdf".into());
    let setup = MediaSetup::new(vec![file]).edit(autonomous);
    setup.fakes.source.serve(&relay(0), Ok(original(length)));
    setup
}

/// A custom app whose attachment 0 comes from `locator`.
fn custom(locator: AttachmentSource) -> MediaSetup {
    let mut file = relayed(0, AttachmentKind::File);
    file.source = Some(locator.clone());
    let setup = MediaSetup::new(vec![file]).custom().edit(autonomous_custom);
    setup.fakes.source.serve(&locator, Ok(original(3 * KIB)));
    setup
}

fn file_url() -> AttachmentSource {
    AttachmentSource::Url {
        url: "https://files.example/report.pdf".into(),
    }
}

async fn revoked(response: reqwest::Response) {
    let body = problem(response, 403, "forbidden").await;
    assert_eq!(body["detail"], "media_link_revoked");
}

/// A link to attachment 0 issued before the owner's `edit` is revoked by it.
async fn revoked_after(setup: MediaSetup, edit: impl FnOnce(&mut ChannelDefinition)) {
    let app = App::with_media(setup).await;
    let token = link(&app, 0);
    redefine(&app, edit).await;
    revoked(download(&app, &token).await).await;
}

#[tokio::test]
async fn download_relay() {
    let setup = relay_file(150 * KIB);
    let fakes = setup.fakes.clone();
    let app = App::with_media(setup).await;
    let body = served(
        download(&app, &link(&app, 0)).await,
        "report.pdf",
        Some(150 * KIB),
    )
    .await;
    assert_eq!(body, original(150 * KIB));
    // Relay originals are fetched with the channel key, never a reply token.
    let seen = fakes.source.seen();
    assert_eq!(
        seen.last().unwrap().credential,
        SourceCredential::ChannelKey
    );
    assert_eq!(seen.last().unwrap().cap, 20 * 1024 * 1024);
}

#[tokio::test]
async fn download_native_telegram() {
    let setup = MediaSetup::new(vec![native("clip", AttachmentKind::Video)])
        .native()
        .edit(autonomous_native);
    setup.fakes.source.serve(
        &AttachmentSource::Telegram {
            file_id: "file-clip".into(),
            file_unique_id: "clip".into(),
        },
        Ok(original(70 * KIB)),
    );
    let app = App::with_media(setup).await;
    let body = served(
        download(&app, &link(&app, 0)).await,
        "attachment.bin",
        Some(70 * KIB),
    )
    .await;
    assert_eq!(body, original(70 * KIB));
}

#[tokio::test]
async fn download_custom_url() {
    let app = App::with_media(custom(file_url())).await;
    let body = served(
        download(&app, &link(&app, 0)).await,
        "attachment.bin",
        Some(3 * KIB),
    )
    .await;
    assert_eq!(body, original(3 * KIB));
}

#[tokio::test]
async fn download_inline() {
    let app = App::with_media(custom(AttachmentSource::Inline {
        pointer: "/attachments/0/data".into(),
    }))
    .await;
    let body = served(
        download(&app, &link(&app, 0)).await,
        "attachment.bin",
        Some(3 * KIB),
    )
    .await;
    assert_eq!(body, original(3 * KIB));
}

#[tokio::test]
async fn download_while_media_disabled_is_503() {
    let app = App::new().await;
    let response = download(&app, &link(&app, 0)).await;
    let body = problem(response, 503, "media_unavailable").await;
    assert_eq!(body["detail"], "media_disabled");
}

#[tokio::test]
async fn second_use_is_gone() {
    let app = App::with_media(relay_file(KIB)).await;
    let token = link(&app, 0);
    served(download(&app, &token).await, "report.pdf", Some(KIB)).await;
    let body = problem(download(&app, &token).await, 410, "media_unavailable").await;
    assert_eq!(body["detail"], "media_link_used");
}

#[tokio::test]
async fn refused_requests_consume_the_link() {
    let setup = relay_file(KIB);
    let app = App::with_media(setup).await;
    // A link to an attachment the event does not have is refused, and spent.
    let token = link(&app, 5);
    let missing = problem(download(&app, &token).await, 404, "attachment_not_found").await;
    assert_eq!(missing["detail"], "attachment_not_found");
    let again = problem(download(&app, &token).await, 410, "media_unavailable").await;
    assert_eq!(again["detail"], "media_link_used");
}

#[tokio::test]
async fn query_string_token_is_ignored() {
    let app = App::with_media(relay_file(KIB)).await;
    let token = link(&app, 0);
    let response = download_with(&app, &format!("?token={token}"), &[]).await;
    let body = problem(response, 400, "invalid_request").await;
    assert_eq!(body["detail"], "media_link_invalid");
    // The link was never read, so it still works from the header.
    served(download(&app, &token).await, "report.pdf", Some(KIB)).await;
}

#[tokio::test]
async fn two_authorization_headers_are_invalid() {
    let app = App::with_media(relay_file(KIB)).await;
    let token = link(&app, 0);
    let bearer = format!("Bearer {token}");
    for headers in [
        vec![bearer.clone(), bearer.clone()],
        vec![format!("Basic {token}")],
        vec![format!("Bearer {}", app.key)],
        vec![format!("Bearer {token}x")],
        vec!["Bearer".to_owned()],
    ] {
        let body = problem(
            download_with(&app, "", &headers).await,
            400,
            "invalid_request",
        )
        .await;
        assert_eq!(body["detail"], "media_link_invalid");
    }
    // None of those spent the link, and the scheme is case-insensitive.
    let response = download_with(&app, "", &[format!("bEARER {token}")]).await;
    served(response, "report.pdf", Some(KIB)).await;
}

#[tokio::test]
async fn expired_link_is_gone() {
    let app = App::with_media(relay_file(KIB)).await;
    // Issued 200 s ago for 120 s: its expiry has passed.
    let (token, payload) = link_at(&app, 0, now() - 200, 120);
    assert!(payload.expires_at <= now());
    let body = problem(download(&app, &token).await, 410, "media_unavailable").await;
    assert_eq!(body["detail"], "media_link_expired");
}

#[tokio::test]
async fn revoked_on_key_rotation() {
    revoked_after(relay_file(KIB), |definition| {
        definition.secrets.agent_key_hash = format!("sha256:{}", "7".repeat(64));
    })
    .await;
}

#[tokio::test]
async fn revoked_on_channel_deleted() {
    let app = App::with_media(relay_file(KIB)).await;
    let token = link(&app, 0);
    app.fixture
        .backend
        .record_status
        .store(404, Ordering::SeqCst);
    app.state.resolver.invalidate(app.fixture.id.as_str());
    revoked(download(&app, &token).await).await;
}

#[tokio::test]
async fn revoked_on_route_removed() {
    revoked_after(relay_file(KIB), |definition| {
        for source in &mut definition.sources {
            if let Source::NyxidRelay { route_ids, .. } = source {
                *route_ids = vec!["route-2".into()];
            }
        }
    })
    .await;
}

#[tokio::test]
async fn revoked_on_origin_removed() {
    revoked_after(custom(file_url()), |definition| {
        for source in &mut definition.sources {
            if let Source::Custom {
                attachments: Some(attachments),
                ..
            } = source
            {
                attachments.origins = vec!["https://other.example".into()];
            }
        }
    })
    .await;
}

#[tokio::test]
async fn revoked_on_bot_removed() {
    let setup = MediaSetup::new(vec![native("clip", AttachmentKind::Video)])
        .native()
        .edit(autonomous_native);
    revoked_after(setup, |definition| {
        definition
            .sources
            .retain(|source| !matches!(source, Source::Telegram { .. }));
        definition.secrets.telegram_bot_token = None;
        definition.secrets.telegram_webhook_secret = None;
    })
    .await;
}

#[tokio::test]
async fn revoked_on_binding_guarded() {
    revoked_after(relay_file(KIB), |definition| {
        definition.profile.permission.action_mode = ActionMode::Guarded;
    })
    .await;
}

#[tokio::test]
async fn revoked_on_provider_removed_from_set() {
    // The gateway restarted with cma_codex no longer installing its tools.
    let app = App::with_media(relay_file(KIB).tool_providers(vec![])).await;
    revoked(download(&app, &link(&app, 0)).await).await;
}

#[tokio::test]
async fn source_failures_are_classified_before_any_header() {
    let setup = relay_file(KIB);
    let fakes = setup.fakes.clone();
    let app = App::with_media(setup).await;
    for (error, status, code, detail) in [
        (
            FetchError::NotFound,
            404,
            "attachment_not_found",
            "attachment_not_found",
        ),
        (
            FetchError::TooLarge,
            413,
            "payload_too_large",
            "attachment_too_large",
        ),
        (
            FetchError::RateLimited,
            429,
            "rate_limited",
            "source_rate_limited",
        ),
        (
            FetchError::Upstream,
            503,
            "media_unavailable",
            "source_unavailable",
        ),
        (
            FetchError::Unauthorized,
            503,
            "media_unavailable",
            "source_unavailable",
        ),
    ] {
        fakes.source.serve(&relay(0), Err(error));
        let body = problem(download(&app, &link(&app, 0)).await, status, code).await;
        assert_eq!(body["detail"], detail, "{error}");
    }
}

#[tokio::test]
async fn oversize_stream_aborts() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let setup = relay_file(300 * KIB).limits(|limits| limits.max_bytes = 100 * 1024);
    setup.fakes.source.undeclared();
    let app = App::with_media(setup).await;
    let response = download(&app, &link(&app, 0)).await;
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("content-length").is_none());
    assert!(response.bytes().await.is_err());
    let text = handle.render();
    assert!(
        text.contains("cmaeg_media_downloads_total{outcome=\"aborted\"} 1"),
        "{text}"
    );
    assert!(!text.contains("outcome=\"served\""), "{text}");
}

#[tokio::test]
async fn third_concurrent_stream_on_a_channel_is_busy() {
    let setup = relay_file(KIB);
    // A chunked body that stays open holds its slots.
    setup.fakes.source.undeclared();
    setup.fakes.source.stall();
    let app = App::with_media(setup).await;
    let first = download(&app, &link(&app, 0)).await;
    let second = download(&app, &link(&app, 0)).await;
    assert_eq!(
        (first.status().as_u16(), second.status().as_u16()),
        (200, 200)
    );
    let third = download(&app, &link(&app, 0)).await;
    assert_eq!(third.headers()["retry-after"], "2");
    let body = problem(third, 503, "media_unavailable").await;
    assert_eq!(body["detail"], "download_busy");
}

#[tokio::test]
async fn seventeenth_global_stream_is_busy() {
    let app = App::with_media(relay_file(KIB)).await;
    let streams = app.state.downloads.streams();
    let held = streams
        .clone()
        .acquire_many_owned(DOWNLOAD_STREAMS_GLOBAL as u32)
        .await
        .unwrap();
    let response = download(&app, &link(&app, 0)).await;
    assert_eq!(response.headers()["retry-after"], "2");
    let body = problem(response, 503, "media_unavailable").await;
    assert_eq!(body["detail"], "download_busy");
    drop(held);
    served(
        download(&app, &link(&app, 0)).await,
        "report.pdf",
        Some(KIB),
    )
    .await;
}

#[tokio::test]
async fn byte_quota_is_rate_limited() {
    let setup = relay_file(40 * KIB).limits(|limits| limits.download_bytes_per_hour = 64 * 1024);
    let fakes = setup.fakes.clone();
    let app = App::with_media(setup).await;
    served(
        download(&app, &link(&app, 0)).await,
        "report.pdf",
        Some(40 * KIB),
    )
    .await;
    let read = fakes.source.chunks_read();
    let response = download(&app, &link(&app, 0)).await;
    assert_eq!(response.headers()["retry-after"], "60");
    let body = problem(response, 429, "rate_limited").await;
    assert_eq!(body["detail"], "download_quota");
    // The second original was opened but dropped unread.
    assert_eq!(fakes.source.calls(), 2);
    assert_eq!(fakes.source.chunks_read(), read);
}

#[tokio::test]
async fn unpolled_reader_releases_permits_at_deadline() {
    let setup = relay_file(256 * KIB);
    setup.fakes.source.undeclared();
    setup.fakes.source.stall();
    let full = setup.limits.response_bytes;
    let app = App::with_media(setup).await;
    let media = app.media.clone().unwrap();
    let downloads = app.state.downloads.clone();
    let channel = app.fixture.id.clone();
    let response = download(&app, &link(&app, 0)).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        downloads.streams().available_permits(),
        DOWNLOAD_STREAMS_GLOBAL - 1
    );
    assert_eq!(
        downloads.channel_streams_available(&channel),
        DOWNLOAD_STREAMS_PER_CHANNEL - 1
    );
    assert!(media.responses().available_bytes() < full);
    // The body is never read; only the 300 s deadline can free it. The extra
    // second covers the time between the handler setting it and the pause.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(299)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        downloads.streams().available_permits(),
        DOWNLOAD_STREAMS_GLOBAL - 1
    );
    tokio::time::advance(Duration::from_secs(2)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        downloads.streams().available_permits(),
        DOWNLOAD_STREAMS_GLOBAL
    );
    assert_eq!(
        downloads.channel_streams_available(&channel),
        DOWNLOAD_STREAMS_PER_CHANNEL
    );
    assert_eq!(media.responses().available_bytes(), full);
    drop(response);
}

#[tokio::test]
async fn download_logs_carry_no_token() {
    let mut file = relayed(0, AttachmentKind::File);
    file.filename = Some("quarterly-secret-plan.pdf".into());
    let setup = MediaSetup::new(vec![file]).edit(autonomous);
    setup.fakes.source.serve(&relay(0), Ok(original(2 * KIB)));
    let (guard, logs) = capture_logs(TRACE_ENV);
    let app = App::with_media(setup).await;
    let (token, payload) = link_at(&app, 0, now(), 120);
    served(
        download(&app, &token).await,
        "quarterly-secret-plan.pdf",
        Some(2 * KIB),
    )
    .await;
    problem(download(&app, &token).await, 410, "media_unavailable").await;
    drop(guard);
    let text = logs.text();
    assert!(text.contains("cmaeg.media.download"), "{text}");
    assert!(text.contains("outcome=\"served\"") || text.contains("outcome=served"));
    assert_absent(
        &text,
        &[
            &token,
            &payload.jti,
            &payload.agent_key_hash,
            "quarterly-secret-plan",
            "cmaeg1.md.",
            &app.key,
        ],
    );
}
