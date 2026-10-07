#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "relay/support.rs"]
pub mod relay_support;
/// The media helpers the shared tools app is built with.
pub mod media {
    #[path = "fakes.rs"]
    pub mod fakes;
    #[path = "image_fixtures.rs"]
    pub mod image_fixtures;
    #[path = "support.rs"]
    pub mod support;
}
#[path = "tools/media_support.rs"]
pub mod media_support;
#[path = "relay/signer.rs"]
pub mod signer;
#[allow(dead_code)]
#[path = "tools/support.rs"]
mod support;
use cmaeg::{
    interactions::{Destination, InteractionStore},
    records::RecordClient,
};
use cmaeg_core::{ids::EventId, message::OutboundMessage};
use serde_json::json;

#[tokio::test]
async fn callback_authority_survives_restart_and_fences_actor_chat_message_and_revision() {
    let app = support::App::new().await;
    let backend = fake_cma::FakeCma::start(false).await;
    let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
    let store = InteractionStore::new(
        records.clone(),
        app.state.keys.clone(),
        app.state.providers.clone(),
    );
    let event = app
        .event_ref
        .open(&app.state.keys, relay_support::now())
        .unwrap()
        .payload;
    let channel = app
        .state
        .resolver
        .resolve(event.channel_id.as_str())
        .await
        .unwrap();
    let message:OutboundMessage=serde_json::from_value(json!({"text":"Choose owner","blocks":[{"type":"choice","id":"owner","prompt":"Personal or org?","multi":false,"options":[{"id":"personal","label":"Personal","selected":true},{"id":"org","label":"Organization"}]},{"type":"link","url":"https://nyx.example/login","label":"Sign in"}]})).unwrap();
    let mut prepared = store.prepare(&channel, &event, &message).await.unwrap();
    let data = prepared.markup["inline_keyboard"][1][0]["callback_data"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(data.len() <= 64);
    assert_eq!(
        prepared.markup["inline_keyboard"][2][0]["url"],
        "https://nyx.example/login"
    );
    let reference = store
        .bind(
            &mut prepared,
            Destination {
                outbound_id: uuid::Uuid::new_v4().to_string(),
                platform_id: "42".into(),
            },
        )
        .await
        .unwrap();
    let snapshot = app
        .state
        .providers
        .event_context(
            &channel.definition.provider.slug,
            &channel.definition.provider.binding_id,
            &event.conversation_id,
            &event.event_id,
            channel
                .definition
                .secrets
                .provider_credential
                .as_ref()
                .unwrap(),
        )
        .await
        .unwrap();
    let mut activity = snapshot.activity;
    activity.event_id = EventId::new("click-1").unwrap();
    drop(store);
    let restarted =
        InteractionStore::new(records, app.state.keys.clone(), app.state.providers.clone());
    let mut impostor = activity.clone();
    impostor.actor.id = "another-user".into();
    assert!(restarted
        .consume(&channel, &impostor, "route-1", "query-1", &data, "42")
        .await
        .is_err());
    impostor = activity.clone();
    impostor.conversation.id = "another-chat".into();
    assert!(restarted
        .consume(&channel, &impostor, "route-1", "query-1", &data, "42")
        .await
        .is_err());
    assert!(restarted
        .consume(&channel, &activity, "route-1", "query-1", &data, "43")
        .await
        .is_err());
    assert!(restarted
        .consume(&channel, &activity, "another-route", "query-1", &data, "42")
        .await
        .is_err());
    let (a, b) = tokio::join!(
        restarted.consume(&channel, &activity, "route-1", "query-1", &data, "42"),
        restarted.consume(&channel, &activity, "route-1", "query-1", &data, "42")
    );
    let first = a.unwrap();
    assert_eq!(first.0, b.unwrap().0);
    assert_eq!(first.0.values["selected"], json!(["org"]));
    assert_eq!(first.0.values["message_ref"], reference.as_str());
    assert_eq!(first.0.block_id, "owner");
    assert_eq!(first.0.action_id, "org");
    assert!(restarted
        .consume(&channel, &activity, "route-1", "query-2", &data, "42")
        .await
        .is_err());
    let payload = reference.open(&app.state.keys).unwrap().payload;
    let next = restarted
        .replacement(&channel, &payload, &message)
        .await
        .unwrap();
    assert_eq!(next.card.revision, 2);
    assert!(restarted
        .consume(&channel, &activity, "route-1", "query-1", &data, "42")
        .await
        .is_err());
    // The opaque encrypted record is namespace- and ID-bound, and never stores plain UI/actor values.
    for record in backend.records().values() {
        assert!(record.blob.starts_with(b"cmaeg1.in."));
        assert!(!String::from_utf8_lossy(&record.blob).contains("Choose owner"));
    }
}
