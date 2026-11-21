use super::support::App;
use cmaeg::auth::AgentCaller;
use cmaeg_core::{
    activity::SourceKind,
    ids::MessageRef,
    seal::{
        event_ref::activity_digest, seal_blob, DiscordMessageDestination, EventRef,
        MessageRefPayload, ReplyAddress,
    },
};
use reqwest::header::HeaderMap;

fn caller(app: &App) -> AgentCaller {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", app.key).parse().unwrap(),
    );
    AgentCaller::from_headers(&headers).unwrap()
}

#[tokio::test]
async fn native_edit_capability_requires_issuance_gate_and_does_not_change_relay_capabilities() {
    let mut app = App::new().await;
    let previous = app
        .state
        .context(caller(&app), &app.event_ref)
        .await
        .unwrap()
        .capabilities
        .edit;
    let mut event = app
        .event_ref
        .open(&app.state.keys, super::relay_support::now())
        .unwrap()
        .payload;
    let mut activity = app.activity.clone();
    activity.source = SourceKind::Discord {
        bot_id: "77".into(),
    };
    event.reply = ReplyAddress::Discord {
        bot_id: "77".into(),
        channel_id: "99".into(),
        message_id: "700".into(),
        thread_id: None,
    };
    event.activity_sha256 = Some(activity_digest(&activity));
    let reference = EventRef::seal(&event, &app.state.keys).unwrap();
    for enabled in [false, true, false] {
        app.state.native_discord_edits_enabled = enabled;
        let view = app
            .state
            .verify_context(caller(&app), &reference, activity.clone())
            .await
            .unwrap();
        assert_eq!(view.capabilities.edit, enabled);
        assert_eq!(view.capabilities.max_text_chars, 2000);
        let relay = app
            .state
            .context(caller(&app), &app.event_ref)
            .await
            .unwrap();
        assert_eq!(relay.capabilities.edit, previous);
    }
}

#[tokio::test]
async fn native_edit_authorization_rechecks_current_record_after_cached_key_revocation() {
    let app = App::new().await;
    let channel = app
        .state
        .resolver
        .resolve(app.fixture.id.as_str())
        .await
        .unwrap();
    let reference = MessageRef::seal(
        &MessageRefPayload {
            discord_message: Some(Box::new(DiscordMessageDestination {
                effects_only: false,
                effects: None,
                bot_id: "77".into(),
                channel_id: "99".into(),
                authority_sha256: [3; 32],
            })),
            native_media_edit: None,
            received_reaction: None,
            native_lark_effect: None,
            whatsapp_reaction: None,
            lark_effect: None,
            telegram_effect: None,
            channel_id: channel.id.clone(),
            event_id: "discord:77:700".parse().unwrap(),
            platform_message_id: "800".into(),
            route_id: "discord:77:99".into(),
            interaction_record_id: None,
            relay_message: None,
            consumer: None,
        },
        &app.state.keys,
    )
    .unwrap();
    app.state
        .authorize_message(caller(&app), &reference, "updateMessage")
        .await
        .unwrap();
    let mut revoked = channel.definition.clone();
    revoked.secrets.agent_key_hash = format!("sha256:{}", "1".repeat(64));
    *app.fixture.backend.blob.lock().unwrap() =
        seal_blob(&app.state.keys, &serde_json::to_vec(&revoked).unwrap()).unwrap();
    let result = app
        .state
        .authorize_message(caller(&app), &reference, "updateMessage")
        .await;
    assert!(
        result.is_err(),
        "an old cached channel must not authorize a revoked key"
    );
    assert!(app.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_card_edit_authorization_rechecks_current_record_after_cached_key_revocation() {
    let app = App::new().await;
    let channel = app
        .state
        .resolver
        .resolve(app.fixture.id.as_str())
        .await
        .unwrap();
    let reference = MessageRef::seal(
        &MessageRefPayload {
            discord_message: None,
            native_media_edit: None,
            received_reaction: None,
            native_lark_effect: None,
            whatsapp_reaction: None,
            lark_effect: None,
            telegram_effect: None,
            channel_id: channel.id.clone(),
            event_id: "discord:77:700".parse().unwrap(),
            platform_message_id: "800".into(),
            route_id: "discord:77:99".into(),
            interaction_record_id: Some("11111111-1111-4111-8111-111111111111".into()),
            relay_message: None,
            consumer: None,
        },
        &app.state.keys,
    )
    .unwrap();
    app.state
        .authorize_message(caller(&app), &reference, "updateMessage")
        .await
        .unwrap();
    let mut revoked = channel.definition.clone();
    revoked.secrets.agent_key_hash = format!("sha256:{}", "1".repeat(64));
    *app.fixture.backend.blob.lock().unwrap() =
        seal_blob(&app.state.keys, &serde_json::to_vec(&revoked).unwrap()).unwrap();
    assert!(app
        .state
        .authorize_message(caller(&app), &reference, "updateMessage")
        .await
        .is_err());
    assert!(app.requests.lock().unwrap().is_empty());
}
