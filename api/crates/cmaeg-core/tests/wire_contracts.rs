use cmaeg_core::{
    activity::{Activity, AttachmentKind},
    channel::{ChannelDefinition, ConformanceLevel, Platform},
    cloudevent::CloudEvent,
    ids::{BindingId, ChannelId, ConversationId, EventId, ProviderSlug, RecordId},
    message::{OutboundMessage, RenderingReport},
    problem::{Problem, ProblemCode},
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::fmt::Debug;

fn stable<T: DeserializeOwned + Serialize + Debug + PartialEq>(value: Value) {
    let decoded: T = serde_json::from_value(value).unwrap();
    let first = serde_json::to_vec(&decoded).unwrap();
    let restored: T = serde_json::from_slice(&first).unwrap();
    assert_eq!(decoded, restored);
    assert_eq!(first, serde_json::to_vec(&restored).unwrap());
}

#[test]
fn golden_fixtures_produce_byte_stable_serializations() {
    stable::<Activity>(serde_json::from_str(include_str!("fixtures/activity.json")).unwrap());
    stable::<OutboundMessage>(
        serde_json::from_str(include_str!("fixtures/outbound_message.json")).unwrap(),
    );
    stable::<CloudEvent>(serde_json::from_str(include_str!("fixtures/cloudevent.json")).unwrap());
    let channel: ChannelDefinition =
        serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    let first = serde_json::to_vec(&channel.to_public_json()).unwrap();
    let decoded: ChannelDefinition = serde_json::from_slice(&first).unwrap();
    assert_eq!(
        first,
        serde_json::to_vec(&decoded.to_public_json()).unwrap()
    );
}

#[test]
fn opaque_ids_validate_both_direct_construction_and_wire_decoding() {
    macro_rules! check {
        ($($ty:ty),+) => { $(
            for text in ["x".into(),"é".repeat(256)] {
                let id: $ty = text.parse().unwrap();
                assert_eq!(id.as_str(),text);
                assert_eq!(id.to_string(),text);
                stable::<$ty>(json!(text));
            }
            for text in [String::new(),"é".repeat(257)] {
                assert!(text.parse::<$ty>().is_err());
                assert!(serde_json::from_value::<$ty>(json!(text)).is_err());
            }
        )+ };
    }
    check!(RecordId, EventId, BindingId, ProviderSlug, ConversationId);
    for value in ["", "wrong", &format!("cmaeg1.ch.{}", "x".repeat(512))] {
        assert!(value.parse::<ChannelId>().is_err());
        assert!(serde_json::from_value::<ChannelId>(json!(value)).is_err());
    }
    let id = format!("cmaeg1.ch.{}", "x".repeat(512 - 10));
    stable::<ChannelId>(json!(id));
    for (value, wellformed) in [
        ("conv_0123456789abcdef0123456789abcdef", true),
        ("conv_0123456789ABCDEF0123456789abcdef", false),
        ("conv_0123", false),
        ("opaque", false),
    ] {
        assert_eq!(
            value.parse::<ConversationId>().unwrap().is_wellformed(),
            wellformed
        );
    }
}

#[test]
fn problem_codes_preserve_machine_codes_status_and_optional_context() {
    for (code, wire) in [
        (ProblemCode::InvalidRequest, "invalid_request"),
        (ProblemCode::NotAuthenticated, "not_authenticated"),
        (ProblemCode::Forbidden, "forbidden"),
        (ProblemCode::ChannelNotFound, "channel_not_found"),
        (
            ProblemCode::ChannelRestoreUnavailable,
            "channel_restore_unavailable",
        ),
        (ProblemCode::ChannelInvalid, "channel_invalid"),
        (ProblemCode::ProviderUnavailable, "provider_unavailable"),
        (ProblemCode::ProviderRejected, "provider_rejected"),
        (ProblemCode::ProviderBusy, "provider_busy"),
        (ProblemCode::Draining, "draining"),
        (ProblemCode::EventRefInvalid, "event_ref_invalid"),
        (ProblemCode::EventRefExpired, "event_ref_expired"),
        (ProblemCode::CallbackInvalid, "callback_invalid"),
        (ProblemCode::AdmissionRefused, "admission_refused"),
        (ProblemCode::RateLimited, "rate_limited"),
        (ProblemCode::Conflict, "conflict"),
        (ProblemCode::PayloadTooLarge, "payload_too_large"),
        (ProblemCode::UnsupportedMediaType, "unsupported_media_type"),
        (ProblemCode::Internal, "internal"),
    ] {
        let problem = Problem::new(code, 400, "reason");
        let value = serde_json::to_value(&problem).unwrap();
        assert_eq!(value["type"], "about:blank");
        assert_eq!(value["code"], wire);
        assert_eq!(problem.to_string(), format!("{wire} (400): reason"));
        stable::<Problem>(value);
    }
    let problem: Problem = serde_json::from_value(json!({"title":"Forbidden","status":403,"code":"forbidden","instance":"/v1/channels/example"})).unwrap();
    assert_eq!(problem.r#type, "about:blank");
    assert_eq!(problem.to_string(), "forbidden (403)");
    assert!(serde_json::from_value::<Problem>(
        json!({"title":"Forbidden","status":403,"code":"forbidden","extra":true})
    )
    .is_err());
}

#[test]
fn rendering_and_form_variants_have_stable_public_encodings() {
    for outcome in ["rendered", "degraded", "unsupported"] {
        stable::<RenderingReport>(json!({"outcome":outcome,"blocks":[
            {"index":0,"outcome":{"type":"exact"}},
            {"index":1,"outcome":{"type":"degraded","how":"text fallback"}},
            {"index":2,"outcome":{"type":"dropped","why":"unsupported"}}
        ]}));
    }
    for kind in [
        json!({"type":"text"}),
        json!({"type":"number"}),
        json!({"type":"date"}),
        json!({"type":"choice","options":[{"id":"a","label":"A","style":"primary"},{"id":"b","label":"B","style":"danger"}]}),
    ] {
        stable::<OutboundMessage>(
            json!({"blocks":[{"type":"form","id":"form","fields":[{"id":"field","label":"Field","kind":kind,"required":true}]}]}),
        );
    }
    for kind in ["image", "file", "audio", "video", "other"] {
        stable::<AttachmentKind>(json!(kind));
    }
    for level in ["l1", "l2", "l3", "l4", "l5"] {
        stable::<ConformanceLevel>(json!(level));
    }
    assert_eq!("telegram".parse::<Platform>().unwrap().as_str(), "telegram");
    assert!("other".parse::<Platform>().is_err());
}
