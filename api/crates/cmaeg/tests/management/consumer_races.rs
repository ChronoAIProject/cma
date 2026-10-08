//! Pending-bind completion under competing record writers.
use super::{consumer_support::*, *};
use cmaeg::channels::definition::hash_secret;
use cmaeg_core::channel::Secret;
use serde_json::Value;
use std::sync::atomic::Ordering;

fn spawn_add(
    gateway: &Gateway,
    id: &cmaeg_core::ids::ChannelId,
) -> tokio::task::JoinHandle<reqwest::Response> {
    let client = gateway.client.clone();
    let url = format!("{}/v1/channels/{id}/consumers", gateway.base);
    tokio::spawn(async move {
        client
            .post(url)
            .bearer_auth("token-alice")
            .json(&body(1, "cma_codex", "reviewer"))
            .send()
            .await
            .unwrap()
    })
}

#[tokio::test]
async fn ready_cas_retry_preserves_unrelated_concurrent_edits() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    let (captured, release) = gateway.provider.pause_next_bind();
    let request = spawn_add(&gateway, &id);
    captured.await.unwrap();
    let (pending, version) = gateway.stored(&record);
    assert_eq!(version, 2);
    assert!(pending.routing.as_ref().unwrap().consumers[0].pending);
    gateway
        .replace(&record, |definition| {
            definition.name = "changed concurrently".into();
            let routing = definition.routing.as_mut().unwrap();
            routing.max_targets = 2;
            routing.consumers[0].label = Some("New label".into());
            routing.consumers[0].mentionable = false;
        })
        .await;
    release.send(()).unwrap();
    let response = request.await.unwrap();
    assert_eq!(response.status(), 201);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["consumer"]["label"], "New label");
    let (stored, version) = gateway.stored(&record);
    assert_eq!(version, 4);
    assert_eq!(stored.name, "changed concurrently");
    assert_eq!(stored.routing.as_ref().unwrap().max_targets, 2);
    let consumer = &stored.routing.as_ref().unwrap().consumers[0];
    assert!(!consumer.pending);
    assert!(!consumer.mentionable);
    assert_eq!(gateway.provider.requests().len(), 1);
    assert_eq!(*gateway.index.changed.lock().unwrap(), [2, 4]);
    gateway.close().await;
}

#[tokio::test]
async fn a_removed_consumer_or_confirmed_deleted_record_compensates_the_new_binding() {
    for whole_record in [false, true] {
        let gateway = Gateway::start(true).await;
        let (id, record, _) = gateway.seed(|_| {});
        let (captured, release) = gateway.provider.pause_next_bind();
        let request = spawn_add(&gateway, &id);
        captured.await.unwrap();
        let (pending, version) = gateway.stored(&record);
        let binding = pending.routing.as_ref().unwrap().consumers[0]
            .provider
            .binding_id
            .clone();
        if whole_record {
            gateway
                .state
                .records
                .delete(&record, version)
                .await
                .unwrap();
        } else {
            gateway
                .replace(&record, |definition| {
                    definition.routing.as_mut().unwrap().consumers.clear();
                    definition.secrets.consumer_keys.clear();
                })
                .await;
        }
        release.send(()).unwrap();
        assert_eq!(request.await.unwrap().status(), 409);
        let calls = gateway.provider.requests();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].method, "DELETE");
        assert!(calls[1].path.ends_with(binding.as_str()));
        assert_eq!(
            calls[1].authorization.as_deref(),
            Some("Bearer fixture-key-reviewer")
        );
        assert_eq!(*gateway.index.changed.lock().unwrap(), [2]);
        gateway.close().await;
    }
}

#[tokio::test]
async fn changed_binding_identity_is_neither_overwritten_nor_unbound() {
    for change in ["profile", "credential", "binding", "owner"] {
        let gateway = Gateway::start(true).await;
        let (id, record, _) = gateway.seed(|_| {});
        let (captured, release) = gateway.provider.pause_next_bind();
        let request = spawn_add(&gateway, &id);
        captured.await.unwrap();
        gateway
            .replace(&record, |definition| {
                let consumer = &mut definition.routing.as_mut().unwrap().consumers[0];
                match change {
                    "profile" => {
                        consumer.profile.context.system_prompt = "concurrent profile".into();
                        consumer.profile_digest = consumer.profile.digest();
                    }
                    "credential" => {
                        let key = Secret::new("concurrent-credential");
                        consumer.agent_key_hash = hash_secret(key.as_str());
                        definition.secrets.consumer_keys[0].1 = key;
                    }
                    "binding" => {
                        consumer.provider.binding_id = "other-binding".parse().unwrap();
                    }
                    "owner" => {
                        definition.owner.subject = "mallory".into();
                    }
                    _ => unreachable!(),
                }
            })
            .await;
        let before = gateway.cma.records()[record.as_str()].blob.clone();
        release.send(()).unwrap();
        assert_eq!(request.await.unwrap().status(), 409, "{change}");
        assert_eq!(
            gateway.provider.requests().len(),
            1,
            "{change} must retain the binding"
        );
        assert_eq!(gateway.cma.records()[record.as_str()].blob, before);
        gateway.close().await;
    }
}

#[tokio::test]
async fn unreadable_record_is_not_proof_of_deletion_for_compensation() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    let (captured, release) = gateway.provider.pause_next_bind();
    let request = spawn_add(&gateway, &id);
    captured.await.unwrap();
    gateway
        .state
        .records
        .put(&record, "alice", b"unreadable retained blob", Some(2))
        .await
        .unwrap();
    release.send(()).unwrap();
    assert_eq!(request.await.unwrap().status(), 500);
    assert_eq!(gateway.provider.requests().len(), 1);
    gateway.close().await;
}

#[tokio::test]
async fn a_second_ready_cas_conflict_keeps_the_pending_consumer_and_binding() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    let (captured, release) = gateway.provider.pause_next_bind();
    let request = spawn_add(&gateway, &id);
    captured.await.unwrap();
    gateway.cma.put_conflicts.store(2, Ordering::SeqCst);
    release.send(()).unwrap();
    assert_eq!(request.await.unwrap().status(), 409);
    assert!(gateway.stored(&record).0.routing.unwrap().consumers[0].pending);
    assert_eq!(gateway.provider.requests().len(), 1);
    assert_eq!(*gateway.index.changed.lock().unwrap(), [2]);
    gateway.close().await;
}

#[tokio::test]
async fn a_rebind_cas_conflict_never_unbinds_the_retained_consumer() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    let response = added(&gateway, &id, 1, "reviewer").await;
    let consumer = response["consumer"]["id"].as_str().unwrap();
    let before = gateway.cma.records()[record.as_str()].blob.clone();
    gateway.cma.put_conflicts.store(1, Ordering::SeqCst);
    let request = json!({"expected_version":3,"name":"reviewer","credentials":{"agent_key":"rotated-fixture-key"}});
    assert_eq!(gateway.update(&id, consumer, &request).await.status(), 409);
    assert!(gateway
        .provider
        .requests()
        .iter()
        .all(|call| call.method == "PUT"));
    assert_eq!(gateway.cma.records()[record.as_str()].blob, before);
    gateway.close().await;
}
