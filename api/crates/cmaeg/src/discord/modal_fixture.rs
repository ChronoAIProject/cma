use super::{components_fixture::FixtureState, InboxLaunch, InboxReader, VerifiedInboxLaunch};
use crate::interactions::{Destination, PreparedCard};
use cmaeg_core::message::OutboundMessage;
use serde_json::{json, Value};

pub(crate) fn message() -> OutboundMessage {
    serde_json::from_value(json!({"text":"Complete the request", "blocks":[{"type":"form","id":"private-form","fields":[
        {"id":"private-text","label":"Name","required":true,"kind":{"type":"text"}},
        {"id":"private-choice","label":"Scope","required":false,"kind":{"type":"choice","options":[{"id":"private-read","label":"Read"},{"id":"private-write","label":"Write"}]}},
        {"id":"private-number","label":"Amount","required":true,"kind":{"type":"number"}},
        {"id":"private-date","label":"Date","required":true,"kind":{"type":"date"}}
    ]}]})).unwrap()
}
pub(crate) fn values() -> Value {
    json!([
        {"type":18,"component":{"type":4,"custom_id":"f0","value":"Ada"}},
        {"type":18,"component":{"type":3,"custom_id":"f1","values":["1"]}},
        {"type":18,"component":{"type":4,"custom_id":"f2","value":"12.5"}},
        {"type":18,"component":{"type":4,"custom_id":"f3","value":"2028-02-29"}}
    ])
}
pub(crate) async fn card(f: &FixtureState) -> PreparedCard {
    let thread = if f.thread_kind.is_some() {
        f.queue_thread_metadata();
        f.rest
            .api
            .verify_component_channel(
                super::rest::token(&f.channel, "77").unwrap(),
                "99",
                &f.context.activity.conversation,
            )
            .await
            .unwrap()
    } else {
        None
    };
    let mut prepared = f
        .store
        .prepare_discord_form(
            &f.channel,
            &f.original,
            &message(),
            f.context.clone(),
            thread,
        )
        .await
        .unwrap();
    f.store
        .bind(
            &mut prepared,
            Destination {
                outbound_id: "800".into(),
                platform_id: "800".into(),
            },
        )
        .await
        .unwrap();
    prepared
}
pub(crate) async fn launch(
    f: &FixtureState,
    card: &PreparedCard,
    sequence: u64,
) -> (Value, String) {
    let raw = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    if f.thread_kind.is_some() {
        f.queue_thread_metadata();
    }
    f.receive(raw.clone(), sequence).await.unwrap();
    let body: Value =
        serde_json::from_slice(&f.rest.memory.calls.lock().unwrap().last().unwrap().body).unwrap();
    assert_eq!(body["type"], 9);
    (raw, body["data"]["custom_id"].as_str().unwrap().into())
}
pub(crate) fn submit(f: &FixtureState, custom: &str) -> Value {
    f.setup.fixture.advance(1);
    let mut raw = f.callback(custom);
    raw["type"] = 5.into();
    raw["data"] = json!({"custom_id":custom,"components":values()});
    raw
}
pub(crate) async fn restored(f: &FixtureState, raw: &Value) -> Box<VerifiedInboxLaunch> {
    let InboxLaunch::Launch(value) =
        InboxReader::new(f.setup.fixture.store.channels.clone(), f.keys())
            .load(&f.id(raw), f.channel.clone(), f.setup.now())
            .await
            .unwrap()
    else {
        panic!("expected launch");
    };
    value
}
