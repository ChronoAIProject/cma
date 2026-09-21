use cmaeg::turns::render_input::render;
use cmaeg_core::{
    activity::{Activity, ActivityKind, InteractionResult},
    channel::ChannelDefinition,
};
use serde_json::json;
fn fixtures() -> (Activity, ChannelDefinition) {
    let activity = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    let mut definition: serde_json::Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition["sources"] = json!([{"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks","key_id":"key","route_ids":["route"],"platform":"telegram","bot_username":"support_bot","admission":{"type":"all"}}]);
    (activity, serde_json::from_value(definition).unwrap())
}
#[test]
fn renders_neutral_text_and_strips_only_bot_address_tokens() {
    let (mut activity, definition) = fixtures();
    activity.kind = ActivityKind::Message {
        text: "@SUPPORT_BOT hello @support_bot_more mail@support_bot".into(),
        attachments: vec![],
        mentions_bot: true,
        reply_to: Some("earlier".parse().unwrap()),
    };
    let rendered = render(&activity, &definition);
    assert_eq!(
        rendered[0]["content"][0]["text"],
        "[replying to an earlier message]\nhello @support_bot_more mail@support_bot"
    );
    for word in ["telegram", "nyxid", "lark"] {
        assert!(!serde_json::to_string(&rendered).unwrap().contains(word));
    }
    activity.kind = ActivityKind::Command {
        name: "help".into(),
        args: "me".into(),
    };
    assert_eq!(
        render(&activity, &definition)[0]["content"][0]["text"],
        "/help me"
    );
    activity.kind = ActivityKind::Interaction(InteractionResult {
        block_id: "choice".into(),
        action_id: "yes".into(),
        values: json!({"label":"Proceed"}).as_object().unwrap().clone(),
    });
    assert_eq!(
        render(&activity, &definition)[0]["content"][0]["text"],
        "The user chose \"Proceed\" (block choice)."
    );
}
#[test]
fn cloud_event_data_is_canonical_and_bounded_without_splitting_utf8() {
    let (mut activity, definition) = fixtures();
    let mut event: cmaeg_core::cloudevent::CloudEvent = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/cloudevent.json"
    ))
    .unwrap();
    event.data = Some(json!({"z":1,"a":2}));
    activity.kind = ActivityKind::Event { ce: event.clone() };
    assert!(render(&activity, &definition)[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .ends_with("{\"a\":2,\"z\":1}"));
    event.data = Some(json!("🤖".repeat(10000)));
    activity.kind = ActivityKind::Event { ce: event };
    let input = render(&activity, &definition);
    let text = input[0]["content"][0]["text"].as_str().unwrap();
    assert!(text.split_once('\n').unwrap().1.len() <= 8192);
}
