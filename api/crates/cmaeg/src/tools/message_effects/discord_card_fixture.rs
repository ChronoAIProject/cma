//! Shared local HTTP/record fixtures for native card authority and mutation tests.
use super::*;
use crate::{
    delivery::{
        custom::CustomDelivery, engine::DeliveryEngine, relay_client::RelayClient,
        router::ShardedScheduler,
    },
    discord::components_fixture::FixtureState,
    http,
    turns::TurnTracker,
    DeliveryConfig,
};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

pub(crate) fn state(f: &FixtureState, enabled: bool) -> ToolsState {
    let (sender, _) = mpsc::channel(16);
    ToolsState {
        discord_thread_history: None,
        telegram_topics: None,
        discord_threads: None,
        native_lark_conversation_history: None,
        discord_conversation_history: None,
        complete_replies_enabled: false,
        commonmark_relay: Default::default(),
        telegram_effects_issuance_enabled: false,
        native_telegram_media_controls_enabled: false,
        native_telegram_media_control_edits_enabled: false,
        native_media_edits: None,
        native_whatsapp_forms: None,
        native_typing: None,
        native_lark_effects: None,
        lark_effects_issuance_enabled: false,
        lark_effects: None,
        telegram_effects: None,
        whatsapp_templates_enabled: false,
        discord_interactions_enabled: false,
        native_discord_media_controls_enabled: false,
        native_discord_media_card_edits_enabled: false,
        native_discord_media_card_replacements_enabled: false,
        native_discord_media_card_sets_enabled: false,
        whatsapp_interactions_enabled: false,
        native_whatsapp_interactions_enabled: false,
        native_whatsapp_media_controls_enabled: false,
        consumer_interactions_enabled: false,
        native_media_enabled: false,
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
        whatsapp_reactions: None,
        native_whatsapp_templates_enabled: false,
        native_whatsapp_groups_enabled: false,
        relay_text_edits_enabled: false,
        native_discord_edits_enabled: false,
        discord_effects_issuance_enabled: false,
        discord_effects: Some(Arc::new(
            discord::DiscordEffectsClient::new(f.rest.api.clone()).with_card_effects(enabled),
        )),
        lark_cards_enabled: false,
        resolver: f.setup.state.resolver.clone(),
        keys: f.keys(),
        providers: f.provider.client(true, Duration::from_secs(5)),
        delivery: Arc::new(ShardedScheduler::new(sender)),
        routed_replies: None,
        tracker: TurnTracker::new(http::readiness::Readiness::initialized()),
        public_origin: "https://gateway.example".into(),
        public_api_url: "https://gateway.example".into(),
        public_api_aliases: vec![],
        advertise_public_api_url: true,
        media: None,
        downloads: Default::default(),
    }
}
pub(crate) fn engine(f: &FixtureState, state: &mut ToolsState) -> DeliveryEngine {
    let (sender, receiver) = mpsc::channel(16);
    let mut relay = RelayClient::new("http://127.0.0.1:1", Duration::from_secs(5))
        .unwrap()
        .with_interactions(f.store.clone())
        .with_discord_card_edits(true)
        .with_consumer_interactions(true)
        .with_discord_multiselect(true);
    relay.discord = f.rest.api.clone();
    state.delivery = Arc::new(ShardedScheduler::new(sender));
    DeliveryEngine::start(
        receiver,
        f.keys(),
        relay,
        DeliveryConfig::default(),
        state.tracker.clone(),
        CustomDelivery::new(state.public_origin.clone(), Arc::default()),
    )
}
pub(crate) fn caller(f: &FixtureState) -> AgentCaller {
    AgentCaller {
        key_hash: *f
            .channel
            .consumer_key_hash(f.original.consumer.as_ref())
            .unwrap(),
    }
}
pub(crate) fn reference(card: &crate::interactions::PreparedCard) -> MessageRef {
    card.card.reference.clone().unwrap()
}
pub(crate) fn echo(f: &FixtureState, write_status: StatusCode, reactions: Value) {
    let memory = Arc::downgrade(&f.rest.memory);
    let metadata = if f.thread_kind.is_some() {
        f.thread_metadata()
    } else if f.context.activity.conversation.kind
        == cmaeg_core::activity::ConversationKind::Private
    {
        json!({"id":"99","type":1})
    } else {
        json!({"id":"99","type":0,"guild_id":"88"})
    };
    *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
        let memory = memory.upgrade().unwrap();
        let (status, body) = if call.method == "GET" && call.path == "/api/v10/channels/99" {
            (StatusCode::OK, metadata.clone())
        } else if call.method == "GET" {
            (
                StatusCode::OK,
                json!({"id":"800","channel_id":"99","author":{"id":"77","bot":true},"reactions":reactions}),
            )
        } else if call.method == "PATCH" {
            let mut body: Value = serde_json::from_slice(&call.body).unwrap();
            body["id"] = "800".into();
            body["channel_id"] = "99".into();
            body["author"] = json!({"id":"77","bot":true});
            (StatusCode::OK, body)
        } else {
            (write_status, json!({"retry_after":1}))
        };
        memory.responses.lock().unwrap().push_back((status, body));
    }));
}
pub(crate) fn writes(f: &FixtureState) -> Vec<(String, String)> {
    f.rest
        .memory
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.method != "GET")
        .map(|c| (c.method.clone(), c.path.clone()))
        .collect()
}
pub(crate) async fn http_call(
    f: &FixtureState,
    state: &ToolsState,
    path: &str,
    body: Value,
    key: Option<&str>,
) -> (StatusCode, Value) {
    let key_value = f
        .channel
        .consumer(f.original.consumer.as_ref())
        .unwrap()
        .credential
        .unwrap()
        .as_str();
    let router = http::tools::router(state.clone()).merge(crate::mcp::router(state.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut request = reqwest::Client::new()
        .post(format!("http://{address}{path}"))
        .bearer_auth(key_value)
        .json(&body);
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    let body = response.json().await.unwrap();
    server.abort();
    (status, body)
}
