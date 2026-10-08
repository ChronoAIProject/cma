use cmaeg::turns::{
    render_input::{self, ResponsesInputItem},
    render_media::{Audience, RenderMedia},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, InteractionResult, SourceKind},
    channel::{ChannelDefinition, Platform},
};
use serde_json::json;
/// Without attachments neither the audience nor a nonce may change a byte.
fn render(activity: &Activity, definition: &ChannelDefinition) -> Vec<ResponsesInputItem> {
    let plain = render_input::render(
        activity,
        definition,
        &RenderMedia::empty(None),
        Audience {
            tools: false,
            downloads: false,
        },
        None,
    );
    let fenced = render_input::render(
        activity,
        definition,
        &RenderMedia::empty(Some("0123456789ab".into())),
        Audience {
            tools: true,
            downloads: true,
        },
        None,
    );
    assert_eq!(plain, fenced);
    plain
}
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
        album_id: None,
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
    activity.kind = ActivityKind::Event {
        ce: event.clone(),
        attachments: Vec::new(),
    };
    assert!(render(&activity, &definition)[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .ends_with("{\"a\":2,\"z\":1}"));
    event.data = Some(json!("🤖".repeat(10000)));
    activity.kind = ActivityKind::Event {
        ce: event,
        attachments: Vec::new(),
    };
    let input = render(&activity, &definition);
    let text = input[0]["content"][0]["text"].as_str().unwrap();
    assert!(text.split_once('\n').unwrap().1.len() <= 8192);
}
#[test]
fn lark_and_feishu_strip_only_leading_mention_keys() {
    let (mut activity, mut definition) = fixtures();
    definition.sources = vec![serde_json::from_value(json!({"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks","key_id":"key","route_ids":["route"],"platform":"lark","bot_id":"ou_bot","admission":{"type":"all"}})).unwrap()];
    for platform in [Platform::Lark, Platform::Feishu] {
        activity.source = SourceKind::NyxidRelay {
            platform,
            route_id: "route".into(),
        };
        for (text, mentions_bot, expected) in [
            ("@_user_1 status?", true, "status?"),
            (
                " @_user_1  @_user_12 ask @_user_2 later",
                true,
                "ask @_user_2 later",
            ),
            ("@_user_1", true, ""),
            ("@_user_1x keep", true, "@_user_1x keep"),
            ("@_user_ keep", true, "@_user_ keep"),
            ("@_user_1 unaddressed", false, "@_user_1 unaddressed"),
        ] {
            activity.kind = ActivityKind::Message {
                text: text.into(),
                attachments: vec![],
                mentions_bot,
                reply_to: None,
                album_id: None,
            };
            assert_eq!(
                render(&activity, &definition)[0]["content"][0]["text"],
                expected,
                "{text}"
            );
        }
    }
}
#[test]
fn lark_post_placeholders_name_their_embedded_kind() {
    let (mut activity, definition) = fixtures();
    activity.source = SourceKind::NyxidRelay {
        platform: Platform::Lark,
        route_id: "route".into(),
    };
    let embedded = |reference: &str, kind: &str| {
        serde_json::from_value(json!({"reference": reference, "kind": kind, "variant": "embedded"}))
            .unwrap()
    };
    activity.kind = ActivityKind::Message {
        text: "Weekly update\nsee the chart".into(),
        attachments: vec![
            embedded("lark:img_v3_chart", "image"),
            embedded("lark:file_v3_demo", "video"),
        ],
        mentions_bot: false,
        reply_to: None,
        album_id: None,
    };
    let input = render_input::render(
        &activity,
        &definition,
        &RenderMedia::empty(None),
        Audience {
            tools: true,
            downloads: false,
        },
        None,
    );
    assert_eq!(
        input[0]["content"][0]["text"],
        "Weekly update\nsee the chart\n\n[Attachments: 2.]\n\
         [0] image embedded in the post · not retrievable through this channel\n\
         [1] video embedded in the post · not retrievable through this channel"
    );
}
