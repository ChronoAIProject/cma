//! A custom source's attachments setting: pointer and origin bounds.
use cmaeg_core::{
    activity::{validate_attachments, Attachment, SourceKind},
    channel::{
        ChannelDefinition, CustomAttachments, Source, DEFAULT_CUSTOM_ATTACHMENT_POINTER,
        MAX_CUSTOM_ATTACHMENT_ORIGINS, MAX_CUSTOM_ATTACHMENT_POINTER_BYTES,
    },
    ValidationError,
};
use serde_json::json;

const BASE: &str = "/sources/0/attachments";

fn setting(pointer: &str, origins: &[&str]) -> CustomAttachments {
    CustomAttachments {
        pointer: pointer.into(),
        origins: origins.iter().map(|origin| (*origin).to_owned()).collect(),
    }
}

fn refused_at(setting: CustomAttachments) -> String {
    setting.validate(BASE).unwrap_err().pointer
}

#[test]
fn an_empty_setting_defaults_to_the_attachments_pointer_and_no_origins() {
    let parsed: CustomAttachments = serde_json::from_value(json!({})).unwrap();
    assert_eq!(parsed.pointer, DEFAULT_CUSTOM_ATTACHMENT_POINTER);
    assert_eq!(parsed.pointer, "/attachments");
    assert!(parsed.origins.is_empty());
    parsed.validate(BASE).unwrap();
}

#[test]
fn the_pointer_is_bounded_to_240_bytes() {
    let longest = format!("/{}", "a".repeat(MAX_CUSTOM_ATTACHMENT_POINTER_BYTES - 1));
    assert_eq!(longest.len(), 240);
    setting(&longest, &[]).validate(BASE).unwrap();
    let longer = format!("{longest}b");
    assert_eq!(
        refused_at(setting(&longer, &[])),
        "/sources/0/attachments/pointer"
    );
    assert_eq!(
        refused_at(setting("", &[])),
        "/sources/0/attachments/pointer"
    );
}

#[test]
fn the_pointer_must_be_an_rfc_6901_pointer_without_controls() {
    setting("/media/items", &[]).validate(BASE).unwrap();
    setting("/a~0b/c~1d", &[]).validate(BASE).unwrap();
    for pointer in ["attachments", "/a~2b", "/a~", "/a\u{0007}b", "/a\nb"] {
        assert_eq!(
            refused_at(setting(pointer, &[])),
            "/sources/0/attachments/pointer",
            "{pointer:?}"
        );
    }
}

#[test]
fn exact_https_origins_are_accepted_structurally() {
    setting(
        "/attachments",
        &[
            "https://a.example",
            "https://a.example:8443",
            "https://[2001:db8::1]",
            "https://203.0.113.7",
            "https://xn--bcher-kva.example",
        ],
    )
    .validate(BASE)
    .unwrap();
}

#[test]
fn each_malformed_origin_is_refused_at_its_exact_pointer() {
    for origin in [
        "http://a.example",
        "https://a.example/",
        "https://a.example/x",
        "https://a.example?q=1",
        "https://a.example#f",
        "https://A.example",
        "https://a.example:443",
        "https://u@a.example",
        "https://u:p@a.example",
        "https://*.example",
        "https://a..example",
        "a.example",
        "",
    ] {
        let error = setting("/attachments", &["https://ok.example", origin])
            .validate(BASE)
            .unwrap_err();
        assert_eq!(
            error.pointer, "/sources/0/attachments/origins/1",
            "{origin:?}"
        );
        assert!(error.message.contains("https origin"), "{origin:?}");
    }
}

#[test]
fn a_wildcard_origin_cannot_pass_the_parse() {
    assert_eq!(
        refused_at(setting("/attachments", &["https://*.example"])),
        "/sources/0/attachments/origins/0"
    );
    assert_eq!(
        refused_at(setting("/attachments", &["https://a.*.example"])),
        "/sources/0/attachments/origins/0"
    );
}

#[test]
fn an_overlong_host_is_refused() {
    let label = "a".repeat(63);
    let host = [label.as_str(); 4].join(".");
    assert!(host.len() > 253);
    let origin = format!("https://{host}");
    assert_eq!(
        refused_at(setting("/attachments", &[origin.as_str()])),
        "/sources/0/attachments/origins/0"
    );
}

#[test]
fn origins_are_unique_and_at_most_sixteen() {
    let error = setting(
        "/attachments",
        &[
            "https://a.example",
            "https://b.example",
            "https://a.example",
        ],
    )
    .validate(BASE)
    .unwrap_err();
    assert_eq!(error.pointer, "/sources/0/attachments/origins/2");
    assert_eq!(error.message, "origins must be unique");

    let sixteen: Vec<String> = (0..MAX_CUSTOM_ATTACHMENT_ORIGINS)
        .map(|index| format!("https://host-{index}.example"))
        .collect();
    let mut allowed = setting("/attachments", &[]);
    allowed.origins = sixteen.clone();
    allowed.validate(BASE).unwrap();
    allowed.origins.push("https://host-16.example".into());
    assert_eq!(allowed.origins.len(), 17);
    assert_eq!(refused_at(allowed), "/sources/0/attachments/origins/16");
}

#[test]
fn the_longest_pointer_keeps_every_inline_locator_within_the_attachment_bound() {
    let longest = format!("/{}", "p".repeat(MAX_CUSTOM_ATTACHMENT_POINTER_BYTES - 1));
    let item: Attachment = serde_json::from_value(json!({
        "reference": "custom:15",
        "kind": "image",
        "source": {"type": "inline", "pointer": format!("{longest}/15/url")},
    }))
    .unwrap();
    validate_attachments(&[item], &SourceKind::Custom, "/kind/attachments").unwrap();
}

#[test]
fn the_setting_round_trips_and_refuses_unknown_fields() {
    let value = json!({"pointer":"/media/items","origins":["https://a.example"]});
    let parsed: CustomAttachments = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(parsed, setting("/media/items", &["https://a.example"]));
    assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    assert!(serde_json::from_value::<CustomAttachments>(
        json!({"pointer":"/attachments","origins":[],"wildcard":true})
    )
    .is_err());
}

fn channel(source: serde_json::Value) -> ChannelDefinition {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/channel.json")).unwrap();
    value["sources"] = json!([source]);
    let mut definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.profile_digest = definition.profile.digest();
    definition
}

#[test]
fn a_definition_validates_its_custom_setting_under_the_source_pointer() {
    let auth = json!({"type":"hmac","key_id":"key-1"});
    let definition = channel(json!({"type":"custom","auth":auth,
        "attachments":{"origins":["https://a.example"]}}));
    definition.validate().unwrap();
    let Source::Custom { attachments, .. } = &definition.sources[0] else {
        panic!("custom source expected");
    };
    assert_eq!(
        attachments.as_ref().unwrap().pointer,
        DEFAULT_CUSTOM_ATTACHMENT_POINTER
    );

    let invalid = channel(json!({"type":"custom","auth":auth,
        "attachments":{"pointer":"attachments","origins":["https://a.example/"]}}));
    assert_eq!(
        invalid.validate(),
        Err(ValidationError::new(
            "/sources/0/attachments/pointer",
            "must be an RFC 6901 JSON pointer starting with '/' without control characters",
        ))
    );
    let invalid = channel(json!({"type":"custom","auth":auth,
        "attachments":{"origins":["https://a.example/"]}}));
    assert_eq!(
        invalid.validate().unwrap_err().pointer,
        "/sources/0/attachments/origins/0"
    );
}

#[test]
fn a_definition_without_the_setting_keeps_its_serialized_bytes() {
    let definition = channel(json!({"type":"custom","auth":{"type":"hmac","key_id":"key-1"}}));
    let serialized = serde_json::to_value(&definition.sources[0]).unwrap();
    assert_eq!(
        serialized,
        json!({"type":"custom","auth":{"type":"hmac","key_id":"key-1"}})
    );
    assert!(serde_json::from_value::<Source>(json!({
        "type":"custom","auth":{"type":"hmac","key_id":"key-1"},"attachments":{"unknown":1}
    }))
    .is_err());
}
