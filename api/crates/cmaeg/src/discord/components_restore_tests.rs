//! Durable component restoration separates frozen selection from current authority.
use super::{
    components_fixture::{message, FixtureState},
    GatewayError, InboxLaunch, InboxReader, VerifiedInboxLaunch,
};
use crate::channels::{definition::hash_secret, resolver::ResolveError, routing_fixture};
use cmaeg_core::{ids::ConsumerId, seal::ReplyAddress};
use serde_json::{json, Value};

async fn replace(f: &mut FixtureState, value: Value) {
    f.setup.channel = serde_json::from_value(value).unwrap();
    f.setup.fixture.replace(&f.setup.channel);
    f.channel = f
        .setup
        .state
        .resolver
        .resolve_current(f.channel.id.as_str())
        .await
        .unwrap();
}

async fn named_pair() -> FixtureState {
    let mut f = FixtureState::named().await;
    let mut value = f.setup.channel.to_blob_json();
    value["routing"]["consumers"]
        .as_array_mut()
        .unwrap()
        .push(routing_fixture::consumer(
            routing_fixture::NYX,
            "other",
            "cma_codex",
        ));
    value["secrets"]["consumer_keys"]
        .as_array_mut()
        .unwrap()
        .push(json!([
            routing_fixture::NYX,
            routing_fixture::agent_key(routing_fixture::NYX)
        ]));
    replace(&mut f, value).await;
    f.settle_context_message().await;
    f
}

async fn persisted(f: &FixtureState) -> String {
    let card = f.card(&message(false)).await;
    let raw = f.callback(
        card.markup[0]["components"][0]["custom_id"]
            .as_str()
            .unwrap(),
    );
    f.receive(raw.clone(), 2).await.unwrap();
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_some());
    assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
    assert!(f.provider.memory.lock().unwrap().seen.is_empty());
    f.id(&raw)
}

fn reader(f: &FixtureState) -> InboxReader {
    InboxReader::new(f.setup.fixture.store.channels.clone(), f.keys())
}

async fn load(f: &FixtureState, id: &str) -> Box<VerifiedInboxLaunch> {
    let InboxLaunch::Launch(verified) = reader(f)
        .load(id, f.channel.clone(), f.setup.now())
        .await
        .unwrap()
    else {
        panic!("persisted callback must launch")
    };
    verified
}

fn rotate(value: &mut Value, index: usize, key: &str) {
    value["routing"]["consumers"][index]["agent_key_hash"] = hash_secret(key).into();
    value["secrets"]["consumer_keys"][index][1] = key.into();
}

#[tokio::test]
async fn native_component_restore_admits_after_unrelated_edits_and_valid_key_rotations() {
    for change in 0..4 {
        let mut f = named_pair().await;
        let id = persisted(&f).await;
        let first = load(&f, &id).await;
        let frozen: Value = serde_json::from_slice(&first.source_bytes).unwrap();
        assert_eq!(
            frozen["channel"]["routing"]["consumers"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let receipt = f
            .activity
            .latest(&f.setup.channel.owner.subject, &f.setup.fixture.record)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|item| item.event_id == first.request.activity.event_id.as_str())
            .unwrap();
        assert!(receipt.consumers.is_empty());

        let mut value = f.setup.channel.to_blob_json();
        match change {
            0 => {
                value["routing"]["consumers"]
                    .as_array_mut()
                    .unwrap()
                    .remove(1);
                value["secrets"]["consumer_keys"]
                    .as_array_mut()
                    .unwrap()
                    .remove(1);
            }
            1 => rotate(&mut value, 1, "rotated-unrelated-fixture"),
            2 => rotate(&mut value, 0, "rotated-selected-fixture"),
            _ => {
                value["secrets"]["agent_key_hash"] = hash_secret("rotated-default-fixture").into();
                value["secrets"]["provider_credential"] = "rotated-default-fixture".into();
            }
        }
        value["routing"]["default_targets"] = json!(["default"]);
        value["routing"]["consumers"][0]["name"] = "renamed".into();
        value["partition"]["key"] = "conversation".into();
        replace(&mut f, value).await;

        let restored = load(&f, &id).await;
        let serving = &restored.request.channel;
        assert_eq!(serving.definition, f.channel.definition);
        serving.definition.validate().unwrap();
        crate::channels::definition::validate_consumer_keys(&serving.definition).unwrap();
        assert_eq!(serving.consumer_keys, f.channel.consumer_keys);
        assert_eq!(serving.version, f.channel.version);
        if change == 0 {
            let other = ConsumerId::parse(routing_fixture::NYX).unwrap();
            assert!(serving.consumer(Some(&other)).is_none());
            assert!(serving.consumer_key_hash(Some(&other)).is_none());
        }
        if change == 2 {
            assert_eq!(
                serving
                    .consumer(f.original.consumer.as_ref())
                    .unwrap()
                    .credential
                    .unwrap()
                    .as_str(),
                "rotated-selected-fixture"
            );
        }
        assert_eq!(restored.source_bytes, first.source_bytes);
        assert_eq!(restored.received_at, first.received_at);
        assert_eq!(restored.request.activity, first.request.activity);
        assert_eq!(
            restored.request.idempotency_key,
            first.request.idempotency_key
        );
        assert_eq!(restored.request.consumer, f.original.consumer);
        assert_eq!(restored.request.conversation_id, f.original.conversation_id);
        let event = restored
            .request
            .event_ref
            .open(&f.keys(), f.setup.now())
            .unwrap()
            .payload;
        let original_event = first
            .request
            .event_ref
            .open(&f.keys(), f.setup.now())
            .unwrap()
            .payload;
        assert_eq!(event, original_event);
        assert_eq!(event.partition_key, f.original.partition_key);
        assert!(
            matches!(event.reply, ReplyAddress::Discord { message_id, .. } if message_id == "800")
        );

        let report = f.setup.sweep(f.setup.now()).await;
        assert_eq!(report.pending, 0);
        assert_eq!(report.invalid, 0);
        let plan = f
            .setup
            .plans
            .replay_native_interaction(&restored.request, &restored.source_bytes)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(plan.source_body, first.source_bytes);
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(
            plan.targets[0].launch.consumer,
            f.original.consumer.clone().unwrap()
        );
        assert_eq!(
            plan.targets[0].launch.provider,
            first
                .request
                .channel
                .consumer(f.original.consumer.as_ref())
                .unwrap()
                .provider
                .clone()
        );
        assert_eq!(
            plan.targets[0].launch.event.conversation_id,
            f.original.conversation_id
        );
        assert_eq!(
            plan.targets[0].launch.event.partition_key,
            f.original.partition_key
        );
        assert_eq!(f.setup.sweep(f.setup.now()).await.pending, 0);
        let replay = f
            .setup
            .plans
            .replay_native_interaction(&first.request, &first.source_bytes)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay.admission_id, plan.admission_id);
        assert_eq!(replay.request_hash, plan.request_hash);
        let admitted = f
            .activity
            .latest(&f.setup.channel.owner.subject, &f.setup.fixture.record)
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|item| item.event_id == first.request.activity.event_id.as_str())
            .unwrap();
        assert_eq!(admitted.received_at, receipt.received_at);
        assert_eq!(admitted.consumers.len(), 1);
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
        assert_eq!(f.setup.direct_calls(), 0);
    }
}

#[tokio::test]
async fn native_component_restore_refuses_retired_selected_consumer_owner_and_source() {
    for change in 0..6 {
        let mut f = named_pair().await;
        let id = persisted(&f).await;
        let first = load(&f, &id).await;
        let mut value = f.setup.channel.to_blob_json();
        match change {
            0 => {
                value["routing"]["consumers"]
                    .as_array_mut()
                    .unwrap()
                    .remove(0);
                value["secrets"]["consumer_keys"]
                    .as_array_mut()
                    .unwrap()
                    .remove(0);
            }
            1 => value["routing"]["consumers"][0]["provider"]["binding_id"] = "bnd_replaced".into(),
            2 => value["routing"]["consumers"][0]["pending"] = true.into(),
            3 => value["owner"]["subject"] = "retired-owner".into(),
            4 => value["secrets"]["discord_bot_token"] = "rotated-source-fixture".into(),
            _ => {
                value.as_object_mut().unwrap().remove("routing");
                value["secrets"]["consumer_keys"] = json!([]);
            }
        }
        replace(&mut f, value).await;
        assert!(matches!(
            reader(&f).load(&id, f.channel.clone(), f.setup.now()).await,
            Err(GatewayError::Authority)
        ));
        let report = f.setup.sweep(f.setup.now()).await;
        assert_eq!(report.pending, 1);
        assert!(f
            .setup
            .plans
            .replay_native_interaction(&first.request, &first.source_bytes)
            .await
            .unwrap()
            .is_none());
        assert!(f.provider.memory.lock().unwrap().seen.is_empty());
        assert_eq!(f.rest.memory.calls.lock().unwrap().len(), 1);
        assert_eq!(f.setup.direct_calls(), 0);
    }
}

#[tokio::test]
async fn native_component_restore_requires_valid_current_credentials_and_schema_versions() {
    for change in 0..5 {
        let f = named_pair().await;
        let id = persisted(&f).await;
        let first = load(&f, &id).await;
        let mut value = f.setup.channel.to_blob_json();
        match change {
            0 => {
                value["secrets"]["consumer_keys"]
                    .as_array_mut()
                    .unwrap()
                    .remove(0);
            }
            1 => value["secrets"]["consumer_keys"][0][1] = "unmatched-selected-fixture".into(),
            2 => value["partition"]["version"] = 2.into(),
            3 => value["routing"]["version"] = 2.into(),
            _ => value["schema_version"] = 2.into(),
        }
        let definition = serde_json::from_value(value).unwrap();
        f.setup.fixture.replace(&definition);
        let mut unchecked = (*f.channel).clone();
        unchecked.definition = definition;
        assert!(matches!(
            reader(&f)
                .load(&id, std::sync::Arc::new(unchecked), f.setup.now())
                .await,
            Err(GatewayError::Authority)
        ));
        assert!(matches!(
            f.setup
                .state
                .resolver
                .resolve_current(f.channel.id.as_str())
                .await,
            Err(ResolveError::InvalidDefinition)
        ));
        assert_eq!(f.setup.sweep(f.setup.now()).await.pending, 1);
        assert!(f
            .setup
            .plans
            .replay_native_interaction(&first.request, &first.source_bytes)
            .await
            .unwrap()
            .is_none());
        assert!(f.provider.memory.lock().unwrap().seen.is_empty());
        assert_eq!(f.setup.direct_calls(), 0);
    }
}
