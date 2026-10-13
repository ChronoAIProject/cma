//! Eager media preparation: inline transcripts and excerpts for every
//! binding class within the budget, explicit pending and failure lines,
//! background warm-up for tool-bearing agents only, and the media label.
use super::{
    preparation_support::{config, item, last_input, message, mp4, voice, Media, Speech, WORDS},
    support::Fixture,
};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    media::{
        derive::{Artifact, Derived},
        jobs::JobOutcome,
        prepare::prepare,
        FetchError,
    },
    telemetry::timing::{IngressTiming, MediaLabel},
    turns::{
        render_input,
        render_media::{Audience, InlineNotice, RenderMedia},
    },
    TurnConfig,
};
use cmaeg_core::{
    activity::{ActivityKind, AttachmentKind, MediaVariant},
    channel::{ChannelDefinition, ProviderTarget},
    profile::ActionMode,
};
use std::time::Duration;

const BUDGET: Duration = Duration::from_secs(5);

/// Rebinds the fixture channel to one binding class.
type Rebind = fn(&mut ChannelDefinition);

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn deadline(after: Duration) -> tokio::time::Instant {
    tokio::time::Instant::now() + after
}

/// The input text `render_input::render` produces for `request` and `media`.
fn rendered(
    request: &cmaeg::ingress::launch::LaunchRequest,
    media: &RenderMedia,
    audience: Audience,
) -> String {
    render_input::render(
        &request.activity,
        &request.channel.definition,
        media,
        audience,
    )[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The fence of a transcript, whatever the turn's nonce.
fn fenced(input: &str, kind: &str, index: usize) -> bool {
    input.contains("<media-") && input.contains(&format!(" {kind} [{index}]>"))
}

#[tokio::test]
async fn voice_note_transcript_for_every_binding_class() {
    let media = Media::new(Speech::Words, BUDGET).await;
    let classes: [(&str, Rebind, bool); 4] = [
        ("guarded", |_| {}, true),
        (
            "autonomous",
            |definition| definition.profile.permission.action_mode = ActionMode::Autonomous,
            true,
        ),
        (
            "existing",
            |definition| {
                definition.provider.target = ProviderTarget::ExistingAgent {
                    agent_id: "agent-1".into(),
                }
            },
            false,
        ),
        (
            "nyxbot",
            |definition| definition.provider.slug = "nyxbot".parse().unwrap(),
            false,
        ),
    ];
    for (class, edit, tools) in classes {
        let items = vec![voice(&media, 0, Some(4))];
        let input = media.input(media.turn(class, items, edit)).await;
        assert!(fenced(&input, "transcript", 0), "{class}: {input}");
        assert!(input.contains(WORDS), "{class}: {input}");
        assert_eq!(
            input.contains("readEventAttachment"),
            tools,
            "{class}: {input}"
        );
    }
    assert_eq!(media.words.calls(), 4);
}

#[tokio::test]
async fn slow_transcript_renders_pending_then_fills_cache() {
    let media = Media::new(Speech::Words, Duration::from_millis(300)).await;
    media.source.delay(Duration::from_secs(1));
    let items = vec![voice(&media, 0, Some(4))];
    let input = media.input(media.turn("slow", items.clone(), |_| {})).await;
    assert!(
        input.contains("transcript is still being prepared"),
        "{input}"
    );
    assert!(!input.contains(WORDS), "{input}");
    let again = media.turn("slow", items, |_| {});
    let target = media.service.target_for_turn(&again, now()).unwrap();
    let outcome = media
        .service
        .derive(&target, 0, Artifact::Transcript, Duration::from_secs(5))
        .await;
    assert!(matches!(
        outcome,
        JobOutcome::Ready(ref derived) if matches!(**derived, Derived::Transcript { .. })
    ));
    assert_eq!(
        media.source.calls(),
        1,
        "the background job filled the cache"
    );
    assert_eq!(media.words.calls(), 1);
}

#[tokio::test]
async fn nyxid_404_renders_not_found() {
    let media = Media::new(Speech::Words, BUDGET).await;
    let mut voice_note = voice(&media, 0, Some(4));
    media.source.serve(0, Err(FetchError::NotFound));
    voice_note.mime_type = None;
    let input = media
        .input(media.turn("missing", vec![voice_note], |_| {}))
        .await;
    assert!(
        input.contains("transcript unavailable: not_found"),
        "{input}"
    );
    assert!(!input.contains("</media-"), "no fence: {input}");
}

#[tokio::test]
async fn disabled_transcriber_renders_reason_without_fetch() {
    let media = Media::new(Speech::Disabled, BUDGET).await;
    let items = vec![voice(&media, 0, Some(4))];
    let input = media.input(media.turn("disabled", items, |_| {})).await;
    assert!(
        input.contains("transcript unavailable: transcriber_disabled"),
        "{input}"
    );
    assert_eq!(media.source.calls(), 0);
}

#[tokio::test]
async fn small_text_file_is_excerpted() {
    let media = Media::new(Speech::Words, BUDGET).await;
    media
        .source
        .serve(0, Ok(b"meeting notes: ship on friday".to_vec()));
    let mut notes = item(0, AttachmentKind::File, Some(MediaVariant::Document));
    notes.filename = Some("notes.txt".into());
    notes.mime_type = Some("text/plain".into());
    notes.size_bytes = Some(29);
    let input = media.input(media.turn("notes", vec![notes], |_| {})).await;
    assert!(fenced(&input, "excerpt", 0), "{input}");
    assert!(input.contains("meeting notes: ship on friday"), "{input}");
    assert_eq!(media.source.calls(), 1);
}

#[tokio::test]
async fn pdf_is_not_fetched_eagerly() {
    let media = Media::new(Speech::Words, BUDGET).await;
    let mut report = item(0, AttachmentKind::File, Some(MediaVariant::Document));
    report.filename = Some("report.pdf".into());
    report.mime_type = Some("application/pdf".into());
    report.size_bytes = Some(20_480);
    let input = media.input(media.turn("pdf", vec![report], |_| {})).await;
    assert!(input.contains("file \"report.pdf\""), "{input}");
    assert!(!input.contains("</media-"), "no fence: {input}");
    assert_eq!(media.source.calls(), 0);
}

#[tokio::test]
async fn long_declared_audio_is_not_eager() {
    let media = Media::new(Speech::Words, BUDGET).await;
    let items = vec![voice(&media, 0, Some(400))];
    let input = media.input(media.turn("long", items, |_| {})).await;
    assert!(input.contains("[0] voice message"), "{input}");
    assert!(!input.contains("</media-"), "no fence: {input}");
    assert!(!input.contains("transcript"), "{input}");
    assert_eq!((media.source.calls(), media.words.calls()), (0, 0));
}

#[tokio::test]
async fn animation_is_never_transcribed() {
    let media = Media::new(Speech::Words, BUDGET).await;
    media.source.serve(0, Ok(mp4()));
    let mut clip = item(0, AttachmentKind::Video, Some(MediaVariant::Animation));
    clip.duration_seconds = Some(3);
    let input = media
        .input(media.turn("animation", vec![clip], |_| {}))
        .await;
    assert!(!input.contains("</media-"), "no fence: {input}");
    assert_eq!((media.source.calls(), media.words.calls()), (0, 0));
}

#[tokio::test]
async fn video_note_without_duration_is_eager() {
    let media = Media::new(Speech::Words, BUDGET).await;
    media.source.serve(0, Ok(mp4()));
    let note = item(0, AttachmentKind::Video, Some(MediaVariant::VideoNote));
    let input = media
        .input(media.turn("video-note", vec![note], |_| {}))
        .await;
    assert!(fenced(&input, "transcript", 0), "{input}");
    assert!(input.contains(WORDS), "{input}");
    assert_eq!(media.words.calls(), 1);
}

#[tokio::test]
async fn photo_warms_once_for_tool_bearing_only() {
    let media = Media::new(Speech::Words, BUDGET).await;
    media.source.serve(0, Ok(vec![0xFF, 0xD8, 0xFF, 0xE0]));
    media.source.delay(Duration::from_secs(2));
    let photo = item(0, AttachmentKind::Image, Some(MediaVariant::Photo));
    let started = std::time::Instant::now();
    let input = media
        .input(media.turn("warm", vec![photo.clone()], |_| {}))
        .await;
    assert!(started.elapsed() < Duration::from_secs(1), "not awaited");
    assert!(input.contains("readEventAttachment"), "{input}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while media.source.calls() < 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let existing = |definition: &mut ChannelDefinition| {
        definition.provider.target = ProviderTarget::ExistingAgent {
            agent_id: "agent-1".into(),
        }
    };
    let other = Media::new(Speech::Words, BUDGET).await;
    other.source.serve(0, Ok(vec![0xFF, 0xD8, 0xFF, 0xE0]));
    other.input(other.turn("cold", vec![photo], existing)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(other.source.calls(), 0, "no warm-up without tools");
    assert_eq!(media.source.calls(), 1, "warmed once");
}

#[tokio::test]
async fn text_turn_does_no_media_work() {
    let media = Media::new(Speech::Words, BUDGET).await;
    let mut request = media.fixture.request("plain");
    let timing = IngressTiming::new("custom");
    request.timing = Some(timing.clone());
    let outcome = media.runner.launch(request).await;
    assert!(matches!(outcome, LaunchOutcome::Accepted { .. }));
    assert_eq!((media.source.calls(), media.words.calls()), (0, 0));
    assert_eq!(timing.media().as_str(), "none");
}

#[tokio::test]
async fn labels_prepared_and_fenced() {
    let media = Media::new(Speech::Words, BUDGET).await;
    media.source.delay(Duration::from_millis(300));
    let mut first = media.turn("labelled", vec![voice(&media, 0, Some(4))], |_| {});
    let mut second = media.fixture.request("labelled-text");
    second.conversation_id = first.conversation_id.clone();
    let (prepared, fenced) = (IngressTiming::new("custom"), IngressTiming::new("custom"));
    first.timing = Some(prepared.clone());
    second.timing = Some(fenced.clone());
    let runner = media.runner.clone();
    let launch = tokio::spawn(async move { runner.launch(first).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(matches!(
        media.runner.launch(second).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert!(matches!(
        launch.await.unwrap(),
        LaunchOutcome::Accepted { .. }
    ));
    assert_eq!(prepared.media(), MediaLabel::Prepared);
    assert_eq!(fenced.media(), MediaLabel::Fenced);
    let inputs: Vec<bool> = media
        .fixture
        .fake
        .requests()
        .into_iter()
        .filter(|request| request.method == "POST")
        .map(|request| request.body.contains(WORDS))
        .collect();
    assert_eq!(
        inputs,
        [true, false],
        "the voice note reached the provider first"
    );
}

#[tokio::test]
async fn tampered_event_ref_renders_not_retrievable_and_starts() {
    let media = Media::new(Speech::Words, BUDGET).await;
    let mut request = media.turn("tampered", vec![voice(&media, 0, Some(4))], |_| {});
    if let ActivityKind::Message { text, .. } = &mut request.activity.kind {
        // The sealed digest no longer matches the activity.
        text.push_str(" (edited)");
    }
    let input = media.input(request).await;
    assert!(
        input.contains("transcript unavailable: not_retrievable"),
        "{input}"
    );
    assert_eq!(media.source.calls(), 0);
}

#[tokio::test]
async fn missing_nonce_renders_descriptors_without_inline_text() {
    let media = Media::new(Speech::Words, BUDGET).await;
    let request = media.turn("no-nonce", vec![voice(&media, 0, Some(4))], |_| {});
    let audience = Audience {
        tools: false,
        downloads: false,
    };
    let prepared = prepare(
        &media.service,
        &request,
        audience,
        &config(BUDGET),
        None,
        deadline(BUDGET),
        now(),
    )
    .await;
    assert!(!prepared.eager);
    assert_eq!((media.source.calls(), media.words.calls()), (0, 0));
    let input = rendered(&request, &prepared.media, audience);
    assert!(input.contains("[0] voice message"), "{input}");
    assert!(!input.contains("<media-"), "{input}");
    assert!(!input.contains(WORDS), "{input}");
}

#[tokio::test]
async fn excerpt_and_untruncated_transcript_have_no_notice() {
    let audience = Audience {
        tools: false,
        downloads: false,
    };
    let media = Media::new(Speech::Words, BUDGET).await;
    media.source.serve(1, Ok(b"line one".to_vec()));
    let mut notes = item(1, AttachmentKind::File, None);
    notes.mime_type = Some("text/plain".into());
    let request = media.turn("notice", vec![voice(&media, 0, Some(4)), notes], |_| {});
    let prepared = prepare(
        &media.service,
        &request,
        audience,
        &config(BUDGET),
        Some("0123456789ab".into()),
        deadline(BUDGET),
        now(),
    )
    .await;
    assert!(prepared.eager);
    assert_eq!(prepared.media.inline.len(), 2);
    assert!(prepared
        .media
        .inline
        .iter()
        .all(|item| item.notice.is_none()));

    let truncating = Media::new(Speech::FirstMinute, BUDGET).await;
    let request = truncating.turn("notice", vec![voice(&truncating, 0, None)], |_| {});
    let prepared = prepare(
        &truncating.service,
        &request,
        audience,
        &config(BUDGET),
        Some("0123456789ab".into()),
        deadline(BUDGET),
        now(),
    )
    .await;
    assert_eq!(
        prepared.media.inline[0].notice,
        Some(InlineNotice::AudioTruncated { seconds: 65 })
    );
    let input = rendered(&request, &prepared.media, audience);
    assert!(
        input.contains("[transcript covers the first 1:05]"),
        "{input}"
    );

    // The transformer truncates at T, ten minutes at the default limits.
    let transformed = Media::new(Speech::Transformer, BUDGET).await;
    let request = transformed.turn("long", vec![voice(&transformed, 0, None)], |_| {});
    let prepared = prepare(
        &transformed.service,
        &request,
        audience,
        &config(BUDGET),
        Some("0123456789ab".into()),
        deadline(BUDGET),
        now(),
    )
    .await;
    assert_eq!(
        prepared.media.inline[0].notice,
        Some(InlineNotice::AudioTruncated { seconds: 600 })
    );
    let input = rendered(&request, &prepared.media, audience);
    assert!(
        input.contains("[transcript covers the first 10:00]"),
        "{input}"
    );
}

#[tokio::test]
async fn media_disabled_renders_descriptors_only() {
    let mut fixture = Fixture::new(Default::default(), TurnConfig::default(), 4).await;
    let voice_note = message(vec![item(
        0,
        AttachmentKind::Audio,
        Some(MediaVariant::Voice),
    )]);
    let outcome = fixture
        .runner
        .launch(fixture.request_with_kind("disabled-media", Some(voice_note)))
        .await;
    assert!(matches!(outcome, LaunchOutcome::Accepted { .. }));
    fixture.terminal().await;
    let input = last_input(&fixture);
    assert!(input.contains("[Attachments: 1."), "{input}");
    assert!(!input.contains("</media-"), "no fence: {input}");
    assert!(!input.contains("transcript"), "{input}");
    let text = fixture.request("disabled-text");
    let expected = rendered(
        &text,
        &RenderMedia::empty(None),
        Audience::of(&text.channel.consumer(None).unwrap(), &Default::default()),
    );
    assert!(matches!(
        fixture.runner.launch(text).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert_eq!(last_input(&fixture), expected);
}
