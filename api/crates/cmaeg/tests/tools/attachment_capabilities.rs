//! `inbound_media` in event context: what the calling binding can do with an
//! event's attachments, and its absence while media is disabled.
use super::{
    media_support::{native, relayed, MediaSetup},
    support::App,
};
use cmaeg_core::activity::AttachmentKind;
use serde_json::{json, Value};

async fn capabilities(app: &App) -> Value {
    let response = app
        .fixture
        .client
        .get(app.event_url("context"))
        .bearer_auth(&app.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["capabilities"].clone()
}

fn image() -> MediaSetup {
    MediaSetup::new(vec![relayed(0, AttachmentKind::Image)])
}

#[tokio::test]
async fn media_off_adds_no_inbound_media() {
    let app = App::new().await;
    let capabilities = capabilities(&app).await;
    assert!(capabilities.get("inbound_media").is_none());
    assert_eq!(
        capabilities,
        json!({"edit":true,"typing":false,"buttons":"inline_keyboard","cards":"text","attachments":"link_text","max_text_chars":4000})
    );
}

#[tokio::test]
async fn a_guarded_cma_codex_binding_has_the_read_tool_but_no_downloads() {
    let app = App::with_media(image()).await;
    assert_eq!(
        capabilities(&app).await["inbound_media"],
        json!({"read_tool":true,"transcripts":true,"visuals":"none","downloads":false})
    );
}

#[tokio::test]
async fn an_autonomous_binding_is_still_not_offered_downloads() {
    let app = App::with_media(image().edit(|value| {
        value["profile"]["permission"]["action_mode"] = "autonomous".into();
    }))
    .await;
    let inbound = &capabilities(&app).await["inbound_media"];
    assert_eq!(
        (
            inbound["read_tool"].as_bool(),
            inbound["downloads"].as_bool()
        ),
        (Some(true), Some(false))
    );
}

#[tokio::test]
async fn a_nyxbot_binding_has_no_read_tool() {
    let app = App::with_media(image().edit(|value| {
        value["provider"]["slug"] = "nyxbot".into();
        value["profile"]["metadata"]["provider"] = "nyxbot".into();
    }))
    .await;
    assert_eq!(
        capabilities(&app).await["inbound_media"]["read_tool"],
        false
    );
}

#[tokio::test]
async fn an_existing_agent_binding_has_no_read_tool() {
    let app = App::with_media(image().edit(|value| {
        value["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agent-1"});
    }))
    .await;
    assert_eq!(
        capabilities(&app).await["inbound_media"]["read_tool"],
        false
    );
}

#[tokio::test]
async fn visuals_follow_the_source_and_the_extractor() {
    let native_app =
        App::with_media(MediaSetup::new(vec![native("clip", AttachmentKind::Video)]).native())
            .await;
    assert_eq!(
        capabilities(&native_app).await["inbound_media"]["visuals"],
        "platform_thumbnail"
    );
    let relay = App::with_media(MediaSetup::new(vec![relayed(0, AttachmentKind::Video)])).await;
    assert_eq!(
        capabilities(&relay).await["inbound_media"]["visuals"],
        "none"
    );
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Video)]);
    setup.fakes.visuals.keyframes();
    let keyframes = App::with_media(setup).await;
    assert_eq!(
        capabilities(&keyframes).await["inbound_media"]["visuals"],
        "keyframes"
    );
}

#[tokio::test]
async fn transcripts_follow_the_transcriber() {
    let setup = image();
    setup.fakes.transcriber.disable();
    let app = App::with_media(setup).await;
    assert_eq!(
        capabilities(&app).await["inbound_media"]["transcripts"],
        false
    );
}

#[tokio::test]
async fn verified_context_carries_inbound_media_too() {
    let app = App::with_media(image()).await;
    let response = app
        .fixture
        .client
        .post(app.event_url("context"))
        .bearer_auth(&app.key)
        .json(&app.activity)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let verified: Value = response.json().await.unwrap();
    assert_eq!(verified["capabilities"]["inbound_media"]["read_tool"], true);
}
