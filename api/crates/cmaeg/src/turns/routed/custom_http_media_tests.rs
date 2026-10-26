//! Public signed Custom HTTP through routed admission, real inline media and tool reads.
use super::{custom_http_fixture::App, test_support::fixture, Phase, TargetState};
use crate::{
    media::{
        budget::InflightBytes,
        service::{MediaDeps, MediaService},
        source::{inline::InlineSource, SourceRegistry},
        transcribe::{meter::SttSpend, upload::NoTranscoder, DisabledTranscriber},
        visual::PlatformThumbnail,
        MediaLimits,
    },
    MediaConfig, MediaToolProviders, SttConfig,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Semaphore;

fn inline_media() -> Arc<MediaService> {
    // Only the real inline reader is available: this fixture cannot fetch a URL
    // or satisfy another platform's media locator with invented bytes.
    let inline = Arc::new(InlineSource);
    let sources = Arc::new(SourceRegistry::new(
        inline.clone(),
        inline.clone(),
        inline.clone(),
        inline,
    ));
    let decode = Arc::new(Semaphore::new(1));
    let transcriber = Arc::new(DisabledTranscriber);
    let stt = SttConfig::default();
    let limits = MediaLimits::from_config(&MediaConfig::default());
    Arc::new(MediaService::new(MediaDeps {
        visuals: Arc::new(PlatformThumbnail::new(sources.clone(), decode.clone())),
        sources,
        transcriber: transcriber.clone(),
        transcoder: Arc::new(NoTranscoder::new(&stt, transcriber)),
        stt: Arc::new(SttSpend::new(&stt)),
        limits,
        keys: Arc::new(fixture::keys()),
        tool_providers: Arc::new(MediaToolProviders::from_iter(["cma_codex"
            .parse()
            .unwrap()])),
        decode,
        inflight: InflightBytes::new(&limits),
    }))
}

#[tokio::test]
async fn public_custom_inline_attachment_survives_routed_provider_and_authenticated_tool_read() {
    let mut app = App::configured(
        true,
        |channel| {
            channel["sources"][0]["attachments"] = json!({"pointer":"/attachments","origins":[]});
        },
        Some(inline_media()),
    )
    .await;
    let original = "Routed inline report: build 42 passed all checks.";
    let encoded = STANDARD.encode(original);
    let mut event = app.event("custom-routed-inline");
    event["data"]["attachments"] = json!([{
        "url":format!("data:text/plain;base64,{encoded}"),
        "filename":"report.txt",
        "size_bytes":original.len(),
    }]);
    let body = event.to_string();
    let response = app.request(body.clone(), true).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let stream = response.text().await.unwrap();
    assert_eq!(
        stream.matches("event: cmaeg.completed").count(),
        1,
        "{stream}"
    );
    assert!(!stream.contains("cmaeg.failed"), "{stream}");
    app.drain().await;
    assert!(app.waiters.is_empty());

    let plans = app.plans().await;
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].source_body, body.as_bytes());
    let state = app
        .journal
        .read::<TargetState>(&plans[0].targets[0].id)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(matches!(state.phase, Phase::Completed { .. }));
    let posts: Vec<_> = app
        .provider
        .seen()
        .into_iter()
        .filter(|request| request.method == "POST")
        .collect();
    assert_eq!(
        posts.len(),
        1,
        "the routed scheduler must launch exactly once"
    );
    let post: Value = serde_json::from_slice(&posts[0].body).unwrap();
    let input = post["input"].to_string();
    for expected in [
        original,
        "report.txt",
        "[Attachments: 1",
        "readEventAttachment",
    ] {
        assert!(input.contains(expected), "missing {expected}: {input}");
    }
    assert!(!input.contains(&encoded));
    assert!(!input.contains("data:text/plain"));
    let context = post["event_context"].clone();
    let attachment = &context["activity"]["kind"]["attachments"][0];
    assert_eq!(
        attachment["source"],
        json!({"type":"inline","pointer":"/attachments/0/url"})
    );
    assert_eq!(attachment["size_bytes"], original.len());
    assert_eq!(attachment["filename"], "report.txt");
    assert_eq!(context["activity"]["source"]["type"], "custom");

    // The tool reads the exact provider snapshot created by that admitted turn.
    app.provider.memory.lock().unwrap().config.event_context = Some(context.clone());
    let base = reqwest::Url::parse(&app.url)
        .unwrap()
        .origin()
        .ascii_serialization();
    let reference = context["event_ref"].as_str().unwrap();
    let tool = format!("{base}/v1/events/{reference}/attachments/0?view=text");
    let key = super::custom_http_fixture::AGENT_KEY;
    let client = reqwest::Client::new();
    let read = client.get(&tool).bearer_auth(key).send().await.unwrap();
    let status = read.status();
    let view: Value = read.json().await.unwrap();
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["status"], "ready");
    assert_eq!(view["text"]["content"], original);
    assert_eq!(view["filename"], "report.txt");
    assert_eq!(view["text"]["source"], "document");
    let reads: Vec<_> = app
        .provider
        .seen()
        .into_iter()
        .filter(|r| {
            r.method == "GET" && r.uri.contains("/conversations/") && r.uri.contains("/events/")
        })
        .collect();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].bearer, format!("Bearer {key}"));

    // A provider cannot replace a retained inline file under the same reference,
    // even if the legitimate excerpt is already cached from preparation.
    let mut tampered = context;
    tampered["activity"]["kind"]["ce"]["data"]["attachments"][0]["url"] =
        "data:text/plain;base64,dGFtcGVyZWQ=".into();
    app.provider.memory.lock().unwrap().config.event_context = Some(tampered);
    let rejected = client.get(&tool).bearer_auth(key).send().await.unwrap();
    let status = rejected.status();
    let problem: Value = rejected.json().await.unwrap();
    assert_eq!(status, 503, "{problem}");
    assert_eq!(problem["detail"], "event_context_unavailable");
}
