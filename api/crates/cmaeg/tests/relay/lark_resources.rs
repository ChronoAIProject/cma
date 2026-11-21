//! Resource authority must originate in the signed raw post, before sealing.
use super::{
    lark_media::{accept, last},
    support::*,
};
use cmaeg_core::{
    activity::{ActivityKind, AttachmentSource},
    channel::LarkResourceKind,
};
use serde_json::{json, Value};

fn bind(value: &mut Value) {
    value["sources"][0]["bot_id"] = json!("ou_receiver");
    value["sources"][0]["lark_resources"] = json!({"user_service_id":"connection-one","catalog_service_id":"catalog-one","app_id":"cli_fixture"});
    value["secrets"]["channel_key"] = json!("resource-fixture-credential");
}
fn post(platform: &str) -> Envelope {
    let mut event = Envelope::on(platform);
    event.lark_message()["message_type"] = "post".into();
    event.lark_message()["content"] = json!({"content":[[{"tag":"text","text":"look"},{"tag":"img","image_key":"img_resource"},{"tag":"media","file_key":"file_video"}]]}).to_string().into();
    event.payload["content"] = json!({"type":"unknown","text":null,"attachments":[]});
    event
}

#[tokio::test]
async fn verified_native_message_app_and_resource_are_bound_before_the_activity_is_sealed() {
    for platform in ["lark", "feishu"] {
        let fixture = Fixture::on_with_media(platform, bind, Default::default()).await;
        accept(&fixture, &post(platform), platform).await;
        let (text, attachments) = last(&fixture);
        assert_eq!(text, "look");
        assert_eq!(attachments.len(), 2);
        for (attachment, kind, key) in [
            (&attachments[0], LarkResourceKind::Image, "img_resource"),
            (&attachments[1], LarkResourceKind::File, "file_video"),
        ] {
            let Some(AttachmentSource::LarkResource {
                message_id,
                resource_key,
                resource_kind,
                binding,
                bot_id,
                source_revision,
            }) = &attachment.source
            else {
                panic!("missing resource locator");
            };
            assert_eq!(message_id, "om_message");
            assert_ne!(message_id, "message-1"); // NyxID's envelope ID is a different namespace.
            assert_eq!(resource_key, key);
            assert_eq!(*resource_kind, kind);
            assert_eq!(binding.app_id, "cli_fixture");
            assert_eq!(bot_id, "ou_receiver");
            assert_eq!(source_revision.len(), 64);
        }
        let requests = fixture.recorder.requests.lock().unwrap();
        let request = requests.last().unwrap();
        let sealed = request
            .event_ref
            .open(&fixture.state.master_keys, now())
            .unwrap();
        assert!(sealed.payload.verifies_activity(&request.activity));
        let mut changed = request.activity.clone();
        let ActivityKind::Message { attachments, .. } = &mut changed.kind else {
            unreachable!()
        };
        let Some(AttachmentSource::LarkResource { resource_key, .. }) = &mut attachments[0].source
        else {
            unreachable!()
        };
        *resource_key = "img_forged".into();
        assert!(!sealed.payload.verifies_activity(&changed));
    }
}

#[tokio::test]
async fn absent_binding_or_wrong_app_keeps_post_text_and_nonretrievable_placeholders() {
    for configured in [false, true] {
        let fixture = Fixture::on_with_media(
            "lark",
            |value| {
                if configured {
                    bind(value);
                }
            },
            Default::default(),
        )
        .await;
        let mut event = post("lark");
        event.payload["raw_platform_data"]["header"]["app_id"] = "cli_other".into();
        accept(&fixture, &event, "unbound").await;
        let (text, attachments) = last(&fixture);
        assert_eq!(text, "look");
        assert_eq!(attachments.len(), 2);
        assert!(attachments
            .iter()
            .all(|attachment| attachment.source.is_none()));
    }
}

#[tokio::test]
async fn malformed_native_id_or_resource_path_cannot_create_a_fetch_locator() {
    for field in ["message", "resource"] {
        let fixture = Fixture::on_with_media("lark", bind, Default::default()).await;
        let mut event = post("lark");
        if field == "message" {
            event.lark_message()["message_id"] = "om_native/../../other".into();
        } else {
            event.lark_message()["content"] = json!({"content":[[{"tag":"text","text":"look"},{"tag":"img","image_key":"img_native/../../other"}]]}).to_string().into();
        }
        accept(&fixture, &event, field).await;
        let (text, attachments) = last(&fixture);
        assert_eq!(text, "look");
        assert!(attachments
            .iter()
            .all(|attachment| attachment.source.is_none()));
    }
}
