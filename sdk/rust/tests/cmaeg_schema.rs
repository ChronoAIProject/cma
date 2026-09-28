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
fn roundtrip<T: DeserializeOwned + Serialize>(name: &str, examples: &Value) {
    for example in examples[name].as_array().unwrap() {
        let decoded: T = serde_json::from_value(example.clone())
            .unwrap_or_else(|_| panic!("cannot decode {name}"));
        assert_eq!(&serde_json::to_value(decoded).unwrap(), example, "{name}");
    }
}

#[test]
fn every_gateway_wire_type_replays_the_protocol_examples_without_losing_properties() {
    let examples = fixtures();
    let mut inventory = Vec::new();
    macro_rules! check { ($($ty:ident),+ $(,)?) => {$({ inventory.push(stringify!($ty)); roundtrip::<$ty>(stringify!($ty), &examples); })+}; }
    check!(
        AcceptedStatus,
        ActionMode,
        Activity,
        ActivityConversation,
        ActivityKind,
        Actor,
        ActorKind,
        AdmissionPolicy,
        AgentProfileSnapshot,
        EventWorkerProfile,
        ApiType,
        Attachment,
        AttachmentKind,
        BindingId,
        Block,
        BlockRendering,
        BlockRenderingOutcome,
        CallbackPayload,
        CallbackResponse,
        Capabilities,
        ChannelCreateRequest,
        ChannelCreation,
        ChannelId,
        ChannelResponse,
        ChannelUpdateRequest,
        ChoiceOption,
        CloudEvent,
        ConformanceLevel,
        ContextConfig,
        ContextResponse,
        ConversationId,
        ConversationKind,
        CredentialPatch,
        CredentialsRequest,
        CustomAuth,
        DeliveryOutcome,
        EndpointResponse,
        EventAccepted,
        EventContextSnapshot,
        EventId,
        EventRef,
        Field,
        FieldRefusal,
        FormField,
        FormFieldKind,
        GroupAdmission,
        HmacSecretRequest,
        IdentityAdmission,
        IntentReceipt,
        InteractionResult,
        McpAuth,
        McpServerRef,
        MessageRef,
        ModelAccess,
        ModelConfig,
        NyxIdIdentity,
        NyxIdIdentityKind,
        OptionStyle,
        OutboundMessage,
        PartitionKey,
        PartitionRule,
        PermissionConfig,
        Platform,
        Problem,
        ProblemCode,
        ProfileMetadata,
        ProviderBinding,
        ProviderRequest,
        ProviderTarget,
        ProviderResponse,
        ProviderSlug,
        PublicActivity,
        PublicChannelDefinition,
        PublicSecrets,
        RecordId,
        Refusal,
        RekeyResponse,
        RelayAgent,
        RelayAttachment,
        RelayContent,
        RelayConversation,
        RelaySender,
        RenderingOutcome,
        RenderingReport,
        ReplyPolicy,
        ReplyTargetRef,
        ReplyTargetResponse,
        Secret,
        Sink,
        SkillRef,
        SkillSource,
        Source,
        SourceKind,
        SourceRequest,
        TestChannelResponse,
        ThinkingEffort,
        TypingOutcome,
        TypingReceipt
    );
    inventory.sort_unstable();
    assert_eq!(
        inventory,
        snapshot()["components"]["schemas"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
}

fn properties(schema: &Value, schemas: &Value) -> Value {
    if let Some(reference) = schema["$ref"].as_str() {
        return properties(&schemas[reference.rsplit('/').next().unwrap()], schemas);
    }
    if let Some(parts) = schema["allOf"].as_array() {
        let mut combined = json!({"type":"object","properties":{}});
        for part in parts {
            combined["properties"].as_object_mut().unwrap().extend(
                properties(part, schemas)["properties"]
                    .as_object()
                    .unwrap()
                    .clone(),
            );
        }
        return combined;
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
            let value = object
                .get(key)
                .ok_or_else(|| format!("{path}/{key}: missing example property"))?;
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
