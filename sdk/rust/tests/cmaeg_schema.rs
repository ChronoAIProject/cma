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
fn historical_activity_receipts_roundtrip_without_inventing_event_facts() {
    let mut page = fixtures()["ActivityPage"][0].clone();
    for row in page["items"].as_array_mut().unwrap() {
        for key in ["kind", "event_type", "source_occurred_at"] {
            row.as_object_mut().unwrap().remove(key);
        }
    }
    let decoded: ActivityPage = serde_json::from_value(page.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), page);
}

#[test]
fn legacy_relay_sources_never_gain_a_resource_binding_on_roundtrip() {
    for platform in ["telegram", "lark", "feishu", "discord", "whatsapp", "slack"] {
        let request = json!({"type":"nyxid_relay","platform":platform,"issuer":"https://nyx.example","key_id":"key-fixture","route_ids":["route-fixture"],"admission":{"type":"all"}});
        let decoded: SourceRequest = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), request);
        let mut source = request;
        source["jwks_url"] = json!("https://nyx.example/.well-known/jwks.json");
        let decoded: Source = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), source);
    }
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
        FormCustomerMessage,
        FormPreparationReceipt,
        FormPreparationState,
        AcceptedStatus,
        ActionMode,
        Activity,
        ActivityConsumer,
        ActivityConversation,
        ActivityDelivery,
        ActivityKind,
        ActivityPage,
        ActivityRecipient,
        Actor,
        ActorKind,
        AdmissionPolicy,
        AgentProfileSnapshot,
        EventWorkerProfile,
        ApiType,
        Attachment,
        AttachmentKind,
        AttachmentSource,
        LarkRegion,
        NativeLarkIdentity,
        NativeLarkCredentials,
        NativeLarkSource,
        LarkResourceBinding,
        LarkMessageEffectsBinding,
        LarkResourceKind,
        AttachmentImage,
        AttachmentImageOrigin,
        AttachmentStatus,
        AttachmentText,
        AttachmentTextSource,
        AttachmentUnavailableReason,
        AttachmentView,
        AttachmentViewKind,
        BindingId,
        Block,
        BlockRendering,
        BlockRenderingOutcome,
        BotMessageAdmission,
        CallbackPayload,
        CallbackResponse,
        Capabilities,
        CapabilityView,
        AgentToolCapability,
        RelayAuthoringCapabilities,
        RelayPlatformCapability,
        RelayVerificationCapability,
        ResourcePlatform,
        ResourceConnectionSource,
        ResourceConnection,
        ResourceConnectionList,
        ConnectionPage,
        ConnectionView,
        CreateConnection,
        PlatformCapability,
        Status,
        ChannelCreateRequest,
        ChannelActivity,
        ChannelCreation,
        ChannelId,
        ChannelListItem,
        ChannelPage,
        ChannelResponse,
        ChannelUpdateRequest,
        ChoiceOption,
        CloudEvent,
        ConformanceLevel,
        ContextConfig,
        ContextResponse,
        ReceivedMessageContext,
        ThreadHistory,
        ThreadRef,
        ThreadListView,
        CreateThreadRequest,
        ThreadAction,
        ThreadTarget,
        ManageThreadRequest,
        ThreadSummary,
        ThreadListScope,
        ThreadList,
        ThreadOutcome,
        ThreadReceipt,
        ThreadCapabilities,
        TopicAction,
        TopicLifecycleCapabilities,
        HistoryCursor,
        HistoryAttachmentFacts,
        HistoryOmittedContent,
        HistoryMessage,
        HistoryContinuation,
        HistoryContentCoverage,
        ConversationHistoryPage,
        HistoryDirection,
        ConversationHistoryCapabilities,
        ThreadHistoryMessage,
        ConversationId,
        ConversationKind,
        CredentialPatch,
        CredentialsRequest,
        WhatsappCredentials,
        WhatsappWebhookResponse,
        CustomAttachments,
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
        InboundMediaCapabilities,
        IntentReceipt,
        EmptyEffectRequest,
        MessageEffectReceipt,
        MessageEffectsCapabilities,
        MessageReactionRequest,
        TelegramReaction,
        InteractionResult,
        McpAuth,
        McpServerRef,
        MediaDownload,
        MediaVariant,
        MessageRef,
        ModelAccess,
        ModelConfig,
        NyxIdIdentity,
        NyxIdIdentityKind,
        OptionStyle,
        OutboundMessage,
        PrepareOutboundMessage,
        OutboundReceipt,
        OutboundStatus,
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
        ThreadAddress,
        ThreadContract,
        ThreadFacts,
        ThreadKind,
        TypingOutcome,
        TypingReceipt,
        VisualSupport,
        ActivityKindName,
        Addressing,
        AgentEventPolicy,
        Consumer,
        ConsumerId,
        ConsumerName,
        ConsumerReply,
        ConsumerState,
        DefaultConsumer,
        DefaultReply,
        Pattern,
        PublishPolicy,
        ReplyChoice,
        ReplyLabel,
        RoutingPolicy,
        RoutingRule,
        RuleId,
        RuleMatch,
        SourceId,
        ActivityRouting,
        Causation,
        ConsumerSummary,
        ConsumerTarget,
        RouteResolution,
        WhatsappTemplate,
        WhatsappTemplateLanguage,
        WhatsappTemplateParameter,
        WhatsappTemplateMedia,
        WhatsappTemplateDocument,
        WhatsappTemplateCurrency,
        WhatsappTemplateDateTime,
        WhatsappTemplateButtonType,
        WhatsappTemplateComponent,
        NativeDiscordActivation,
        NativeAttachment,
        NativeMediaKind,
        NativeMediaSource,
        NativeMediaCapabilities,
        MediaControlsCapabilities,
        NativeMediaUpdate,
        NativeMediaReplacement,
        NativeMediaEditCapabilities,
        NativeStickerCapabilities,
        NativeVideoCapabilities,
        SourceCheckResponse,
        SourceCheck,
        SourceCheckStatus,
        RoutingUpdateRequest,
        RoutingPolicyRequest,
        ConsumerPolicyRequest
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
