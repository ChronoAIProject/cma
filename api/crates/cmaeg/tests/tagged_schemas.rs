//! Closed newtype payloads must admit their enclosing Serde discriminator.
use cmaeg::channels::SourceRequest;
use cmaeg_core::{
    activity::{ActivityKind, SourceKind},
    channel::Source,
};
use serde_json::{json, Value};
use std::sync::OnceLock;

fn schemas() -> &'static Value {
    static SCHEMAS: OnceLock<Value> = OnceLock::new();
    SCHEMAS.get_or_init(|| serde_json::from_str(&cmaeg::openapi::snapshot()).unwrap())
}

fn validator(name: &str) -> jsonschema::Validator {
    let document = json!({
        "$ref": format!("#/components/schemas/{name}"),
        "components": schemas()["components"],
    });
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&document)
        .unwrap()
}

fn samples() -> Vec<(&'static str, Value)> {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/sdk-contract.json")).unwrap();
    [
        ("Source", "native_lark"),
        ("SourceRequest", "native_lark"),
        ("SourceKind", "native_lark"),
        ("ActivityKind", "interaction"),
    ]
    .into_iter()
    .map(|(name, tag)| {
        let example = fixtures[name]
            .as_array()
            .unwrap()
            .iter()
            .find(|example| example["type"] == tag)
            .unwrap()
            .clone();
        (name, example)
    })
    .collect()
}

#[test]
fn generated_schema_validates_every_closed_tagged_newtype_and_refuses_invalid_inputs() {
    for (name, example) in samples() {
        let validator = validator(name);
        assert!(validator.is_valid(&example), "{name}");
        // Exercise the same actual wire types, without a schema-only mirror.
        match name {
            "Source" => {
                serde_json::from_value::<Source>(example.clone()).unwrap();
            }
            "SourceRequest" => {
                serde_json::from_value::<SourceRequest>(example.clone()).unwrap();
            }
            "SourceKind" => {
                serde_json::from_value::<SourceKind>(example.clone()).unwrap();
            }
            "ActivityKind" => {
                serde_json::from_value::<ActivityKind>(example.clone()).unwrap();
            }
            _ => unreachable!(),
        }
        let mut missing = example.clone();
        missing.as_object_mut().unwrap().remove("type");
        assert!(!validator.is_valid(&missing), "missing tag: {name}");
        let mut wrong = example.clone();
        wrong["type"] = json!("unknown_variant");
        assert!(!validator.is_valid(&wrong), "wrong tag: {name}");
        let mut extra = example;
        extra["unexpected_field"] = json!(true);
        assert!(!validator.is_valid(&extra), "unknown field: {name}");
    }
}

#[test]
fn native_lark_create_body_validates_against_the_generated_public_contract() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/sdk-contract.json")).unwrap();
    let mut request = fixtures["ChannelCreateRequest"][0].clone();
    let source = samples()
        .into_iter()
        .find(|(name, _)| *name == "SourceRequest")
        .unwrap()
        .1;
    request["sources"] = json!([source]);
    request["credentials"] = json!({
        "agent_key": fixtures["ChannelCreateRequest"][0]["credentials"]["agent_key"],
        "native_lark": fixtures["NativeLarkCredentials"][0],
    });
    request
        .as_object_mut()
        .unwrap()
        .remove("durable_custom_execution");
    request.as_object_mut().unwrap().remove("connection_ref");
    request
        .as_object_mut()
        .unwrap()
        .remove("connection_admission");
    let decoded =
        serde_json::from_value::<cmaeg::channels::ChannelCreateRequest>(request.clone()).unwrap();
    let SourceRequest::NativeLark(source) = &decoded.sources[0] else {
        unreachable!()
    };
    source
        .validate(decoded.credentials.native_lark.as_ref())
        .unwrap();
    assert!(validator("ChannelCreateRequest").is_valid(&request));
    request["sources"][0]["unexpected_field"] = json!(true);
    assert!(!validator("ChannelCreateRequest").is_valid(&request));
}

#[test]
fn generated_schema_has_no_closed_all_of_payload_that_excludes_its_tag() {
    fn audit(value: &Value, path: &str, schemas: &Value) {
        match value {
            Value::Object(object) => {
                if let Some(parts) = object.get("allOf").and_then(Value::as_array) {
                    for tag_part in parts {
                        let Some(properties) =
                            tag_part.get("properties").and_then(Value::as_object)
                        else {
                            continue;
                        };
                        for (tag, definition) in properties {
                            if definition.get("enum").is_none() {
                                continue;
                            }
                            for part in parts {
                                let payload = part
                                    .get("$ref")
                                    .and_then(Value::as_str)
                                    .map(|reference| {
                                        schemas
                                            .pointer(reference.strip_prefix('#').unwrap())
                                            .unwrap()
                                    })
                                    .unwrap_or(part);
                                if payload.get("additionalProperties") == Some(&json!(false)) {
                                    assert!(
                                        payload["properties"].get(tag).is_some(),
                                        "{path} excludes tag {tag}"
                                    );
                                }
                            }
                        }
                    }
                }
                for (key, child) in object {
                    audit(child, &format!("{path}/{key}"), schemas);
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    audit(child, &format!("{path}/{index}"), schemas);
                }
            }
            _ => {}
        }
    }
    audit(schemas(), "", schemas());
}

#[test]
fn standalone_payloads_remain_closed_and_do_not_accept_an_enum_tag() {
    for (name, mut example) in samples()
        .into_iter()
        .filter(|(name, _)| *name != "SourceRequest")
    {
        let payload = match name {
            "Source" => "NativeLarkSource",
            "SourceKind" => "NativeLarkIdentity",
            "ActivityKind" => "InteractionResult",
            _ => unreachable!(),
        };
        let validator = validator(payload);
        assert!(!validator.is_valid(&example), "standalone tag: {payload}");
        example.as_object_mut().unwrap().remove("type");
        assert!(
            validator.is_valid(&example),
            "standalone payload: {payload}"
        );
    }
}
