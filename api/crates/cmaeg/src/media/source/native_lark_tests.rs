use super::*;
#[path = "native_lark_test_support.rs"]
mod support;
use cmaeg_core::{
    activity::SourceKind,
    channel::{LarkRegion, Secret, Source},
};
use support::*;

#[tokio::test]
async fn regional_resources_bind_exact_message_key_type_and_verified_bot_token() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        let f = Fixture::new(region).await;
        assert_eq!(f.fetch().await.unwrap(), BYTES);
        let calls = f.mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls
            .iter()
            .all(|(url, _)| url.starts_with(region.origin())));
        assert_eq!(calls[2].0, format!("{}/open-apis/im/v1/messages/om_media/resources/00000000-1111-2222-3333-44444444444g?type=file", region.origin()));
        assert!(calls[0].1.is_empty());
        assert!(calls[1..]
            .iter()
            .all(|(_, auth)| auth == &format!("Bearer {TOKEN}")));
    }
}

#[tokio::test]
async fn changed_exact_source_custody_revokes_before_any_provider_call() {
    for change in [
        "source",
        "app",
        "tenant",
        "bot",
        "region",
        "admission",
        "app_secret",
        "encrypt_key",
        "verification_token",
        "provider",
        "agent",
        "owner",
    ] {
        let mut f = Fixture::new(LarkRegion::Feishu).await;
        match change {
            "source" => f.definition.sources.clear(),
            "app_secret" => {
                f.definition
                    .secrets
                    .native_lark
                    .as_mut()
                    .unwrap()
                    .app_secret = Secret::new("fixture-rotated-app-secret")
            }
            "encrypt_key" => {
                f.definition
                    .secrets
                    .native_lark
                    .as_mut()
                    .unwrap()
                    .encrypt_key = Secret::new("fixture-rotated-encrypt-key")
            }
            "verification_token" => {
                f.definition
                    .secrets
                    .native_lark
                    .as_mut()
                    .unwrap()
                    .verification_token = Secret::new("fixture-rotated-verification")
            }
            "provider" => {
                f.definition.provider.binding_id = "new-provider-binding".parse().unwrap()
            }
            "agent" => f.definition.secrets.agent_key_hash = format!("sha256:{}", "0".repeat(64)),
            "owner" => f.definition.owner.subject = "other-owner".into(),
            _ => {
                let Source::NativeLark(s) = &mut f.definition.sources[0] else {
                    panic!()
                };
                match change {
                    "app" => s.identity.app_id = "cli_other".into(),
                    "tenant" => s.identity.tenant_key = "other_tenant".into(),
                    "bot" => s.identity.bot_open_id = "ou_other".into(),
                    "region" => s.identity.region = LarkRegion::Lark,
                    _ => {
                        s.admission = cmaeg_core::channel::AdmissionPolicy::Allowlist {
                            sender_ids: vec!["ou_other".into()],
                        }
                    }
                }
            }
        }
        f.store();
        assert_eq!(
            f.fetch().await.unwrap_err(),
            FetchError::Unauthorized,
            "{change}"
        );
        assert!(f.mock.calls.lock().unwrap().is_empty(), "{change}");
    }
}

#[tokio::test]
async fn cached_derivatives_still_require_current_native_source_and_consumer_authority() {
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
    let mut f = Fixture::new(LarkRegion::Feishu).await;
    let sources = Arc::new(
        SourceRegistry::new(
            f.source.clone(),
            f.source.clone(),
            f.source.clone(),
            f.source.clone(),
        )
        .with_native_lark(f.source.clone()),
    );
    let decode = Arc::new(tokio::sync::Semaphore::new(1));
    let stt = crate::SttConfig::default();
    let transcriber = Arc::new(DisabledTranscriber);
    let limits = MediaLimits::from_config(&crate::MediaConfig::default());
    let service = MediaService::new(MediaDeps {
        visuals: Arc::new(PlatformThumbnail::new(sources.clone(), decode.clone())),
        sources,
        transcriber: transcriber.clone(),
        transcoder: Arc::new(NoTranscoder::new(&stt, transcriber)),
        stt: Arc::new(SttSpend::new(&stt)),
        limits,
        keys: f.keys.clone(),
        tool_providers: Arc::new(crate::MediaToolProviders::default()),
        decode,
        inflight: InflightBytes::new(&limits),
    })
    .with_resolver(f.resolver.clone());
    let target = MediaTarget::new(
        f.channel.clone(),
        f.event.clone(),
        Arc::new(f.activity.clone()),
    )
    .unwrap();
    for _ in 0..2 {
        let result = service
            .derive(&target, 0, Artifact::Excerpt, Duration::from_secs(5))
            .await;
        assert!(
            matches!(result, JobOutcome::Ready(ref value) if matches!(value.as_ref(), Derived::Excerpt { text, .. } if text.as_bytes() == BYTES)),
            "{result:?}"
        );
    }
    assert_eq!(
        f.mock.calls.lock().unwrap().len(),
        3,
        "a second read uses the derivative cache"
    );
    f.definition.name = "Rename keeps the same authority".into();
    f.store();
    assert!(matches!(
        service
            .derive(&target, 0, Artifact::Excerpt, Duration::from_secs(5))
            .await,
        JobOutcome::Ready(_)
    ));
    f.definition
        .secrets
        .native_lark
        .as_mut()
        .unwrap()
        .verification_token = Secret::new("fixture-rotated-verification");
    f.store();
    assert!(matches!(
        service
            .derive(&target, 0, Artifact::Excerpt, Duration::from_secs(5))
            .await,
        JobOutcome::Failed(DeriveError::SourceUnavailable)
    ));
    assert_eq!(
        f.mock.calls.lock().unwrap().len(),
        3,
        "revoked cached reads perform no provider request"
    );
}

#[tokio::test]
async fn sealed_message_key_activity_and_reply_forgery_never_reach_api() {
    for change in ["message", "chat", "region", "digest", "locator", "record"] {
        let mut f = Fixture::new(LarkRegion::Feishu).await;
        match change {
            "record" => f.event.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap(),
            "digest" => f.event.activity_sha256 = Some("0".repeat(64)),
            "locator" => {
                let cmaeg_core::activity::ActivityKind::Message { attachments, .. } =
                    &mut f.activity.kind
                else {
                    panic!()
                };
                let Some(AttachmentSource::NativeLark { resource_key, .. }) =
                    &mut attachments[0].source
                else {
                    panic!()
                };
                *resource_key = "other_key".into();
            }
            _ => {
                let ReplyAddress::NativeLark(r) = &mut f.event.reply else {
                    panic!()
                };
                match change {
                    "message" => r.message_id = "om_other".into(),
                    "chat" => r.chat_id = "oc_other".into(),
                    _ => r.identity.region = LarkRegion::Lark,
                }
            }
        }
        assert_eq!(
            f.fetch().await.unwrap_err(),
            FetchError::Unauthorized,
            "{change}"
        );
        assert!(f.mock.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn rotation_during_token_or_resource_request_never_returns_bytes() {
    for during_token in [true, false] {
        let f = Fixture::new(LarkRegion::Feishu).await;
        if during_token {
            *f.mock.token_hook.lock().unwrap() = Some(f.rotate_hook());
        } else {
            *f.mock.media_hook.lock().unwrap() = Some(f.rotate_hook());
        }
        assert_eq!(f.fetch().await.unwrap_err(), FetchError::Unauthorized);
        assert_eq!(
            f.mock.calls.lock().unwrap().len(),
            if during_token { 2 } else { 3 }
        );
    }
}

#[tokio::test]
async fn resource_errors_caps_custody_and_unrelated_changes_are_explicit() {
    for (status, error) in [
        (302, FetchError::Rejected),
        (400, FetchError::Rejected),
        (401, FetchError::Unauthorized),
        (403, FetchError::Unauthorized),
        (404, FetchError::NotFound),
        (429, FetchError::RateLimited),
        (500, FetchError::Upstream),
    ] {
        let f = Fixture::new(LarkRegion::Feishu).await;
        *f.mock.status.lock().unwrap() = status;
        assert_eq!(f.fetch().await.unwrap_err(), error);
        assert_eq!(f.mock.calls.lock().unwrap().len(), 3);
    }
    let mut f = Fixture::new(LarkRegion::Feishu).await;
    f.definition.name = "Renamed".into();
    f.store();
    assert_eq!(f.fetch().await.unwrap(), BYTES);
    assert_eq!(
        f.source
            .fetch(FetchRequest {
                cap: 2,
                ..f.request()
            })
            .await
            .unwrap_err(),
        FetchError::TooLarge
    );
    assert_eq!(
        &f.source
            .fetch(FetchRequest {
                cap: 2,
                overflow: crate::media::body::Overflow::Truncate,
                ..f.request()
            })
            .await
            .unwrap()
            .bytes[..],
        &BYTES[..2]
    );
    *f.mock.bot.lock().unwrap() = "ou_other".into();
    assert_eq!(f.fetch().await.unwrap_err(), FetchError::Unauthorized);
    assert!(matches!(f.activity.source, SourceKind::NativeLark(_)));
}

#[tokio::test]
async fn streamed_resources_remain_capped_and_stalled_transports_obey_deadlines() {
    let f = Fixture::new(LarkRegion::Feishu).await;
    let mut opened = f.source.open(f.request()).await.unwrap();
    let mut bytes = Vec::new();
    while let Some(chunk) = opened.stream.next().await {
        bytes.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(bytes, BYTES);
    assert_eq!(
        f.source
            .open(FetchRequest {
                cap: 2,
                ..f.request()
            })
            .await
            .unwrap_err(),
        FetchError::TooLarge
    );
    struct Stalled;
    impl Transport for Stalled {
        fn get<'a>(
            &'a self,
            _: &'a Url,
            _: &'a cmaeg_core::channel::Secret,
            _: Instant,
        ) -> BoxFuture<'a, Result<Response, FetchError>> {
            Box::pin(std::future::pending())
        }
    }
    let source = NativeLarkFileSource {
        client: f.source.client.clone(),
        resolver: f.resolver.clone(),
        transport: Arc::new(Stalled),
    };
    assert_eq!(
        source
            .fetch(FetchRequest {
                deadline: Instant::now() + Duration::from_millis(50),
                ..f.request()
            })
            .await
            .unwrap_err(),
        FetchError::TimedOut
    );
}
