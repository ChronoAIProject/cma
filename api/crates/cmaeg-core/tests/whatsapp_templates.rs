use cmaeg_core::message::OutboundMessage;
use serde_json::{json, Value};

fn message() -> Value {
    json!({"whatsapp_template":{"name":"order_update","language":{"code":"en_US"},"components":[
        {"type":"body","parameters":[{"type":"text","text":"Order 42"}]},
        {"type":"button","sub_type":"url","index":"0","parameters":[{"type":"text","text":"42"}]}
    ]}})
}
fn valid(value: Value) -> bool {
    serde_json::from_value::<OutboundMessage>(value).is_ok_and(|m| m.validate().is_ok())
}
#[test]
fn accepts_approved_template_shape_and_parameterless_templates() {
    assert!(valid(message()));
    assert!(valid(
        json!({"whatsapp_template":{"name":"hello_world","language":{"code":"en"}}})
    ));
}
#[test]
fn refuses_ambiguous_content_and_arbitrary_metadata_or_destinations() {
    for (key, value) in [
        ("text", json!("ignored text")),
        ("blocks", json!([{"type":"text","text":"ignored"}])),
        ("thread_reply", json!(true)),
        (
            "native_attachments",
            json!([{"kind":"image","source":{"type":"url","url":"https://example.com/image.png"}}]),
        ),
        ("metadata", json!({"to":"another recipient"})),
        ("to", json!("another recipient")),
    ] {
        let mut input = message();
        input[key] = value;
        assert!(!valid(input), "{key}");
    }
    for key in ["to", "recipient", "namespace", "metadata"] {
        let mut input = message();
        input["whatsapp_template"][key] = json!("override");
        assert!(!valid(input), "{key}");
    }
}
#[test]
fn bounds_names_languages_components_and_parameters() {
    for name in ["", "UpperCase", "bad-name", &"a".repeat(513)] {
        let mut input = message();
        input["whatsapp_template"]["name"] = json!(name);
        assert!(!valid(input));
    }
    for code in ["", "EN", "en-us", "en_US_extra", "english", "en_1A"] {
        let mut input = message();
        input["whatsapp_template"]["language"]["code"] = json!(code);
        assert!(!valid(input));
    }
    for components in [
        json!([{"type":"body","parameters":[]}]),
        json!([{"type":"footer","parameters":[{"type":"text","text":"x"}]}]),
        json!([{"type":"body","parameters":[{"type":"image","image":{"link":"https://example.com/a"}}]}]),
        json!([{"type":"button","sub_type":"quick_reply","index":"0","parameters":[{"type":"text","text":"x"}]}]),
        json!([{"type":"button","sub_type":"url","index":"3","parameters":[{"type":"text","text":"x"}]}]),
        json!([{"type":"body","parameters":[{"type":"text","text":"\n"}]}]),
        json!([{"type":"body","parameters":[{"type":"text","text":"x".repeat(1025)}]}]),
        json!([{"type":"body","parameters":vec![json!({"type":"text","text":"x"});21]}]),
        json!([{"type":"header","parameters":vec![json!({"type":"text","text":"x"});2]}]),
        json!([{"type":"body","parameters":[{"type":"text","text":"x"}]},{"type":"body","parameters":[{"type":"text","text":"x"}]}]),
    ] {
        let mut input = message();
        input["whatsapp_template"]["components"] = components;
        assert!(!valid(input));
    }
}
#[test]
fn preserves_legacy_message_bytes_when_no_template_is_present() {
    let original = json!({"text":"hello","blocks":[]});
    let parsed: OutboundMessage = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), original);
}
