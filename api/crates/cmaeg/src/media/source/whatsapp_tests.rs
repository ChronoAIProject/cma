use super::*;
#[path = "whatsapp_test_support.rs"]
mod support;
use futures_util::TryStreamExt;
use serde_json::json;
use support::*;

#[test]
fn download_origin_and_exact_media_identity_cannot_redirect_credentials() {
    assert!(whatsapp_transport::download_url(CDN, "45678").is_ok());
    for raw in [
        "https://lookaside.fbsbx.com.evil.example/whatsapp_business/attachments/?mid=45678",
        "http://lookaside.fbsbx.com/whatsapp_business/attachments/?mid=45678",
        "https://user@lookaside.fbsbx.com/whatsapp_business/attachments/?mid=45678",
        "https://lookaside.fbsbx.com:8443/whatsapp_business/attachments/?mid=45678",
        "https://lookaside.fbsbx.com/whatsapp_business/attachments/?mid=99999",
        "https://lookaside.fbsbx.com/whatsapp_business/attachments/?mid=45678&mid=45678",
        "https://lookaside.fbsbx.com/whatsapp_business/attachments/?mid=45678#secret",
        "https://lookaside.fbsbx.com/other/?mid=45678",
        "https://graph.facebook.com/whatsapp_business/attachments/?mid=45678",
    ] {
        assert_eq!(
            whatsapp_transport::download_url(raw, "45678"),
            Err(FetchError::Rejected)
        );
    }
}

#[tokio::test]
async fn source_uses_fresh_phone_bound_metadata_and_authenticates_both_official_reads() {
    let f = Fixture::new();
    f.transport.success();
    f.transport.success();
    for _ in 0..2 {
        assert_eq!(&f.source.fetch(f.request()).await.unwrap().bytes[..], BYTES);
    }
    let calls = f.transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    for pair in calls.chunks_exact(2) {
        assert_eq!(
            pair[0].0,
            "https://graph.facebook.com/v25.0/45678?phone_number_id=34567"
        );
        assert_eq!(pair[1].0, CDN);
        assert_eq!(pair[0].1, pair[1].1);
        assert_eq!(
            pair[0].1,
            crate::whatsapp::tests::credentials().access_token.as_str()
        );
    }
}

#[tokio::test]
async fn metadata_must_match_signed_hash_media_and_mime_before_download() {
    for (field, value) in [
        ("id", json!("99999")),
        ("messaging_product", json!("other")),
        ("sha256", json!("11".repeat(32))),
        ("mime_type", json!("image/png")),
        ("file_size", json!("-1")),
        ("url", json!("https://evil.example/file")),
    ] {
        let f = Fixture::new();
        let mut meta = metadata();
        meta[field] = value;
        f.transport.meta(meta);
        assert_eq!(
            f.source.fetch(f.request()).await.unwrap_err(),
            FetchError::Rejected
        );
        assert_eq!(f.transport.calls.lock().unwrap().len(), 1);
    }
    let f = Fixture::new();
    f.transport.meta(metadata());
    assert_eq!(
        f.source
            .fetch(FetchRequest {
                cap: 3,
                ..f.request()
            })
            .await
            .unwrap_err(),
        FetchError::TooLarge
    );
    assert_eq!(f.transport.calls.lock().unwrap().len(), 1);
    let f = Fixture::new();
    let mut meta = metadata();
    meta["file_size"] = json!(MAX_BYTES + 1);
    f.transport.meta(meta);
    assert_eq!(
        f.source.fetch(f.request()).await.unwrap_err(),
        FetchError::TooLarge
    );
}

#[tokio::test]
async fn source_requires_exact_sealed_app_account_phone_message_actor_and_current_custody() {
    for field in 0..5 {
        let mut f = Fixture::new();
        let ReplyAddress::Whatsapp {
            app_id,
            waba_id,
            phone_number_id,
            recipient_id,
            message_id,
        } = &mut f.event.reply
        else {
            panic!()
        };
        *match field {
            0 => app_id,
            1 => waba_id,
            2 => phone_number_id,
            3 => recipient_id,
            _ => message_id,
        } = "99999".into();
        assert_eq!(
            f.source.fetch(f.request()).await.unwrap_err(),
            FetchError::NotRetrievable
        );
        assert!(f.transport.calls.lock().unwrap().is_empty());
    }
    let mut f = Fixture::new();
    f.activity.actor.id = "99999".into();
    f.event.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(&f.activity));
    assert_eq!(
        f.source.fetch(f.request()).await.unwrap_err(),
        FetchError::NotRetrievable
    );
    let mut f = Fixture::new();
    f.event.activity_sha256 = None;
    assert_eq!(
        f.source.fetch(f.request()).await.unwrap_err(),
        FetchError::NotRetrievable
    );
    let f = Fixture::new();
    let other = AttachmentSource::Whatsapp {
        message_id: "wamid.media".into(),
        media_id: "99999".into(),
        sha256: "11".repeat(32),
    };
    assert_eq!(
        f.source
            .fetch(FetchRequest {
                locator: Some(&other),
                ..f.request()
            })
            .await
            .unwrap_err(),
        FetchError::NotRetrievable
    );
    for remove_source in [false, true] {
        let mut f = Fixture::new();
        let channel = Arc::make_mut(&mut f.channel);
        if remove_source {
            channel.definition.sources.clear()
        } else {
            channel.definition.secrets.whatsapp = None
        }
        assert!(!configured(&f.channel, &f.activity.source));
        assert_eq!(
            f.source.fetch(f.request()).await.unwrap_err(),
            FetchError::Unauthorized
        );
        assert!(f.transport.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn stale_url_is_refreshed_once_and_statuses_remain_typed_without_upstream_details() {
    let f = Fixture::new();
    f.transport.meta(metadata());
    f.transport.bytes(404, b"secret upstream error");
    f.transport.success();
    assert_eq!(&f.source.fetch(f.request()).await.unwrap().bytes[..], BYTES);
    assert_eq!(f.transport.calls.lock().unwrap().len(), 4);
    let f = Fixture::new();
    for _ in 0..2 {
        f.transport.meta(metadata());
        f.transport.bytes(404, b"private");
    }
    assert_eq!(
        f.source.fetch(f.request()).await.unwrap_err(),
        FetchError::NotFound
    );
    assert_eq!(f.transport.calls.lock().unwrap().len(), 4);
    for (status, expected) in [
        (302, FetchError::Rejected),
        (401, FetchError::Unauthorized),
        (403, FetchError::Unauthorized),
        (429, FetchError::RateLimited),
        (503, FetchError::Busy),
        (500, FetchError::Upstream),
    ] {
        let f = Fixture::new();
        f.transport.meta(metadata());
        f.transport.bytes(status, b"private-token-url");
        assert_eq!(f.source.fetch(f.request()).await.unwrap_err(), expected);
        assert_eq!(f.transport.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn byte_integrity_is_checked_for_full_reads_and_streams_and_excerpts_stay_capped() {
    let f = Fixture::new();
    f.transport.success();
    let body = f
        .source
        .fetch(FetchRequest {
            cap: 4,
            overflow: Overflow::Truncate,
            ..f.request()
        })
        .await
        .unwrap();
    assert_eq!(&body.bytes[..], &BYTES[..4]);
    assert!(body.truncated);
    f.transport.success();
    let chunks: Vec<_> = f
        .source
        .open(f.request())
        .await
        .unwrap()
        .stream
        .try_collect()
        .await
        .unwrap();
    assert_eq!(chunks.concat(), BYTES);
    f.transport.meta(metadata());
    f.transport.bytes(200, &[0; BYTES.len()]);
    assert_eq!(
        f.source.fetch(f.request()).await.unwrap_err(),
        FetchError::Rejected
    );
    f.transport.meta(metadata());
    f.transport.bytes(200, &[0; BYTES.len()]);
    assert_eq!(
        f.source
            .open(f.request())
            .await
            .unwrap()
            .stream
            .try_collect::<Vec<_>>()
            .await
            .unwrap_err(),
        FetchError::Rejected
    );
    f.transport.meta(metadata());
    f.transport.bytes(200, b"short");
    assert_eq!(
        f.source.fetch(f.request()).await.unwrap_err(),
        FetchError::Rejected
    );
}

#[tokio::test]
async fn metadata_is_capped_and_a_stalled_transport_obeys_the_same_request_deadline() {
    let f = Fixture::new();
    f.transport
        .bytes(200, &vec![b' '; METADATA_CAP as usize + 1]);
    assert_eq!(
        f.source.fetch(f.request()).await.unwrap_err(),
        FetchError::TooLarge
    );
    struct Stalled;
    impl Transport for Stalled {
        fn get<'a>(
            &'a self,
            _: &'a Url,
            _: &'a Secret,
            _: Instant,
        ) -> BoxFuture<'a, Result<Response, FetchError>> {
            Box::pin(std::future::pending())
        }
    }
    let source = WhatsappFileSource {
        transport: Arc::new(Stalled),
    };
    assert_eq!(
        source
            .fetch(FetchRequest {
                deadline: Instant::now() + Duration::from_millis(10),
                ..f.request()
            })
            .await
            .unwrap_err(),
        FetchError::TimedOut
    );
}

#[tokio::test]
async fn native_locator_uses_the_shared_media_service_text_preparation_and_cache() {
    use crate::media::{
        budget::InflightBytes,
        derive::{Artifact, Derived},
        jobs::JobOutcome,
        service::{MediaDeps, MediaService},
        source::SourceRegistry,
        target::MediaTarget,
        transcribe::{meter::SttSpend, upload::NoTranscoder, DisabledTranscriber},
        visual::PlatformThumbnail,
        MediaLimits,
    };
    let f = Fixture::new();
    f.transport.success();
    let source = Arc::new(WhatsappFileSource {
        transport: f.transport.clone(),
    });
    let sources = Arc::new(
        SourceRegistry::new(
            source.clone(),
            source.clone(),
            source.clone(),
            source.clone(),
        )
        .with_whatsapp(source),
    );
    let decode = Arc::new(tokio::sync::Semaphore::new(1));
    let transcriber = Arc::new(DisabledTranscriber);
    let stt = crate::SttConfig::default();
    let limits = MediaLimits::from_config(&crate::MediaConfig::default());
    let service = MediaService::new(MediaDeps {
        visuals: Arc::new(PlatformThumbnail::new(sources.clone(), decode.clone())),
        sources,
        transcriber: transcriber.clone(),
        transcoder: Arc::new(NoTranscoder::new(&stt, transcriber)),
        stt: Arc::new(SttSpend::new(&stt)),
        limits,
        keys: Arc::new(crate::ingress::routing_plan::fixture::keys()),
        tool_providers: Arc::new(crate::MediaToolProviders::default()),
        decode,
        inflight: InflightBytes::new(&limits),
    });
    let target = MediaTarget::new(
        f.channel.clone(),
        f.event.clone(),
        Arc::new(f.activity.clone()),
    )
    .unwrap();
    for _ in 0..2 {
        let JobOutcome::Ready(value) = service
            .derive(&target, 0, Artifact::Excerpt, Duration::from_secs(2))
            .await
        else {
            panic!("media must derive")
        };
        assert!(
            matches!(value.as_ref(), Derived::Excerpt { text, truncated: false } if text.as_bytes() == BYTES)
        );
    }
    assert_eq!(
        f.transport.calls.lock().unwrap().len(),
        2,
        "the second read reuses the shared derived cache"
    );
}
