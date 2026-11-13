#[path = "cmaeg_schema/inventory.rs"]
mod inventory;
#[path = "cmaeg_schema/legacy.rs"]
mod legacy;
#[path = "cmaeg_schema/upstream.rs"]
mod upstream;

use cma_sdk::cmaeg::types::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn snapshot() -> Value {
    serde_json::from_str(include_str!("../../../api/crates/cmaeg/openapi/v1.json")).unwrap()
}
fn fixtures() -> Value {
    serde_json::from_str(include_str!(
        "../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json"
    ))
    .unwrap()
}

#[test]
fn verified_sender_policy_stays_opaque_and_activity_refusal_is_additive() {
    for enabled in [true, false] {
        let mut profile = fixtures()["EventWorkerProfile"][0].clone();
        profile["environment"] = json!({"verified_sender_context":enabled});
        let decoded: EventWorkerProfile = serde_json::from_value(profile.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), profile);
    }
    let mut page = fixtures()["ActivityPage"]
        .as_array()
        .unwrap()
        .iter()
        .find(|page| {
            page["items"].as_array().is_some_and(|rows| {
                rows.first().is_some_and(|row| {
                    row["consumers"]
                        .as_array()
                        .is_some_and(|consumers| !consumers.is_empty())
                })
            })
        })
        .unwrap()
        .clone();
    page["items"][0]["consumers"][0]["refusal_code"] = "verified_sender_context_invalid".into();
    let decoded: ActivityPage = serde_json::from_value(page.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), page);
}

fn roundtrip<T: DeserializeOwned + Serialize>(name: &str, examples: &Value) {
    for example in examples[name].as_array().unwrap() {
        let decoded: T = serde_json::from_value(example.clone())
            .unwrap_or_else(|_| panic!("cannot decode {name}"));
        assert_eq!(&serde_json::to_value(decoded).unwrap(), example, "{name}");
    }
}

#[test]
fn github_nullable_objects_preserve_explicit_null_separately_from_absence() {
    let mut examples = fixtures();
    examples["AppIntegration"][0]["verification"] = Value::Null;
    examples["Task"][0]["result"] = Value::Null;
    examples["WebhookCredentialStatus"][0]["previous"] = Value::Null;
    roundtrip::<AppIntegration>("AppIntegration", &examples);
    roundtrip::<Task>("Task", &examples);
    roundtrip::<WebhookCredentialStatus>("WebhookCredentialStatus", &examples);
}

#[test]
fn examples_exercise_every_snapshot_property_including_optional_and_nested_fields() {
    let snapshot = snapshot();
    let schemas = &snapshot["components"]["schemas"];
    let fixtures = fixtures();
    assert_eq!(
        schemas.as_object().unwrap().keys().collect::<Vec<_>>(),
        fixtures.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    for (name, examples) in fixtures.as_object().unwrap() {
        check_property_coverage(&schemas[name], examples.as_array().unwrap(), schemas, name);
        for example in examples.as_array().unwrap() {
            check_shape(&schemas[name], example, schemas, name).unwrap();
        }
    }
}

#[test]
fn removing_a_snapshot_property_fails_the_sdk_contract_check() {
    let mut snapshot = snapshot();
    let schemas = &mut snapshot["components"]["schemas"];
    schemas["ContextResponse"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("capabilities");
    assert!(
        check_shape(
            &schemas["ContextResponse"],
            &fixtures()["ContextResponse"][0],
            schemas,
            "ContextResponse"
        )
        .unwrap_err()
        .contains("removed schema property")
    );
}

#[test]
#[should_panic(expected = "quoted_message: missing example property coverage")]
fn omitting_an_optional_property_from_every_example_fails_contract_coverage() {
    let snapshot = snapshot();
    let schemas = &snapshot["components"]["schemas"];
    let mut examples = fixtures()["ActivityKind"].as_array().unwrap().clone();
    for example in &mut examples {
        example.as_object_mut().unwrap().remove("quoted_message");
    }
    check_property_coverage(&schemas["ActivityKind"], &examples, schemas, "ActivityKind");
}

#[test]
fn a_channel_without_routing_decodes_and_encodes_without_a_routing_key() {
    let mut example = fixtures()["ChannelResponse"][0].clone();
    assert!(example["definition"]["routing"].is_object());
    let routed: ChannelResponse = serde_json::from_value(example.clone()).unwrap();
    let routing = routed.definition.routing.as_ref().unwrap();
    assert_eq!(
        routing.consumers.as_ref().unwrap()[0].agent_key_hash,
        "fixture"
    );
    example["definition"]
        .as_object_mut()
        .unwrap()
        .remove("routing");
    let legacy: ChannelResponse = serde_json::from_value(example.clone()).unwrap();
    assert!(legacy.definition.routing.is_none());
    assert_eq!(serde_json::to_value(legacy).unwrap(), example);
}

#[test]
fn an_activity_without_routing_decodes_and_encodes_without_a_routing_key() {
    let examples = fixtures();
    for (name, pointer) in [
        ("Activity", ""),
        ("EventContextSnapshot", "/activity"),
        ("PublicActivity", ""),
        ("ContextResponse", "/activity"),
    ] {
        let mut example = examples[name][0].clone();
        let activity = example.pointer_mut(pointer).unwrap();
        assert!(activity["routing"].is_object(), "{name} example is routed");
        activity.as_object_mut().unwrap().remove("routing");
        let encoded = match name {
            "Activity" => {
                let legacy: Activity = serde_json::from_value(example.clone()).unwrap();
                assert!(legacy.routing.is_none());
                serde_json::to_value(legacy).unwrap()
            }
            "EventContextSnapshot" => {
                let legacy: EventContextSnapshot = serde_json::from_value(example.clone()).unwrap();
                assert!(legacy.activity.routing.is_none());
                serde_json::to_value(legacy).unwrap()
            }
            "PublicActivity" => {
                let legacy: PublicActivity = serde_json::from_value(example.clone()).unwrap();
                assert!(legacy.routing.is_none());
                serde_json::to_value(legacy).unwrap()
            }
            _ => {
                let legacy: ContextResponse = serde_json::from_value(example.clone()).unwrap();
                assert!(legacy.activity.routing.is_none());
                serde_json::to_value(legacy).unwrap()
            }
        };
        assert_eq!(encoded, example, "{name}");
    }
}

#[test]
fn a_routed_activity_exposes_its_targets_and_causation() {
    let activity: Activity = serde_json::from_value(fixtures()["Activity"][0].clone()).unwrap();
    let routing = activity.routing.unwrap();
    assert!(routing.resolution == RouteResolution::AgentEvent);
    assert_eq!(routing.targets[0].id, "csm_00000000000a");
    let causation = routing.causation.unwrap();
    assert_eq!(causation.hop, 2);
    assert!(causation.reply == ReplyChoice::Default);
    let mut unknown = fixtures()["RouteResolution"][0].clone();
    unknown = serde_json::Value::String(format!("{}_v2", unknown.as_str().unwrap()));
    assert!(serde_json::from_value::<RouteResolution>(unknown).is_err());
}

#[test]
fn a_context_response_names_its_consumer_only_on_a_routed_channel() {
    let examples = fixtures();
    let decode =
        |example: &Value| -> ContextResponse { serde_json::from_value(example.clone()).unwrap() };
    let routed = decode(&examples["ContextResponse"][0]);
    let consumer = routed.consumer.as_ref().unwrap();
    assert_eq!(
        (
            consumer.id.as_str(),
            consumer.name.as_str(),
            consumer.label.as_str()
        ),
        ("csm_00000000000a", "reviewer", "Reviewer")
    );
    // A hand-off's controls can never be clicked, so it projects none.
    let handoff = decode(&examples["ContextResponse"][1]);
    assert_eq!(
        (
            handoff.capabilities.buttons.as_str(),
            handoff.capabilities.cards.as_str()
        ),
        ("none", "none")
    );
    let mut legacy = examples["ContextResponse"][0].clone();
    legacy.as_object_mut().unwrap().remove("consumer");
    legacy.as_object_mut().unwrap().remove("received_message");
    let decoded = decode(&legacy);
    assert!(decoded.consumer.is_none());
    assert!(decoded.received_message.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
}

#[test]
fn legacy_channel_listing_omits_unknown_ids_and_endpoints() {
    let mut example = fixtures()["ChannelPage"][0].clone();
    let item = example["items"][0].as_object_mut().unwrap();
    item.remove("channel_id");
    item.remove("endpoints");
    item["definition"]
        .as_object_mut()
        .unwrap()
        .remove("creation");
    let decoded: ChannelPage = serde_json::from_value(example.clone()).unwrap();
    assert!(decoded.items[0].channel_id.is_none());
    assert!(decoded.items[0].endpoints.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), example);
}

#[test]
fn native_bot_extensions_preserve_legacy_omitted_defaults() {
    let capabilities = json!({"platforms":[]});
    let parsed: CapabilityView = serde_json::from_value(capabilities.clone()).unwrap();
    assert!(parsed.source_features.is_empty());
    assert_eq!(serde_json::to_value(parsed).unwrap(), capabilities);
    let policy = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention"});
    let parsed: AdmissionPolicy = serde_json::from_value(policy.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), policy);
}

#[test]
fn legacy_native_media_capabilities_omit_unknown_transport_limits() {
    let legacy = json!({"kinds":["image","file","audio","video"],"sources":["url","base64"],"max_attachments":4,"max_inline_bytes":65536});
    let decoded: NativeMediaCapabilities = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
}

#[test]
fn native_media_control_capabilities_round_trip_without_changing_legacy_shape() {
    let value = json!({"kinds":["image"],"sources":["base64"],"max_attachments":4,"max_inline_bytes":65536,
        "controls":{"max_attachments":1,"buttons":true,"choices":true,"caption_max_chars":1024,"update":false}});
    let decoded: NativeMediaCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert!(!decoded.controls.as_ref().unwrap().update);
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn lark_multi_image_controls_preserve_bound_without_inventing_replacement_authority() {
    let value = json!({"kinds":["image"],"sources":["base64"],"max_attachments":4,"max_inline_bytes":65536,
        "controls":{"kinds":["image"],"max_attachments":4,"buttons":true,"choices":true,"caption_max_chars":0,"update":true}});
    let decoded: NativeMediaCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(decoded.controls.as_ref().unwrap().max_attachments, 4);
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn lark_replacement_capabilities_preserve_the_separate_issuance_bound() {
    let value = json!({"kinds":["image"],"sources":["base64"],"max_attachments":4,"max_inline_bytes":65536,
        "controls":{"kinds":["image"],"max_attachments":1,"buttons":true,"choices":true,"caption_max_chars":0,"update":true,"replace_media":true}});
    let decoded: NativeMediaCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(decoded.controls.as_ref().unwrap().replace_media, Some(true));
    assert_eq!(decoded.controls.as_ref().unwrap().max_attachments, 1);
    assert_eq!(decoded.max_attachments, 4);
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn native_album_capabilities_preserve_exact_groups_without_operation_authority() {
    let value = serde_json::json!({
        "kinds":["image","file","audio","video"], "sources":["base64"],
        "max_attachments":4, "max_inline_bytes":65536,
        "albums":{"min_attachments":2,"max_attachments":4,
            "kind_groups":[["image","video"],["file"],["audio"]],
            "controls":false,"message_operations":false}
    });
    let decoded: NativeMediaCapabilities = serde_json::from_value(value.clone()).unwrap();
    let albums = decoded.albums.as_ref().unwrap();
    assert!(!albums.controls && !albums.message_operations);
    assert_eq!(albums.kind_groups[0], ["image", "video"]);
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn explicit_media_presentations_preserve_legacy_audio_and_optional_discovery() {
    for kind in ["audio", "video", "voice", "animation", "sticker"] {
        let wire = json!({"native_attachments":[{"kind":kind,"source":{"type":"base64","data":"AQIDBA=="}}]});
        let decoded: OutboundMessage = serde_json::from_value(wire.clone()).unwrap();
        let encoded = serde_json::to_value(decoded).unwrap();
        assert_eq!(encoded["native_attachments"], wire["native_attachments"]);
    }
    let value = json!({"kinds":["audio","voice"],"sources":["base64"],"max_attachments":4,"max_inline_bytes":65536,
        "presentation_profiles":[{"kind":"voice","mime_types":["audio/ogg"],"max_bytes":8388608,"caption":false,
        "profile":"One complete mono Opus Ogg stream; no transcoding","message_operations":false}]});
    let decoded: NativeMediaCapabilities = serde_json::from_value(value.clone()).unwrap();
    assert!(!decoded.presentation_profiles.as_ref().unwrap()[0].message_operations);
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn discord_voice_constraints_round_trip_and_older_profile_omits_new_fields() {
    for value in fixtures()["NativePresentationProfile"].as_array().unwrap() {
        let decoded: NativePresentationProfile = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
    }
}

fn properties(schema: &Value, schemas: &Value) -> Value {
    if let Some(reference) = schema["$ref"].as_str() {
        return properties(&schemas[reference.rsplit('/').next().unwrap()], schemas);
    }
    if let Some(parts) = schema["allOf"].as_array() {
        let mut variants = vec![json!({"type":"object","properties":{}})];
        for part in parts {
            let expanded = properties(part, schemas);
            let options = expanded["oneOf"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![expanded]);
            let mut next = Vec::new();
            for variant in &variants {
                for option in &options {
                    let mut merged = variant.clone();
                    merged["properties"].as_object_mut().unwrap().extend(
                        properties(option, schemas)["properties"]
                            .as_object()
                            .unwrap()
                            .clone(),
                    );
                    next.push(merged);
                }
            }
            variants = next;
        }
        return if variants.len() == 1 {
            variants.pop().unwrap()
        } else {
            json!({"oneOf":variants})
        };
    }
    schema.clone()
}

fn check_shape(schema: &Value, example: &Value, schemas: &Value, path: &str) -> Result<(), String> {
    if let Some(reference) = schema["$ref"].as_str() {
        return check_shape(
            &schemas[reference.rsplit('/').next().unwrap()],
            example,
            schemas,
            path,
        );
    }
    if schema["allOf"].is_array() {
        return check_shape(&properties(schema, schemas), example, schemas, path);
    }
    if let Some(variants) = schema["oneOf"].as_array() {
        let nonnull: Vec<_> = variants.iter().filter(|v| v["type"] != "null").collect();
        if nonnull.len() == 1 {
            return check_shape(nonnull[0], example, schemas, path);
        }
        for variant in nonnull {
            let expanded = properties(variant, schemas);
            let discriminator = expanded["properties"]
                .as_object()
                .unwrap()
                .iter()
                .find(|(_, v)| v["enum"].is_array());
            if discriminator.is_some_and(|(k, v)| example[k] == v["enum"][0]) {
                return check_shape(&expanded, example, schemas, path);
            }
        }
        return Err(format!("{path}: no matching variant"));
    }
    if let Some(fields) = schema["properties"].as_object() {
        let object = example
            .as_object()
            .ok_or_else(|| format!("{path}: expected object"))?;
        for (key, field) in fields {
            let Some(value) = object.get(key) else {
                if schema["required"]
                    .as_array()
                    .is_some_and(|required| required.iter().any(|field| field == key))
                {
                    return Err(format!("{path}/{key}: missing required example property"));
                }
                continue;
            };
            check_shape(field, value, schemas, &format!("{path}/{key}"))?;
        }
        if schema
            .get("additionalProperties")
            .is_none_or(|v| v == false)
            && object.keys().any(|k| !fields.contains_key(k))
        {
            return Err(format!(
                "{path}: SDK example retains a removed schema property"
            ));
        }
    }
    if schema.get("items").is_some() {
        for item in example
            .as_array()
            .ok_or_else(|| format!("{path}: expected array"))?
        {
            check_shape(&schema["items"], item, schemas, path)?;
        }
    }
    Ok(())
}

// Optional additive fields may be absent from legacy examples. Require their
// coverage across the complete example set, while check_shape validates every
// present field and recursively rejects properties removed from the snapshot.
fn check_property_coverage(schema: &Value, examples: &[Value], schemas: &Value, path: &str) {
    // Referenced types have their own complete top-level example collection.
    if schema["$ref"].is_string() {
        return;
    }
    let schema = properties(schema, schemas);
    if let Some(variants) = schema["oneOf"].as_array() {
        for variant in variants.iter().filter(|variant| variant["type"] != "null") {
            if variant["$ref"].is_string() {
                continue;
            }
            let expanded = properties(variant, schemas);
            let discriminator = expanded["properties"]
                .as_object()
                .and_then(|fields| fields.iter().find(|(_, field)| field["enum"].is_array()));
            let matching: Vec<_> = examples
                .iter()
                .filter(|example| {
                    discriminator.is_none_or(|(key, field)| example[key] == field["enum"][0])
                })
                .cloned()
                .collect();
            assert!(!matching.is_empty(), "{path}: missing variant examples");
            check_property_coverage(&expanded, &matching, schemas, path);
        }
    }
    if let Some(fields) = schema["properties"].as_object() {
        for (key, field) in fields {
            let values: Vec<_> = examples
                .iter()
                .filter_map(|example| example.get(key).cloned())
                .collect();
            assert!(
                !values.is_empty(),
                "{path}/{key}: missing example property coverage"
            );
            check_property_coverage(field, &values, schemas, &format!("{path}/{key}"));
        }
    }
    if let Some(items) = schema.get("items") {
        let values: Vec<_> = examples
            .iter()
            .filter_map(Value::as_array)
            .flatten()
            .cloned()
            .collect();
        check_property_coverage(items, &values, schemas, path);
    }
}
