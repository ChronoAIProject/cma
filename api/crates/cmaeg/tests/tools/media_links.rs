//! Minting media links through `readEventAttachment` view `download`: only
//! for autonomous tool-bearing bindings, within quotas, delivered exactly
//! once inside a ready `curl` command, and never logged, prompted or echoed
//! by event context.
use super::{
    download_support::{autonomous, autonomous_custom, download, relay},
    media::support::{assert_absent, capture_logs, TRACE_ENV},
    media_support::{relayed, MediaSetup, PARTITION_MARKER},
    relay_support::now,
    support::App,
};
use cmaeg::turns::{render_input::render, render_media::RenderMedia};
use cmaeg_core::{
    activity::AttachmentKind,
    seal::{media_link, MediaLinkRef},
};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::Instant;

const READY_NOTICE: &str =
    "the command works once and expires at expires_at; run it in your workspace";
const NOT_PERMITTED_NOTICE: &str =
    "downloads are available only to autonomous agents with gateway tools";

fn photo() -> Vec<u8> {
    (0..48 * 1024).map(|index| (index % 239) as u8).collect()
}

/// A relayed photo at index 0 behind a binding shaped by `edit`.
fn photo_app(edit: fn(&mut Value)) -> MediaSetup {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Image)]).edit(edit);
    setup.fakes.source.serve(&relay(0), Ok(photo()));
    setup
}

fn nyxbot_autonomous(value: &mut Value) {
    autonomous(value);
    value["provider"]["slug"] = "nyxbot".into();
    value["profile"]["metadata"]["provider"] = "nyxbot".into();
}

/// `tools/call readEventAttachment {view:"download"}`: the serialized
/// response and the view in its first content item.
async fn mint(app: &App, index: u8) -> (String, Value) {
    let response = app
        .rpc(json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"readEventAttachment","arguments":{"event_ref":app.event_ref.as_str(),"index":index,"view":"download"}}}))
        .await;
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    let result = &serde_json::from_str::<Value>(&body).unwrap()["result"];
    assert_eq!(result["isError"], false, "{result}");
    assert!(result.get("structuredContent").is_none());
    let view = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    (body, view)
}

/// The same view through the REST twin.
async fn mint_rest(app: &App, index: &str) -> Value {
    let response = app.attachment(index, "?view=download").await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

/// The link inside a ready view's `curl` command.
fn token(view: &Value) -> String {
    let curl = view["download"]["curl"].as_str().unwrap();
    let start = curl.find("Bearer ").unwrap() + "Bearer ".len();
    let end = start + curl[start..].find('\'').unwrap();
    curl[start..end].to_owned()
}

fn not_permitted(body: &str, view: &Value) {
    assert_eq!(
        (view["status"].as_str(), view["reason"].as_str()),
        (Some("unavailable"), Some("download_not_permitted"))
    );
    assert_eq!(view["notice"], NOT_PERMITTED_NOTICE);
    assert!(view.get("download").is_none());
    assert!(!body.contains("cmaeg1.md."), "{body}");
}

#[tokio::test]
async fn mint_then_download_end_to_end() {
    let app = App::with_media(photo_app(autonomous)).await;
    let (_, view) = mint(&app, 0).await;
    assert_eq!(view["status"], "ready", "{view}");
    assert!(view.get("reason").is_none());
    assert_eq!(view["notice"], READY_NOTICE);
    let link = token(&view);
    let ready = &view["download"];
    assert_eq!(ready["method"], "GET");
    assert_eq!(ready["url"], "https://gateway.example/v1/media/content");
    assert_eq!(
        ready["curl"],
        format!("curl -fsSL -H 'Authorization: Bearer {link}' -o 'attachment.bin' 'https://gateway.example/v1/media/content'")
    );
    let expires_at = ready["expires_at"].as_i64().unwrap();
    assert!((now() + 110..=now() + 120).contains(&expires_at));
    // Running the command's request: the exact bytes, then never again.
    let first = download(&app, &link).await;
    assert_eq!(first.status(), 200);
    assert_eq!(first.bytes().await.unwrap(), photo());
    assert_eq!(download(&app, &link).await.status(), 410);
    // The REST twin returns the same shape.
    let rest = mint_rest(&app, "0").await;
    assert_eq!(rest["status"], "ready");
    assert_ne!(token(&rest), link);
}

#[tokio::test]
async fn token_appears_exactly_once_in_tool_result() {
    let app = App::with_media(photo_app(autonomous)).await;
    let (body, view) = mint(&app, 0).await;
    let link = token(&view);
    assert_eq!(body.matches(&link).count(), 1);
    assert_eq!(body.matches("cmaeg1.md.").count(), 1);
    assert!(!view["download"]["url"].as_str().unwrap().contains(&link));
}

#[tokio::test]
async fn guarded_binding_gets_download_not_permitted() {
    let app = App::with_media(photo_app(|_| {})).await;
    let (body, view) = mint(&app, 0).await;
    not_permitted(&body, &view);
    let rest = mint_rest(&app, "0").await;
    not_permitted(&rest.to_string(), &rest);
}

#[tokio::test]
async fn nyxbot_binding_gets_download_not_permitted() {
    let app = App::with_media(photo_app(nyxbot_autonomous)).await;
    let rest = mint_rest(&app, "0").await;
    not_permitted(&rest.to_string(), &rest);
}

#[tokio::test]
async fn an_attachment_without_a_source_is_not_retrievable() {
    let mut item = relayed(0, AttachmentKind::File);
    item.source = None;
    let app = App::with_media(MediaSetup::new(vec![item]).edit(autonomous)).await;
    let view = mint_rest(&app, "0").await;
    assert_eq!(
        (view["status"].as_str(), view["reason"].as_str()),
        (Some("unavailable"), Some("not_retrievable"))
    );
    assert!(view.get("download").is_none());
}

#[tokio::test]
async fn mint_quota_per_channel() {
    let app = App::with_media(photo_app(autonomous)).await;
    for _ in 0..30 {
        assert_eq!(mint_rest(&app, "0").await["status"], "ready");
    }
    let limited = mint_rest(&app, "0").await;
    assert_eq!(
        (limited["status"].as_str(), limited["reason"].as_str()),
        (Some("unavailable"), Some("rate_limited"))
    );
    assert!(limited.get("download").is_none());
}

#[tokio::test]
async fn global_mint_quota() {
    let app = App::with_media(photo_app(autonomous).global_mints(2)).await;
    for _ in 0..2 {
        assert_eq!(mint_rest(&app, "0").await["status"], "ready");
    }
    let limited = mint_rest(&app, "0").await;
    assert_eq!(limited["reason"], "rate_limited");
}

#[tokio::test]
async fn partition_key_too_large_for_a_link_gives_link_unavailable() {
    let mut item = relayed(0, AttachmentKind::File);
    item.source = Some(cmaeg_core::activity::AttachmentSource::Url {
        url: "https://files.example/report.pdf".into(),
    });
    let setup = MediaSetup::new(vec![item])
        .custom()
        .longest_partition_key()
        .edit(autonomous_custom);
    let (guard, logs) = capture_logs(TRACE_ENV);
    let app = App::with_media(setup).await;
    // The event_ref itself is at the 4096-character limit, so the fields a
    // link adds cannot fit.
    assert!(app.event_ref.as_str().len() > 4_000);
    let view = mint_rest(&app, "0").await;
    drop(guard);
    assert_eq!(
        (view["status"].as_str(), view["reason"].as_str()),
        (Some("unavailable"), Some("link_unavailable"))
    );
    assert!(view.get("download").is_none());
    let text = logs.text();
    assert!(
        text.contains("media link exceeds the token limit"),
        "{text}"
    );
    assert!(text.contains("payload_bytes="), "{text}");
    assert_absent(&text, &[PARTITION_MARKER, "cmaeg1.md."]);
}

#[tokio::test]
async fn deferred_result_carries_no_link() {
    let setup = photo_app(autonomous);
    let app = App::with_media(setup).await;
    let media = app.media.clone().unwrap();
    let full = media.responses().available_bytes();
    let held = media
        .responses()
        .acquire(full, Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    let (body, view) = mint(&app, 0).await;
    assert_eq!(
        (view["status"].as_str(), view["reason"].as_str()),
        (Some("pending"), Some("response_budget"))
    );
    assert!(view.get("download").is_none());
    assert!(!body.contains("cmaeg1.md."));
    drop(held);
}

#[tokio::test]
async fn no_capability_in_logs_prompts_or_context() {
    let mut file = relayed(0, AttachmentKind::File);
    file.filename = Some("quarterly-secret-plan.pdf".into());
    let setup = MediaSetup::new(vec![file]).edit(autonomous);
    setup.fakes.source.serve(&relay(0), Ok(photo()));
    let (guard, logs) = capture_logs("trace,reqwest::connect=trace,hyper_util::client=trace");
    let app = App::with_media(setup).await;
    let (_, view) = mint(&app, 0).await;
    let link = token(&view);
    assert!(view["download"]["curl"]
        .as_str()
        .unwrap()
        .contains("-o 'quarterly-secret-plan.pdf'"));
    let payload = media_link::open(
        &app.state.keys,
        &link.parse::<MediaLinkRef>().unwrap(),
        now(),
    )
    .unwrap();
    assert_eq!(download(&app, &link).await.status(), 200);
    let context = app
        .fixture
        .client
        .get(app.event_url("context"))
        .bearer_auth(&app.key)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    drop(guard);
    let channel = app
        .state
        .resolver
        .resolve(app.fixture.id.as_str())
        .await
        .unwrap();
    let audience = app
        .media
        .as_ref()
        .unwrap()
        .audience(&channel.consumer(None).unwrap());
    assert!(audience.downloads);
    let prompt = render(
        &app.activity,
        &channel.definition,
        &RenderMedia::empty(None),
        audience,
        None,
    )[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let secrets = [link.as_str(), payload.jti.as_str(), "cmaeg1.md."];
    assert_absent(&logs.text(), &secrets);
    assert_absent(
        &logs.text(),
        &["quarterly-secret-plan", &payload.agent_key_hash],
    );
    assert_absent(&prompt, &secrets);
    assert_absent(&context, &secrets);
}

#[tokio::test]
async fn context_reports_downloads_for_autonomous_only() {
    for (edit, expected) in [
        (autonomous as fn(&mut Value), true),
        ((|_| {}) as fn(&mut Value), false),
        (nyxbot_autonomous as fn(&mut Value), false),
    ] {
        let app = App::with_media(photo_app(edit)).await;
        let context: Value = app
            .fixture
            .client
            .get(app.event_url("context"))
            .bearer_auth(&app.key)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            context["capabilities"]["inbound_media"]["downloads"], expected,
            "{context}"
        );
    }
}
