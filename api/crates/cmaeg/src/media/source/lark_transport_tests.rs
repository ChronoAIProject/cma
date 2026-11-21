use super::{test_support::*, *};
use crate::media::source::MediaSource;
use cmaeg_core::channel::Platform;
use serde_json::json;

#[tokio::test]
async fn each_region_fetches_the_native_resource_with_only_the_exact_channel_key() {
    for platform in [Platform::Lark, Platform::Feishu] {
        let fixture = Fixture::new(platform).await;
        assert_eq!(fixture.fetch().await.unwrap(), b"opaque-image-fixture");
        let calls = fixture.server.requests.lock().unwrap();
        for (path, bearer) in calls
            .iter()
            .filter(|(path, _)| !path.contains("channel-records"))
        {
            assert_eq!(bearer, &format!("Bearer {KEY}"), "{path}");
            assert!(!path.contains("relay-envelope-different"));
            assert!(!path.contains("channel-relay"));
        }
        assert_eq!(calls.iter().filter(|(path, _)| path == MEDIA).count(), 1);
    }
}

#[tokio::test]
async fn missing_scope_and_wrong_bot_never_request_media_or_try_another_credential() {
    for (path, reply) in [
        (
            "/api/v1/user-services",
            Reply {
                status: 403,
                ..Reply::json(json!({}))
            },
        ),
        (
            BOT,
            Reply::json(json!({"code":0,"bot":{"open_id":"ou_other_bot"}})),
        ),
        (
            BOT,
            Reply::json(json!({"code":999,"bot":{"open_id":"ou_source"}})),
        ),
    ] {
        let fixture = Fixture::new(Platform::Lark).await;
        fixture.server.set(path, reply);
        assert_eq!(fixture.fetch().await.unwrap_err(), FetchError::Unauthorized);
        let paths = fixture.server.paths();
        assert!(!paths.iter().any(|path| path == MEDIA));
        assert_eq!(
            paths
                .iter()
                .filter(|called| called.as_str() == path)
                .count(),
            1
        );
        assert!(!paths.iter().any(|path| path.contains("channel-relay")));
    }
}

#[tokio::test]
async fn redirect_and_provider_json_errors_are_refused_without_following() {
    for (path, status, content_type) in [
        (BOT, 302, "application/json"),
        (MEDIA, 307, "image/png"),
        (MEDIA, 200, "application/json; charset=utf-8"),
        (MEDIA, 206, "image/png"),
    ] {
        let fixture = Fixture::new(Platform::Lark).await;
        fixture.server.set(
            path,
            Reply {
                status,
                content_type,
                body: b"{}".to_vec(),
                delay: Duration::ZERO,
            },
        );
        assert_eq!(fixture.fetch().await.unwrap_err(), FetchError::Rejected);
        assert!(!fixture
            .server
            .paths()
            .iter()
            .any(|path| path == "/redirect-trap"));
    }
}

#[tokio::test]
async fn metadata_media_and_deadlines_are_bounded() {
    let fixture = Fixture::new(Platform::Lark).await;
    fixture.server.set(
        "/api/v1/user-services",
        Reply {
            body: vec![b' '; METADATA_CAP as usize + 1],
            ..Reply::json(json!({}))
        },
    );
    assert_eq!(fixture.fetch().await.unwrap_err(), FetchError::TooLarge);
    assert!(!fixture
        .server
        .paths()
        .iter()
        .any(|path| path == BOT || path == MEDIA));

    let fixture = Fixture::new(Platform::Lark).await;
    fixture.server.set(
        MEDIA,
        Reply {
            status: 200,
            body: vec![0; 1025],
            content_type: "image/png",
            delay: Duration::ZERO,
        },
    );
    assert_eq!(fixture.fetch().await.unwrap_err(), FetchError::TooLarge);
    assert!(matches!(
        fixture.source.open(fixture.request()).await,
        Err(FetchError::TooLarge)
    ));
    fixture.server.set(
        BOT,
        Reply {
            delay: Duration::from_secs(1),
            ..Reply::json(json!({"code":0,"bot":{"open_id":"ou_source"}}))
        },
    );
    let mut request = fixture.request();
    request.deadline = Instant::now() + Duration::from_millis(50);
    let start = Instant::now();
    assert!(matches!(
        fixture.source.fetch(request).await,
        Err(FetchError::TimedOut)
    ));
    assert!(start.elapsed() < Duration::from_millis(500));
}

#[tokio::test]
async fn current_authority_revokes_old_locators_even_with_a_retained_cached_channel() {
    for change in ["key", "binding", "bot", "route", "removed"] {
        let mut fixture = Fixture::new(Platform::Lark).await;
        match change {
            "key" => {
                fixture.definition.secrets.channel_key =
                    Some(Secret::new("rotated-channel-fixture"))
            }
            "removed" => {
                if let Source::NyxidRelay { lark_resources, .. } =
                    &mut fixture.definition.sources[0]
                {
                    *lark_resources = None;
                }
            }
            _ => {
                if let Source::NyxidRelay {
                    lark_resources: Some(binding),
                    bot_id,
                    route_ids,
                    ..
                } = &mut fixture.definition.sources[0]
                {
                    match change {
                        "binding" => binding.user_service_id = "connection-two".into(),
                        "bot" => *bot_id = Some("ou_new".into()),
                        "route" => *route_ids = vec!["new-route".into()],
                        _ => unreachable!(),
                    }
                }
            }
        }
        fixture.store(2);
        assert_eq!(
            fixture.fetch().await.unwrap_err(),
            FetchError::Unauthorized,
            "{change}"
        );
        assert!(
            !fixture
                .server
                .paths()
                .iter()
                .any(|path| path.starts_with("/api/v1/proxy")),
            "{change}"
        );
    }
}

#[tokio::test]
async fn unrelated_channel_edits_preserve_resource_authority() {
    let mut fixture = Fixture::new(Platform::Lark).await;
    fixture.definition.name = "Renamed channel".into();
    fixture.definition.reply.ack_after_ms = 0;
    fixture.definition.sources.push(serde_json::from_value(json!({"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks","key_id":"key-1","route_ids":["another-route"],"platform":"feishu","admission":{"type":"all"}})).unwrap());
    fixture.definition.validate().unwrap();
    fixture.store(2);
    assert_eq!(fixture.fetch().await.unwrap(), b"opaque-image-fixture");
}

#[tokio::test]
async fn forged_region_reply_and_record_authority_do_not_reach_nyxid() {
    for change in ["region", "route", "record"] {
        let mut fixture = Fixture::new(Platform::Lark).await;
        match change {
            "record" => fixture.event.record_id = "different-record".parse().unwrap(),
            _ => {
                if let ReplyAddress::NyxidRelay {
                    platform, route_id, ..
                } = &mut fixture.event.reply
                {
                    if change == "region" {
                        *platform = Platform::Feishu;
                    } else {
                        *route_id = "different-route".into();
                    }
                }
            }
        }
        assert_eq!(fixture.fetch().await.unwrap_err(), FetchError::Unauthorized);
        assert!(!fixture
            .server
            .paths()
            .iter()
            .any(|path| path.starts_with("/api/v1/proxy")));
    }
}

#[tokio::test]
async fn eager_derivative_cache_cannot_bypass_channel_key_rotation() {
    use crate::media::{
        budget::InflightBytes,
        derive::{Artifact, DeriveError, Derived},
        jobs::JobOutcome,
        service::{MediaDeps, MediaService},
        source::SourceRegistry,
        target::MediaTarget,
        transcribe::{meter::SttSpend, upload::NoTranscoder, DisabledTranscriber},
        visual::PlatformThumbnail,
        MediaLimits,
    };
    let mut fixture = Fixture::new(Platform::Lark).await;
    let mut png = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut png, ::image::ImageFormat::Png)
        .unwrap();
    fixture.server.set(
        MEDIA,
        Reply {
            status: 200,
            body: png.into_inner(),
            content_type: "image/png",
            delay: Duration::ZERO,
        },
    );
    let sources = Arc::new(SourceRegistry::new(
        fixture.source.clone(),
        fixture.source.clone(),
        fixture.source.clone(),
        fixture.source.clone(),
    ));
    let decode = Arc::new(tokio::sync::Semaphore::new(1));
    let limits = MediaLimits::from_config(&crate::MediaConfig::default());
    let stt = crate::SttConfig::default();
    let transcriber = Arc::new(DisabledTranscriber);
    let service = MediaService::new(MediaDeps {
        visuals: Arc::new(PlatformThumbnail::new(sources.clone(), decode.clone())),
        sources,
        transcoder: Arc::new(NoTranscoder::new(&stt, transcriber.clone())),
        transcriber,
        stt: Arc::new(SttSpend::new(&stt)),
        limits,
        keys: fixture.keys.clone(),
        tool_providers: Arc::new(crate::MediaToolProviders::default()),
        decode,
        inflight: InflightBytes::new(&limits),
    })
    .with_resolver(fixture.resolver.clone());
    let target = MediaTarget::new(
        fixture.channel.clone(),
        fixture.event.clone(),
        Arc::new(fixture.activity.clone()),
    )
    .unwrap();
    for _ in 0..2 {
        let result = service
            .derive(&target, 0, Artifact::Image, Duration::from_secs(5))
            .await;
        assert!(
            matches!(result, JobOutcome::Ready(ref value) if matches!(&**value, Derived::Image(_))),
            "{result:?}"
        );
    }
    assert_eq!(
        fixture
            .server
            .paths()
            .iter()
            .filter(|path| path.as_str() == MEDIA)
            .count(),
        1,
        "second request must be cached"
    );
    fixture.definition.name = "Renamed without revoking media".into();
    fixture.store(2);
    assert!(matches!(
        service
            .derive(&target, 0, Artifact::Image, Duration::from_secs(5))
            .await,
        JobOutcome::Ready(_)
    ));
    fixture.definition.secrets.channel_key = Some(Secret::new("rotated-fixture"));
    fixture.store(3);
    assert!(matches!(
        service
            .derive(&target, 0, Artifact::Image, Duration::from_secs(5))
            .await,
        JobOutcome::Failed(DeriveError::SourceUnavailable)
    ));
    assert_eq!(
        fixture
            .server
            .paths()
            .iter()
            .filter(|path| path.as_str() == MEDIA)
            .count(),
        1,
        "revoked cache access must not refetch under another key"
    );
}

fn shared_metadata(fixture: &Fixture, source: Value) {
    let (mut services, _, _) = super::tests::metadata();
    services["services"][0]["credential_source"] = source;
    fixture
        .server
        .set("/api/v1/user-services", Reply::json(services));
    fixture
        .server
        .set("/api/v1/endpoints", Reply::json(json!({"endpoints":[]})));
}
const AUTHORIZATION: &str = "/api/v1/endpoints/endpoint-one/authorization";
fn shared_evidence() -> Value {
    json!({"id":"endpoint-one","catalog_service_id":"catalog-one","auto_connected":true,"updated_at":"2026-10-27T00:00:00Z"})
}

#[tokio::test]
async fn shared_auto_connected_endpoint_uses_exact_channel_key_and_retains_bot_proof() {
    let fixture = Fixture::new(Platform::Lark).await;
    shared_metadata(
        &fixture,
        json!({"type":"org","allowed":true,"role":"member"}),
    );
    fixture
        .server
        .set(AUTHORIZATION, Reply::json(shared_evidence()));
    assert_eq!(fixture.fetch().await.unwrap(), b"opaque-image-fixture");
    assert!(fixture
        .server
        .requests
        .lock()
        .unwrap()
        .contains(&(AUTHORIZATION.into(), format!("Bearer {KEY}"))));
    fixture.server.set(
        BOT,
        Reply::json(json!({"code":0,"bot":{"open_id":"ou_other"}})),
    );
    assert_eq!(fixture.fetch().await.unwrap_err(), FetchError::Unauthorized);
    assert_eq!(
        fixture
            .server
            .paths()
            .iter()
            .filter(|path| path.as_str() == MEDIA)
            .count(),
        1
    );
}

#[tokio::test]
async fn shared_endpoint_denials_mismatches_and_custom_origins_never_reach_provider() {
    for change in [
        "denied",
        "wrong-id",
        "wrong-catalog",
        "custom",
        "custom-url",
        "viewer",
        "disallowed",
    ] {
        let fixture = Fixture::new(Platform::Lark).await;
        let mut source = json!({"type":"org","allowed":true,"role":"member"});
        if change == "viewer" {
            source["role"] = json!("viewer");
        }
        if change == "disallowed" {
            source["allowed"] = json!(false);
        }
        shared_metadata(&fixture, source);
        let mut evidence = shared_evidence();
        match change {
            "wrong-id" => evidence["id"] = json!("other"),
            "wrong-catalog" => evidence["catalog_service_id"] = json!("other"),
            "custom" => evidence["auto_connected"] = json!(false),
            "custom-url" => evidence["url"] = json!("https://unrelated.example"),
            _ => {}
        }
        fixture.server.set(
            AUTHORIZATION,
            Reply {
                status: if change == "denied" { 403 } else { 200 },
                ..Reply::json(evidence)
            },
        );
        assert_eq!(
            fixture.fetch().await.unwrap_err(),
            FetchError::Unauthorized,
            "{change}"
        );
        assert!(
            !fixture
                .server
                .paths()
                .iter()
                .any(|path| path == BOT || path == MEDIA),
            "{change}"
        );
        if matches!(change, "viewer" | "disallowed") {
            assert!(!fixture
                .server
                .paths()
                .iter()
                .any(|path| path == AUTHORIZATION));
        }
    }
}
