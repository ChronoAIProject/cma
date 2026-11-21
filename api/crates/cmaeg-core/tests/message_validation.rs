use cmaeg_core::message::OutboundMessage;
use serde_json::{json, Value};

fn validate(value: Value) -> Result<(), cmaeg_core::ValidationError> {
    serde_json::from_value::<OutboundMessage>(value)
        .unwrap()
        .validate()
}

fn message(block: Value) -> Value {
    json!({"blocks":[block]})
}

fn option(index: usize) -> Value {
    json!({"id":format!("option-{index}"),"label":"Option"})
}

#[test]
fn text_limits_count_utf8_bytes_across_all_message_shapes() {
    for (base, pointer, min) in [
        (json!({"text":""}), "/text", 0),
        (
            message(json!({"type":"text","text":""})),
            "/blocks/0/text",
            0,
        ),
        (
            message(json!({"type":"card","title":"Title","body":""})),
            "/blocks/0/body",
            0,
        ),
        (
            message(
                json!({"type":"choice","id":"pick","prompt":"Pick","options":[option(0)],"multi":true}),
            ),
            "/blocks/0/prompt",
            1,
        ),
        (
            message(json!({"type":"attachment","reference":"asset","kind":"image","caption":""})),
            "/blocks/0/caption",
            0,
        ),
    ] {
        let mut value = base;
        for size in [min, 32 * 1024] {
            *value.pointer_mut(pointer).unwrap() = json!("x".repeat(size));
            validate(value.clone()).unwrap();
        }
        *value.pointer_mut(pointer).unwrap() = json!("é".repeat(16 * 1024 + 1));
        assert_eq!(validate(value.clone()).unwrap_err().pointer, pointer);
        if min > 0 {
            *value.pointer_mut(pointer).unwrap() = json!("");
            assert_eq!(validate(value).unwrap_err().pointer, pointer);
        }
    }
}

#[test]
fn rejects_block_card_and_form_collection_overflow() {
    let mut value = json!({"blocks":vec![json!({"type":"text","text":""});16]});
    validate(value.clone()).unwrap();
    value["blocks"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"text","text":"extra"}));
    assert_eq!(validate(value).unwrap_err().pointer, "/blocks");
    let mut value = message(
        json!({"type":"card","title":"Card","fields":vec![json!({"label":"Field","value":"Value"});16],"image":"https://example.test/image"}),
    );
    validate(value.clone()).unwrap();
    value["blocks"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"label":"Extra","value":"Value"}));
    assert_eq!(validate(value).unwrap_err().pointer, "/blocks/0");
    for count in [0, 12, 13] {
        let value = message(
            json!({"type":"form","id":"form","fields":(0..count).map(|index|json!({"id":format!("field-{index}"),"label":"Field","kind":{"type":"text"},"required":false})).collect::<Vec<_>>()}),
        );
        if count == 12 {
            validate(value).unwrap();
        } else {
            assert_eq!(validate(value).unwrap_err().pointer, "/blocks/0/fields");
        }
    }
}

#[test]
fn rejects_empty_duplicate_and_excessive_button_or_choice_options() {
    for (kind, limit) in [("buttons", 8), ("choice", 20)] {
        let mut block =
            json!({"type":kind,"id":"pick","options":(0..limit).map(option).collect::<Vec<_>>()});
        if kind == "choice" {
            block["prompt"] = json!("Pick");
            block["multi"] = json!(false);
        }
        validate(message(block.clone())).unwrap();
        for options in [
            vec![],
            vec![option(0), option(0)],
            (0..=limit).map(option).collect(),
        ] {
            block["options"] = json!(options);
            assert_eq!(
                validate(message(block.clone())).unwrap_err().pointer,
                "/blocks/0/options"
            );
        }
    }
}

#[test]
fn rejects_duplicate_block_ids_across_different_block_kinds() {
    let value = json!({"blocks":[
        {"type":"buttons","id":"same","options":[option(0)]},
        {"type":"form","id":"same","fields":[{"id":"field","label":"Field","kind":{"type":"number"},"required":true}]}
    ]});
    assert_eq!(validate(value).unwrap_err().pointer, "/blocks/1/id");
}

#[test]
fn rejects_empty_and_overlong_message_identifiers_and_labels() {
    for (base, pointer, max) in [
        (
            json!({"type":"card","title":"Title"}),
            "/blocks/0/title",
            200,
        ),
        (
            json!({"type":"buttons","id":"pick","options":[option(0)]}),
            "/blocks/0/id",
            512,
        ),
        (
            json!({"type":"buttons","id":"pick","options":[option(0)]}),
            "/blocks/0/options/0/id",
            512,
        ),
        (
            json!({"type":"buttons","id":"pick","options":[option(0)]}),
            "/blocks/0/options/0/label",
            64,
        ),
        (
            json!({"type":"form","id":"form","fields":[{"id":"field","label":"Field","kind":{"type":"date"},"required":true}]}),
            "/blocks/0/fields/0/id",
            512,
        ),
        (
            json!({"type":"form","id":"form","fields":[{"id":"field","label":"Field","kind":{"type":"text"},"required":false}]}),
            "/blocks/0/fields/0/label",
            200,
        ),
        (
            json!({"type":"attachment","reference":"asset","kind":"other"}),
            "/blocks/0/reference",
            4096,
        ),
        (
            json!({"type":"link","url":"https://example.test","label":"Link"}),
            "/blocks/0/label",
            200,
        ),
    ] {
        let mut value = message(base);
        *value.pointer_mut(pointer).unwrap() = json!("x".repeat(max));
        validate(value.clone()).unwrap();
        for size in [0, max + 1] {
            *value.pointer_mut(pointer).unwrap() = json!("x".repeat(size));
            assert_eq!(validate(value.clone()).unwrap_err().pointer, pointer);
        }
    }
}

#[test]
fn rejects_unsafe_card_images_and_link_urls() {
    for url in [
        "http://example.test",
        "file:///etc/passwd",
        "https://",
        "relative",
        "https://example.test/space here",
    ] {
        for (block, pointer) in [
            (
                json!({"type":"card","title":"Card","image":url}),
                "/blocks/0/image",
            ),
            (json!({"type":"link","url":url}), "/blocks/0/url"),
        ] {
            assert_eq!(validate(message(block)).unwrap_err().pointer, pointer);
        }
    }
}
