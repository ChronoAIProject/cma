//! The media service's admission: per-channel quotas and fairness, warm-ups,
//! turn targets and composition.
use super::{
    fakes::{attachment, custom_target, message, target_on, url, FakeSource},
    service::{
        failed, files, is_pending, limits, ready, ring, service, service_with, LONG, REPLY_TOKEN,
    },
    support::{activity, event, resolved, unix_now},
};
use cmaeg::{
    ingress::launch::LaunchRequest,
    media::{
        derive::{Artifact, DeriveError},
        service::{compose, ComposeDeps, MediaCapabilities},
        target::TargetError,
        transcribe::DisabledTranscriber,
        transform::{
            jobdir::MediaRoot,
            launcher::{JobLauncher, JobResult, JobSpec},
            outcome::TransformOutcome,
            setup::TransformerRuntime,
        },
        visual::VisualKind,
        MediaLimits,
    },
    telegram::TelegramClient,
    ChannelCacheConfig, Config, DeliveryConfig, IngressConfig, ManagementConfig, MediaConfig,
    MediaToolProviders, RoutingConfig, SttConfig, TransformerConfig, TurnConfig,
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, AttachmentKind, SourceKind},
    channel::{Platform, Secret},
    ids::{ChannelId, IdempotencyKey},
    seal::{event_ref::activity_digest, EventRef, ReplyAddress},
};
use futures_util::future::BoxFuture;
use std::{sync::Arc, time::Duration};

#[tokio::test(start_paused = true)]
async fn only_new_jobs_are_charged_to_the_fetch_quota() {
    let source = FakeSource::new();
    let target = files(
        &source,
        &[
            ("a", b"a".to_vec()),
            ("b", b"b".to_vec()),
            ("c", b"c".to_vec()),
        ],
    );
    let service = service_with(
        &source,
        MediaLimits {
            fetch_per_minute: 2,
            ..limits()
        },
        Arc::new(DisabledTranscriber),
    );
    ready(service.derive(&target, 0, Artifact::Text, LONG).await);
    ready(service.derive(&target, 1, Artifact::Text, LONG).await);
    assert_eq!(
        failed(service.derive(&target, 2, Artifact::Text, LONG).await),
        DeriveError::RateLimited
    );
    // A cache hit is not a new job.
    ready(service.derive(&target, 0, Artifact::Text, LONG).await);
    assert_eq!(source.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn joining_a_live_job_is_not_charged() {
    let source = FakeSource::new();
    source.delay(Duration::from_secs(5));
    let target = files(&source, &[("a", b"a".to_vec()), ("b", b"b".to_vec())]);
    let service = service_with(
        &source,
        MediaLimits {
            fetch_per_minute: 1,
            ..limits()
        },
        Arc::new(DisabledTranscriber),
    );
    assert!(is_pending(
        &service
            .derive(&target, 0, Artifact::Text, Duration::ZERO)
            .await
    ));
    assert!(is_pending(
        &service
            .derive(&target, 0, Artifact::Text, Duration::ZERO)
            .await
    ));
    assert_eq!(
        failed(service.derive(&target, 1, Artifact::Text, LONG).await),
        DeriveError::RateLimited
    );
    ready(service.derive(&target, 0, Artifact::Text, LONG).await);
    assert_eq!(source.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_third_job_on_one_channel_waits_while_another_channel_proceeds() {
    let source = FakeSource::new();
    source.delay(Duration::from_secs(10));
    let names = ["a", "b", "c"];
    for name in names {
        source.serve(&url(name), Ok(name.as_bytes().to_vec()));
    }
    let busy = custom_target(
        names
            .iter()
            .map(|name| attachment(AttachmentKind::File, Some(url(name))))
            .collect(),
    );
    let other = target_on(
        "media-other",
        SourceKind::Custom,
        message(vec![attachment(AttachmentKind::File, Some(url("a")))]),
    );
    let service = service(&source);
    for index in 0..3 {
        let outcome = service
            .derive(&busy, index, Artifact::Text, Duration::ZERO)
            .await;
        assert!(is_pending(&outcome));
    }
    assert!(is_pending(
        &service
            .derive(&other, 0, Artifact::Text, Duration::ZERO)
            .await
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(source.calls(), 3, "the third job did not wait for a slot");
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(source.calls(), 4);
    ready(service.derive(&busy, 2, Artifact::Text, LONG).await);
}

#[tokio::test(start_paused = true)]
async fn warming_fills_the_cache_unless_the_quota_is_spent() {
    let source = FakeSource::new();
    let target = files(&source, &[("a", b"a".to_vec()), ("b", b"b".to_vec())]);
    let service = service_with(
        &source,
        MediaLimits {
            fetch_per_minute: 1,
            ..limits()
        },
        Arc::new(DisabledTranscriber),
    );
    service.warm(&target, 0, Artifact::Text);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(source.calls(), 1);
    ready(service.derive(&target, 0, Artifact::Text, LONG).await);
    service.warm(&target, 1, Artifact::Text);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(source.calls(), 1, "a warm-up beyond the quota fetched");
}

fn relayed_turn(edit: impl FnOnce(&mut Activity)) -> (LaunchRequest, i64) {
    let mut activity = activity(
        SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            route_id: "route-1".into(),
        },
        Some(message(vec![attachment(AttachmentKind::Image, None)])),
    );
    let mut payload = event(ReplyAddress::NyxidRelay {
        thread_reply: false,
        message_id: "nyx-message-9".into(),
        route_id: "route-1".into(),
        platform: Platform::Telegram,
        reply_token: Secret::new(REPLY_TOKEN),
        reply_token_expires_at: i64::MAX,
    });
    payload.activity_sha256 = Some(activity_digest(&activity));
    let expires_at = payload.expires_at;
    let event_ref = EventRef::seal(&payload, &ring()).unwrap();
    edit(&mut activity);
    let turn = LaunchRequest {
        channel: Arc::new(resolved(|_| {})),
        conversation_id: payload.conversation_id.clone(),
        idempotency_key: IdempotencyKey::from_event(&activity.event_id),
        activity,
        event_ref,
        received_at: std::time::Instant::now(),
        trace: tracing::Span::none(),
        timing: None,
        consumer: None,
    };
    (turn, expires_at)
}

#[test]
fn turn_targets_open_and_verify_the_sealed_event_ref() {
    let source = FakeSource::new();
    let service = service(&source);
    let (turn, expires_at) = relayed_turn(|_| {});
    let target = service.target_for_turn(&turn, unix_now()).unwrap();
    assert_eq!(target.event.event_id, turn.activity.event_id);
    let shown = format!("{target:?} {service:?}");
    assert!(!shown.contains(REPLY_TOKEN), "{shown}");

    let (tampered, _) = relayed_turn(|activity| {
        if let ActivityKind::Message { text, .. } = &mut activity.kind {
            text.push_str(" (edited)");
        }
    });
    assert_eq!(
        service.target_for_turn(&tampered, unix_now()).unwrap_err(),
        TargetError::DigestMismatch
    );
    let (mut foreign, _) = relayed_turn(|_| {});
    let mut other = resolved(|_| {});
    other.id = ChannelId::new("cmaeg1.ch.another-channel").unwrap();
    foreign.channel = Arc::new(other);
    assert_eq!(
        service.target_for_turn(&foreign, unix_now()).unwrap_err(),
        TargetError::ForeignChannel
    );
    assert_eq!(
        service.target_for_turn(&turn, expires_at).unwrap_err(),
        TargetError::Unverifiable
    );
    let (mut garbled, _) = relayed_turn(|_| {});
    garbled.event_ref = "cmaeg1.ev.garbled".parse().unwrap();
    assert_eq!(
        service.target_for_turn(&garbled, unix_now()).unwrap_err(),
        TargetError::Unverifiable
    );
}

fn config(media: MediaConfig) -> Config {
    Config {
        profile_revisions_enabled: false,
        managed_telegram_setup_enabled: false,
        native_lark_writes_enabled: false,
        native_lark_intake_enabled: false,
        native_lark_controls_enabled: false,
        native_lark_forms_enabled: false,
        native_lark_media_enabled: false,
        native_lark_stickers_enabled: false,
        native_split_text_refs_enabled: false,
        native_telegram_media_edits_enabled: false,
        native_telegram_media_controls_enabled: false,
        native_telegram_media_control_edits_enabled: false,
        native_discord_media_edits_enabled: false,
        native_discord_media_controls_enabled: false,
        native_discord_media_card_edits_enabled: false,
        native_discord_media_card_replacements_enabled: false,
        native_discord_media_card_sets_enabled: false,
        native_lark_message_effects_enabled: false,
        native_lark_received_reactions_enabled: false,
        native_typing_enabled: false,
        native_whatsapp_writes_enabled: false,
        native_whatsapp_groups_writes_enabled: false,
        native_whatsapp_message_events_enabled: false,
        native_whatsapp_forms_enabled: false,
        native_whatsapp_flow_writes_enabled: false,
        native_whatsapp_media_enabled: false,
        native_lark_outbound_media_enabled: false,
        native_lark_sticker_replies_enabled: false,
        native_lark_media_controls_enabled: false,
        native_lark_multi_image_controls_enabled: false,
        native_lark_media_card_replacements_enabled: false,
        native_media_presentations_enabled: false,
        native_discord_voice_enabled: false,
        native_telegram_structured_replies_enabled: false,
        native_whatsapp_structured_replies_enabled: false,
        native_telegram_albums_enabled: false,
        native_lark_thread_start_enabled: false,
        native_whatsapp_outbound_media_enabled: false,
        native_whatsapp_interactions_enabled: false,
        native_whatsapp_media_controls_enabled: false,
        native_whatsapp_reactions_enabled: false,
        native_telegram_received_reactions_enabled: false,
        native_discord_received_reactions_enabled: false,
        native_whatsapp_templates_enabled: false,
        native_whatsapp_groups_enabled: false,
        native_bot_mentions_writes_enabled: false,
        github_enabled: false,
        thread_contract_writes_enabled: false,
        multi_consumer_writes_enabled: false,
        dynamic_sources_writes_enabled: false,
        relay_raw_verification_writes_enabled: false,
        lark_resource_writes_enabled: false,
        lark_message_effects_writes_enabled: false,
        native_discord_writes_enabled: false,
        native_telegram_message_events_writes_enabled: false,
        native_telegram_member_joins_enabled: false,
        relay_telegram_member_joins_enabled: false,
        native_discord_thread_history_enabled: false,
        native_telegram_topic_lifecycle_enabled: false,
        native_telegram_topic_creation_enabled: false,
        native_telegram_general_topic_lifecycle_enabled: false,
        native_discord_thread_lifecycle_enabled: false,
        native_lark_conversation_history_enabled: false,
        native_lark_mixed_history_enabled: false,
        native_discord_conversation_history_enabled: false,
        native_discord_reactions_enabled: false,
        native_discord: Default::default(),
        outbound_messages_writes_enabled: false,
        event_side_messages_enabled: false,
        listen: "127.0.0.1:0".parse().unwrap(),
        metrics_listen: "127.0.0.1:0".parse().unwrap(),
        providers: Default::default(),
        provider_connect_timeout: Duration::from_secs(1),
        provider_request_timeout: Duration::from_secs(1),
        provider_max_inflight: 1,
        management: ManagementConfig {
            console_public_url: None,
            public_url: "https://gateway.example".into(),
            public_api_url: "https://gateway.example".into(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            master_keys: Arc::new(ring()),
            cma_base_url: "https://cma.example".into(),
            cma_service_token: Secret::new("cma-service"),
            nyxid_base_url: "https://nyxid.example".into(),
            tools_nyxid_slug: "cmaeg".into(),
        },
        cma_service_account: None,
        channel_cache: ChannelCacheConfig::default(),
        ingress: IngressConfig::default(),
        turns: TurnConfig::default(),
        delivery: DeliveryConfig::default(),
        media,
        routing: RoutingConfig::default(),
        stt: SttConfig::default(),
        transformer: TransformerConfig::default(),
        console: cmaeg::ConsoleConfig::default(),
    }
}

fn compose_deps() -> ComposeDeps {
    ComposeDeps {
        resolver: cmaeg::channels::resolver::ChannelResolver::new(
            Arc::new(
                cmaeg::records::RecordClient::new("https://cma.example", "fixture-service")
                    .unwrap(),
            ),
            Arc::new(ring()),
            ChannelCacheConfig::default(),
        ),
        keys: Arc::new(ring()),
        tool_providers: Arc::new(MediaToolProviders::default()),
        telegram: Arc::new(
            TelegramClient::new("https://api.telegram.org", Duration::from_secs(1)).unwrap(),
        ),
        transformer: None,
    }
}

#[test]
fn composition_is_skipped_while_media_is_off() {
    let off = compose(&config(MediaConfig::default()), compose_deps()).unwrap();
    assert!(off.is_none());
    let on = compose(
        &config(MediaConfig {
            enabled: true,
            ..MediaConfig::default()
        }),
        compose_deps(),
    )
    .unwrap()
    .expect("media is on");
    assert_eq!(
        on.capabilities(),
        MediaCapabilities {
            transcriber_enabled: false,
            visuals: VisualKind::Thumbnail,
        }
    );
    let definition = resolved(|_| {}).definition;
    let audience = on.audience(&definition.consumer(None).unwrap());
    assert!(!audience.tools && !audience.downloads);
    assert_eq!(
        on.responses().available_bytes(),
        MediaConfig::default().response_bytes
    );
    assert!(on.limiter().slots.is_empty());
}

/// A launcher that is never reached by composition alone.
struct Unused;

impl JobLauncher for Unused {
    fn run(&self, _spec: JobSpec) -> BoxFuture<'_, JobResult> {
        Box::pin(async { JobResult::unavailable(TransformOutcome::SandboxUnavailable) })
    }
}

#[test]
fn a_ready_transformer_is_composed_behind_the_media_seams() {
    let config = config(MediaConfig {
        enabled: true,
        ..MediaConfig::default()
    });
    let mut deps = compose_deps();
    deps.transformer = Some(TransformerRuntime {
        launcher: Box::new(Unused),
        root: MediaRoot::new(std::env::temp_dir()),
    });
    let on = compose(&config, deps).unwrap().expect("media is on");
    assert_eq!(
        on.capabilities(),
        MediaCapabilities {
            transcriber_enabled: false,
            visuals: VisualKind::Keyframes,
        }
    );
}
