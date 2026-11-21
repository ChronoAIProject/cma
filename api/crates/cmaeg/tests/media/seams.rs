//! Media targets, the derivation vocabulary and the extraction seams.
use super::{
    fakes::{
        attachment, custom_target, message, target, telegram_file, telegram_source, url,
        FakeSource, Seen,
    },
    image_fixtures as fixtures,
    support::{activity, event, resolved},
};
use cmaeg::{
    media::{
        body::Overflow,
        derive::{Artifact, DeriveError},
        image::ImageError,
        jobs::JobAbort,
        sniff::sniff,
        source::SourceCredential,
        target::{MediaTarget, TargetError},
        text::TextError,
        transcribe::{
            AudioContainer, AudioInput, DisabledTranscriber, TranscribeError, Transcriber,
        },
        visual::{
            PlatformThumbnail, VisualExtractor, VisualKind, VisualRequest, THUMBNAIL_MAX_BYTES,
        },
        FetchError,
    },
    telemetry::channel_hash,
};
use cmaeg_core::{
    activity::{ActivityKind, AttachmentKind, SourceKind},
    channel::{Platform, Secret},
    cloudevent::CloudEvent,
    ids::ChannelId,
    seal::{event_ref::activity_digest, ReplyAddress},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{sync::Semaphore, time::Instant};

const REPLY_TOKEN: &str = "reply-token-must-stay-sealed";

fn relayed() -> (
    Arc<cmaeg::channels::resolver::ResolvedChannel>,
    cmaeg_core::seal::EventRefPayload,
    cmaeg_core::activity::Activity,
) {
    let activity = activity(
        SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            route_id: "route-1".into(),
        },
        Some(message(vec![attachment(AttachmentKind::Image, None)])),
    );
    let mut payload = event(ReplyAddress::NyxidRelay {
        thread_reply: false,
        message_id: "nyx-message-1".into(),
        route_id: "route-1".into(),
        platform: Platform::Telegram,
        reply_token: Secret::new(REPLY_TOKEN),
        reply_token_expires_at: i64::MAX,
    });
    payload.activity_sha256 = Some(activity_digest(&activity));
    (Arc::new(resolved(|_| {})), payload, activity)
}

#[test]
fn a_target_pairs_an_activity_with_the_event_ref_that_sealed_it() {
    let (channel, payload, activity) = relayed();
    let target =
        MediaTarget::new(channel.clone(), payload.clone(), Arc::new(activity.clone())).unwrap();
    assert_eq!(target.event, payload);

    let mut foreign = payload.clone();
    foreign.channel_id = ChannelId::new("cmaeg1.ch.someone-else").unwrap();
    assert_eq!(
        MediaTarget::new(channel.clone(), foreign, Arc::new(activity.clone())).unwrap_err(),
        TargetError::ForeignChannel
    );

    let mut tampered = activity.clone();
    if let ActivityKind::Message { attachments, .. } = &mut tampered.kind {
        attachments[0].source = Some(url("swapped"));
    }
    assert_eq!(
        MediaTarget::new(channel.clone(), payload.clone(), Arc::new(tampered)).unwrap_err(),
        TargetError::DigestMismatch
    );

    let mut unsealed = payload;
    unsealed.activity_sha256 = None;
    assert_eq!(
        MediaTarget::new(channel, unsealed, Arc::new(activity)).unwrap_err(),
        TargetError::Unverifiable
    );
    assert_eq!(TargetError::Unverifiable.to_string(), "unverifiable");
}

#[test]
fn a_target_debug_shows_neither_the_reply_token_nor_identifiers() {
    let (channel, payload, activity) = relayed();
    let target = MediaTarget::new(channel, payload, Arc::new(activity)).unwrap();
    let shown = format!("{target:?}");
    assert!(!shown.contains(REPLY_TOKEN), "{shown}");
    assert!(!shown.contains("nyx-message-1"), "{shown}");
    assert!(!shown.contains(target.channel.id.as_str()), "{shown}");
    assert!(
        shown.contains(&channel_hash(target.channel.id.as_str())),
        "{shown}"
    );
    assert!(shown.contains("msg-1"), "{shown}");
}

#[test]
fn attachments_are_found_on_messages_and_events_by_index() {
    let first = attachment(AttachmentKind::Image, Some(url("a")));
    let second = attachment(AttachmentKind::File, Some(url("b")));
    let on_message = custom_target(vec![first.clone(), second.clone()]);
    assert_eq!(on_message.attachment(0), Some(&first));
    assert_eq!(on_message.attachment(1), Some(&second));
    assert_eq!(on_message.attachment(2), None);

    let ce = CloudEvent {
        specversion: "1.0".into(),
        id: "ce-1".into(),
        source: "/fixtures".into(),
        r#type: "com.example.upload".into(),
        time: None,
        subject: None,
        datacontenttype: None,
        dataschema: None,
        data: None,
        extensions: BTreeMap::new(),
    };
    let on_event = target(
        SourceKind::Custom,
        ActivityKind::Event {
            ce,
            attachments: vec![second.clone()],
        },
    );
    assert_eq!(on_event.attachment(0), Some(&second));
    assert_eq!(on_event.attachment(1), None);

    let command = target(
        SourceKind::Custom,
        ActivityKind::Command {
            name: "start".into(),
            args: String::new(),
        },
    );
    assert_eq!(command.attachment(0), None);
}

#[test]
fn cache_keys_separate_attachments_and_artifacts() {
    let target = custom_target(vec![attachment(AttachmentKind::Image, Some(url("a")))]);
    assert_eq!(target.cache_key(0, "image"), target.cache_key(0, "image"));
    assert_ne!(target.cache_key(0, "image"), target.cache_key(1, "image"));
    assert_ne!(target.cache_key(0, "image"), target.cache_key(0, "text"));
}

#[test]
fn every_reason_has_its_agent_facing_code() {
    let reasons = [
        (DeriveError::NotRetrievable, "not_retrievable"),
        (DeriveError::NotFound, "not_found"),
        (DeriveError::TooLarge, "too_large"),
        (DeriveError::TooLargePixels, "too_large_pixels"),
        (DeriveError::UnsupportedFormat, "unsupported_format"),
        (DeriveError::DecodeFailed, "decode_failed"),
        (DeriveError::TimedOut, "timed_out"),
        (DeriveError::TranscriberDisabled, "transcriber_disabled"),
        (DeriveError::TranscriberFailed, "transcriber_failed"),
        (DeriveError::TooLong, "too_long"),
        (DeriveError::RateLimited, "rate_limited"),
        (DeriveError::SourceUnavailable, "source_unavailable"),
        (DeriveError::TransformFailed, "transform_failed"),
    ];
    for (reason, code) in reasons {
        assert_eq!(reason.as_str(), code);
        assert_eq!(reason.to_string(), code);
    }
    let tags: Vec<_> = [
        Artifact::Image,
        Artifact::Text,
        Artifact::Excerpt,
        Artifact::Transcript,
        Artifact::Visuals,
        Artifact::Metadata,
    ]
    .iter()
    .map(Artifact::tag)
    .collect();
    assert_eq!(
        tags,
        [
            "image",
            "text",
            "excerpt",
            "transcript",
            "visuals",
            "metadata"
        ]
    );
}

#[test]
fn lower_level_failures_map_into_one_vocabulary() {
    let fetches = [
        (FetchError::NotRetrievable, DeriveError::NotRetrievable),
        (FetchError::TooLarge, DeriveError::TooLarge),
        (FetchError::NotFound, DeriveError::NotFound),
        (FetchError::TimedOut, DeriveError::TimedOut),
        (FetchError::RateLimited, DeriveError::RateLimited),
        (FetchError::Busy, DeriveError::SourceUnavailable),
        (FetchError::Unauthorized, DeriveError::SourceUnavailable),
        (FetchError::Rejected, DeriveError::SourceUnavailable),
        (FetchError::Upstream, DeriveError::SourceUnavailable),
    ];
    for (fetch, derived) in fetches {
        assert_eq!(DeriveError::from(fetch), derived, "{fetch:?}");
    }
    let images = [
        (
            ImageError::UnsupportedFormat,
            DeriveError::UnsupportedFormat,
        ),
        (ImageError::TooLargePixels, DeriveError::TooLargePixels),
        (ImageError::DecodeFailed, DeriveError::DecodeFailed),
        (ImageError::TooLarge, DeriveError::TooLarge),
        (ImageError::TimedOut, DeriveError::TimedOut),
    ];
    for (image, derived) in images {
        assert_eq!(DeriveError::from(image), derived, "{image:?}");
    }
    assert_eq!(
        DeriveError::from(TextError::NotText),
        DeriveError::UnsupportedFormat
    );
    assert_eq!(DeriveError::from(JobAbort::TimedOut), DeriveError::TimedOut);
    assert_eq!(DeriveError::from(JobAbort::Busy), DeriveError::RateLimited);
    let transcriptions = [
        (TranscribeError::Disabled, DeriveError::TranscriberDisabled),
        (TranscribeError::Unsupported, DeriveError::UnsupportedFormat),
        (TranscribeError::TimedOut, DeriveError::TimedOut),
        (TranscribeError::RateLimited, DeriveError::RateLimited),
        (TranscribeError::Failed, DeriveError::TranscriberFailed),
    ];
    for (transcription, derived) in transcriptions {
        assert_eq!(DeriveError::from(transcription), derived);
    }
}

fn padded(prefix: &[u8]) -> Vec<u8> {
    let mut out = prefix.to_vec();
    out.resize(64, 0);
    out
}

#[test]
fn audio_containers_follow_the_sniffed_type_of_real_headers() {
    let mut opus = padded(b"OggS\0\x02");
    opus[28..36].copy_from_slice(b"OpusHead");
    let webm = padded(&[
        0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, 0x01, 0x42, 0xF7, 0x81, 0x01, 0x42, 0xF2,
        0x81, 0x04, 0x42, 0xF3, 0x81, 0x08, 0x42, 0x82, 0x84, b'w', b'e', b'b', b'm',
    ]);
    let cases = [
        (padded(b"ID3\x04\0\0\0\0\0\0"), Some(AudioContainer::Mp3)),
        (
            padded(b"\0\0\0\x20ftypM4A \0\0\0\0M4A mp42isom"),
            Some(AudioContainer::M4a),
        ),
        (
            padded(b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2"),
            Some(AudioContainer::Mp4),
        ),
        (
            padded(b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0"),
            Some(AudioContainer::Wav),
        ),
        (webm, Some(AudioContainer::Webm)),
        (opus, Some(AudioContainer::Ogg)),
        (padded(b"OggS\0\x02"), Some(AudioContainer::Ogg)),
        (padded(b"fLaC\0\0\0\x22"), Some(AudioContainer::Flac)),
        (padded(b"#!AMR\n"), None),
        (padded(b"\0\0\0\x14ftypqt  \0\0\0\0qt  "), None),
    ];
    for (header, expected) in cases {
        let mime = sniff(&header).mime;
        assert_eq!(AudioContainer::from_sniffed(mime), expected, "{mime}");
    }
    let names: Vec<_> = [
        AudioContainer::Mp3,
        AudioContainer::Mp4,
        AudioContainer::M4a,
        AudioContainer::Wav,
        AudioContainer::Webm,
        AudioContainer::Ogg,
        AudioContainer::Flac,
    ]
    .iter()
    .map(|container| {
        assert_eq!(container.extension(), container.format());
        (container.format(), container.upload_mime())
    })
    .collect();
    assert_eq!(
        names,
        [
            ("mp3", "audio/mpeg"),
            ("mp4", "video/mp4"),
            ("m4a", "audio/mp4"),
            ("wav", "audio/wav"),
            ("webm", "audio/webm"),
            ("ogg", "audio/ogg"),
            ("flac", "audio/flac"),
        ]
    );
}

#[tokio::test]
async fn the_disabled_transcriber_refuses_everything() {
    let transcriber = DisabledTranscriber;
    assert!(!transcriber.enabled());
    assert!(!transcriber.accepts(AudioContainer::Mp3));
    let input = AudioInput {
        bytes: bytes::Bytes::from_static(b"ID3"),
        container: AudioContainer::Mp3,
    };
    let refused = transcriber
        .transcribe(input, Instant::now() + Duration::from_secs(1))
        .await;
    assert_eq!(refused.unwrap_err(), TranscribeError::Disabled);
}

fn video_with_thumbnail(thumbnail: Option<&str>) -> MediaTarget {
    let mut video = attachment(AttachmentKind::Video, Some(telegram_file("video")));
    video.thumbnail = thumbnail.map(telegram_file);
    target(telegram_source(), message(vec![video]))
}

async fn extract(
    source: &Arc<FakeSource>,
    target: &MediaTarget,
    index: usize,
) -> Result<usize, DeriveError> {
    let extractor = PlatformThumbnail::new(source.registry(), Arc::new(Semaphore::new(1)));
    assert_eq!(extractor.kind(), VisualKind::Thumbnail);
    let frames = extractor
        .extract(VisualRequest {
            target,
            index,
            deadline: Instant::now() + Duration::from_secs(30),
        })
        .await?;
    for frame in &frames {
        assert!(frame.bytes.len() <= 160 * 1024);
        assert!(frame.width.max(frame.height) <= 768);
        assert_eq!(frame.at_ms, None);
    }
    Ok(frames.len())
}

#[tokio::test]
async fn a_platform_thumbnail_is_one_preview_frame() {
    let source = FakeSource::new();
    let jpeg = fixtures::jpeg(&fixtures::rgb(1280, 720, 16), 90);
    source.serve(&telegram_file("thumb"), Ok(jpeg));
    let target = video_with_thumbnail(Some("thumb"));
    assert_eq!(extract(&source, &target, 0).await, Ok(1));
    assert_eq!(
        source.seen(),
        [Seen {
            cap: THUMBNAIL_MAX_BYTES,
            overflow: Overflow::Reject,
            credential: SourceCredential::ReplyToken,
        }]
    );
}

#[tokio::test]
async fn a_missing_thumbnail_or_attachment_is_refused_without_fetching() {
    let source = FakeSource::new();
    let target = video_with_thumbnail(None);
    assert_eq!(
        extract(&source, &target, 0).await,
        Err(DeriveError::NotRetrievable)
    );
    assert_eq!(
        extract(&source, &target, 3).await,
        Err(DeriveError::NotFound)
    );
    assert_eq!(source.calls(), 0);
}

#[tokio::test]
async fn thumbnail_fetch_and_decode_failures_map_to_reasons() {
    let target = video_with_thumbnail(Some("thumb"));
    for (served, expected) in [
        (Err(FetchError::Busy), DeriveError::SourceUnavailable),
        (Err(FetchError::NotFound), DeriveError::NotFound),
        (Ok(vec![0; 2 * 1024 * 1024]), DeriveError::TooLarge),
        (
            Ok(b"not an image at all".to_vec()),
            DeriveError::UnsupportedFormat,
        ),
    ] {
        let source = FakeSource::new();
        source.serve(&telegram_file("thumb"), served);
        assert_eq!(extract(&source, &target, 0).await, Err(expected));
        assert_eq!(source.calls(), 1);
    }
}
