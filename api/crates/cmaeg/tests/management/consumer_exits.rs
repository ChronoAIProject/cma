//! Owner exits, unbind ordering, master-key rotation and reference revocation.
use super::{consumer_support::*, *};
use cmaeg::channels::{management, ChannelTarget};
use cmaeg_core::{channel::Secret, ids::ConsumerId, seal::open_blob};
use serde_json::Value;
use std::collections::BTreeMap;

const REVIEWER: &str = "csm_0123456789ab";
fn seed_consumer(value: &mut Value, pending: bool) {
    let request = body(1, "cma_codex", "reviewer");
    let profile: cmaeg_core::profile::EventWorkerProfile =
        serde_json::from_value(request["profile"].clone()).unwrap();
    value["routing"]["consumers"] = json!([{"id":REVIEWER,"name":"reviewer","profile":profile,"profile_digest":profile.digest(),
        "provider":{"slug":"cma_codex","binding_id":"consumer-binding","level":"l1"},
        "agent_key_hash":cmaeg::channels::definition::hash_secret("fixture-key-reviewer"),"pending":pending,"created_at":"2026-10-01T00:00:00Z"}]);
    value["secrets"]["consumer_keys"] = json!([[REVIEWER, "fixture-key-reviewer"]]);
}

#[tokio::test]
async fn ready_and_pending_consumers_can_be_removed_with_the_switch_off() {
    for pending in [false, true] {
        let gateway = Gateway::configured(
            false,
            FakeProviderConfig {
                unbind_status: BTreeMap::from([("consumer-binding".into(), 404)]),
                ..Default::default()
            },
        )
        .await;
        let (id, record, _) = gateway.seed(|value| seed_consumer(value, pending));
        gateway.state.resolver.resolve(id.as_str()).await.unwrap();
        assert_eq!(gateway.remove(&id, REVIEWER, 1).await.status(), 200);
        let (stored, version) = gateway.stored(&record);
        assert_eq!(version, 2);
        assert!(stored.routing.unwrap().consumers.is_empty());
        assert!(stored.secrets.consumer_keys.is_empty());
        assert_eq!(gateway.provider.requests().len(), 1);
        assert_eq!(gateway.provider.requests()[0].method, "DELETE");
        let resolved = gateway.state.resolver.resolve(id.as_str()).await.unwrap();
        assert!(resolved
            .consumer(Some(&ConsumerId::parse(REVIEWER).unwrap()))
            .is_none());
        assert_eq!(*gateway.index.changed.lock().unwrap(), [2]);
        assert!(gateway.index.removed.lock().unwrap().is_empty());
        gateway.close().await;
    }
}

#[tokio::test]
async fn removal_reports_each_reference_including_inert_rules_and_disabled_publish() {
    for (field, pointer) in [
        ("default", "/routing/default_targets/0"),
        ("rule", "/routing/rules/0/targets/0"),
        (
            "default_publish",
            "/routing/default_consumer/publish/targets/0",
        ),
        ("other_publish", "/routing/consumers/1/publish/targets/0"),
    ] {
        let gateway = Gateway::start(true).await;
        let (id, record, _) = gateway.seed(|_| {});
        let first = added(&gateway, &id, 1, "reviewer").await;
        let consumer = ConsumerId::parse(first["consumer"]["id"].as_str().unwrap()).unwrap();
        added(&gateway, &id, 3, "helper").await;
        let version = gateway.replace(&record,|definition| {
            let routing = definition.routing.as_mut().unwrap();
            match field {
                "default" => routing.default_targets = vec![consumer.clone()],
                "rule" => routing.rules.push(serde_json::from_value(json!({"id":"inert","when":{"sources":["custom"]},"targets":[consumer]})).unwrap()),
                "default_publish" => routing.default_consumer.publish.targets = vec![consumer.clone()],
                "other_publish" => { routing.consumers[1].publish.enabled = false; routing.consumers[1].publish.targets = vec![consumer.clone()]; }
                _ => unreachable!(),
            }
        }).await;
        let calls = gateway.calls();
        assert_eq!(
            detail(gateway.remove(&id, consumer.as_str(), version).await, 409).await,
            format!("{pointer}: consumer_in_use")
        );
        assert_eq!(gateway.calls(), calls);
        assert_eq!(gateway.stored(&record).1, version);
        gateway.close().await;
    }
}

#[tokio::test]
async fn default_removal_and_unbind_failure_preserve_the_record() {
    let gateway = Gateway::configured(
        true,
        FakeProviderConfig {
            unbind_status: BTreeMap::from([("consumer-binding".into(), 500)]),
            ..Default::default()
        },
    )
    .await;
    let (id, record, _) = gateway.seed(|value| seed_consumer(value, false));
    assert_eq!(
        detail(gateway.remove(&id, "default", 1).await, 400).await,
        "default_consumer_required"
    );
    let before = gateway.cma.records()[record.as_str()].blob.clone();
    assert_eq!(gateway.remove(&id, REVIEWER, 1).await.status(), 503);
    assert_eq!(gateway.stored(&record).1, 1);
    assert_eq!(gateway.cma.records()[record.as_str()].blob, before);
    assert!(gateway.index.changed.lock().unwrap().is_empty());
    gateway.close().await;
}

#[tokio::test]
async fn channel_delete_unbinds_additional_consumers_before_default_and_removes_index() {
    let gateway = Gateway::start(true).await;
    let (id, record, original) = gateway.seed(|value| seed_consumer(value, true));
    let response = gateway.add(&id, &body(1, "nyxbot", "helper")).await;
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    let other = body["consumer"]["binding_id"].as_str().unwrap();
    management::delete(
        &gateway.state,
        &creator(),
        &ChannelTarget::Record(record.clone()),
        3,
    )
    .await
    .unwrap();
    let mut deletes: Vec<_> = gateway
        .provider
        .requests()
        .into_iter()
        .chain(gateway.other.requests())
        .filter(|call| call.method == "DELETE")
        .collect();
    deletes.sort_by_key(|call| call.received_at);
    assert_eq!(deletes.len(), 3);
    assert!(deletes[0].path.ends_with("consumer-binding"));
    assert!(deletes[1].path.ends_with(other));
    assert!(deletes[2].path.ends_with(
        original
            .consumer(None)
            .unwrap()
            .provider
            .binding_id
            .as_str()
    ));
    assert!(!gateway.cma.records().contains_key(record.as_str()));
    assert_eq!(*gateway.index.removed.lock().unwrap(), [record]);
    gateway.close().await;
}

#[tokio::test]
async fn channel_delete_failure_keeps_the_record_and_default_binding() {
    let gateway = Gateway::configured(
        true,
        FakeProviderConfig {
            unbind_status: BTreeMap::from([("consumer-binding".into(), 500)]),
            ..Default::default()
        },
    )
    .await;
    let (id, record, _) = gateway.seed(|value| seed_consumer(value, false));
    assert!(
        management::delete(&gateway.state, &creator(), &ChannelTarget::Id(id), 1)
            .await
            .is_err()
    );
    assert_eq!(gateway.stored(&record).1, 1);
    assert_eq!(gateway.provider.requests().len(), 1);
    assert!(gateway.provider.requests()[0]
        .path
        .ends_with("consumer-binding"));
    assert!(gateway.index.removed.lock().unwrap().is_empty());
    gateway.close().await;
}

#[tokio::test]
async fn rekey_retains_every_consumer_key_in_the_resealed_blob() {
    let gateway = Gateway::start(true).await;
    let (id, record, original) = gateway.seed(|value| seed_consumer(value, true));
    let mut state = gateway.state.clone();
    state.master_keys = Arc::new(key_ring(2));
    assert_eq!(
        management::rekey(&state, &ChannelTarget::Id(id))
            .await
            .unwrap(),
        2
    );
    let blob = gateway.cma.records()[record.as_str()].blob.clone();
    let opened = open_blob(&key_ring(2), &blob).unwrap();
    assert_eq!(opened.kid, 2);
    let definition: cmaeg_core::channel::ChannelDefinition =
        serde_json::from_slice(&opened.payload).unwrap();
    assert_eq!(
        definition.secrets.consumer_keys,
        original.secrets.consumer_keys
    );
    assert_eq!(
        definition.secrets.consumer_keys[0].1,
        Secret::new("fixture-key-reviewer")
    );
    gateway.close().await;
}
