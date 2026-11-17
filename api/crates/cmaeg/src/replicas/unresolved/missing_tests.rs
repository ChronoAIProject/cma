use super::*;
use crate::{
    ingress::{
        routing_plan::{self, fixture, OrdinaryPlan, RoutingOptions},
        routing_store::Candidate as AdmissionCandidate,
    },
    replicas::{
        mongo_fixture::{Fixture, Node},
        partition_model::item_id,
        store::Write,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    ids::{ConversationId, EventId, IdempotencyKey},
    seal::{event_ref::activity_digest, EventRef},
};
use mongodb::bson::{doc, DateTime};
use std::collections::BTreeSet;

async fn admit(node: &Node, name: &str) -> AdmittedPlan {
    let keys = &node.partitions.store.keys;
    let now = node.partitions.store.clock().await.unwrap().millis / 1000;
    let mut request = fixture::request(
        |v| {
            v["routing"]["default_targets"] = serde_json::json!(["default"]);
        },
        fixture::message("missing item fixture"),
    );
    let mut event = request
        .event_ref
        .open(&fixture::keys(), fixture::NOW)
        .unwrap()
        .payload;
    request.activity.event_id = EventId::new(format!("missing-fixture-{name}")).unwrap();
    request.idempotency_key = IdempotencyKey::from_event(&request.activity.event_id);
    request.conversation_id =
        ConversationId::derive(keys, &request.channel.id, 1, &event.partition_key);
    event.event_id = request.activity.event_id.clone();
    event.activity_sha256 = Some(activity_digest(&request.activity));
    event.conversation_id = request.conversation_id.clone();
    event.expires_at = now + 900;
    request.event_ref = EventRef::seal(&event, keys).unwrap();
    let OrdinaryPlan::Routed(routed) = routing_plan::plan(
        &request,
        keys,
        now,
        RoutingOptions {
            enabled: true,
            attributes_signed: true,
            non_human_providers: &BTreeSet::new(),
        },
    )
    .unwrap() else {
        panic!("routed fixture required")
    };
    let candidate =
        AdmissionCandidate::prepare(&request, &routed, b"synthetic", keys, now).unwrap();
    node.plans().admit(&candidate).await.unwrap()
}
fn operator(node: &Node) -> Operator {
    Operator::new(node.partitions.clone(), node.plans(), node.journal())
}
async fn request(node: &Node, plan: &AdmittedPlan) -> Request {
    Request {
        binding: node.head_binding(plan, &plan.targets[0]).await,
        request_id: uuid::Uuid::new_v4().to_string(),
        reason: "retire expired synthetic work".into(),
        plan_digest: None,
    }
}
async fn bound(op: &Operator, mut req: Request) -> Request {
    req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
    req
}
async fn expire(node: &Node, plan: &mut AdmittedPlan) {
    let store = &node.partitions.store;
    let until = store.clock().await.unwrap().millis / 1000 - 1;
    let (mut root, version) = store
        .read::<serde_json::Value>(&plan.admission_id, true)
        .await
        .unwrap()
        .unwrap();
    root["admitted_at"] = (until - 7 * 86400).into();
    root["retained_until"] = until.into();
    store
        .write(
            &plan.admission_id,
            &plan.identity.owner,
            &root,
            Write {
                retained_until: Some(until),
                expected: Some(version),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let key = item_id(&plan.targets[0].id);
    let (mut item, version) = store.read::<Item>(&key, true).await.unwrap().unwrap();
    item.retained_until = until;
    store
        .write(
            &key,
            &plan.identity.owner,
            &item,
            Write {
                retained_until: Some(until),
                expected: Some(version),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    plan.retained_until = until;
}
async fn assert_unchanged(node: &Node, req: &Request) {
    let (head, version) = node
        .partitions
        .store
        .read::<Head>(&req.binding.partition, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(version, req.binding.head_version);
    assert_eq!(head.queue.front(), Some(&req.binding.head_work));
    assert!(operator(node).audit(&req.request_id).await.is_err());
}
async fn replace_item(
    node: &Node,
    plan: &AdmittedPlan,
    mutate: impl FnOnce(&mut serde_json::Value),
) {
    let store = &node.partitions.store;
    let key = item_id(&plan.targets[0].id);
    let row = store.raw(&key, true).await.unwrap().unwrap();
    let (mut value, _) = store.decode::<serde_json::Value>(&key, true, row).unwrap();
    mutate(&mut value);
    let prepared = store
        .prepare(
            &key,
            &plan.identity.owner,
            &value,
            Write {
                retained_until: Some(plan.retained_until),
                ..Default::default()
            },
        )
        .unwrap();
    store.collection(true).unwrap().update_one(doc! {"_id": key}, doc! {"$set": {
        "blob": mongodb::bson::Binary { subtype: mongodb::bson::spec::BinarySubtype::Generic, bytes: prepared.blob }
    }}).await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn ttl_deleted_head_retires_atomically_replays_and_next_work_proceeds() {
    let db = Fixture::new().await;
    let node = db.node(1).await;
    let mut old = admit(&node, "old").await;
    let next = admit(&node, "next").await;
    expire(&node, &mut old).await;
    let store = &node.partitions.store;
    // Observe deletion by Mongo's TTL monitor, never delete the item in this test.
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        while store
            .raw(&item_id(&old.targets[0].id), true)
            .await
            .unwrap()
            .is_some()
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let op = operator(&node);
    let req = bound(&op, request(&node, &old).await).await;
    let preview = op.preview(&req).await.unwrap();
    assert_eq!(preview.item_classification, Some("ttl_deleted"));
    assert_eq!(preview.queue_depth, 2);
    assert_eq!(preview.queue_items.len(), 1);
    assert_eq!(preview.queue_items[0].item, "present");
    assert_eq!(
        op.inspect().await.unwrap().candidates[0].state,
        "missing_item_expired"
    );
    let receipt = op.apply(&req, 1).await.unwrap();
    assert_eq!(receipt.disposition, "missing_item_retired");
    assert_eq!(receipt.prior_state, "missing_item_expired");
    assert_eq!(receipt.target, req.binding.head_work);
    assert_eq!(receipt.owner, old.identity.owner);
    assert_eq!(receipt.rollback_safe_after, receipt.timestamp);
    assert_eq!(
        receipt.retained_until,
        time::OffsetDateTime::parse(
            &receipt.timestamp,
            &time::format_description::well_known::Rfc3339
        )
        .unwrap()
        .unix_timestamp()
            + 30 * 86400
    );
    let (head, version) = store
        .read::<Head>(&req.binding.partition, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.queue.front(), Some(&next.targets[0].id));
    assert_eq!(version, req.binding.head_version + 1);
    assert!(head.authority.is_none());
    assert!(store
        .raw(&item_id(&old.targets[0].id), true)
        .await
        .unwrap()
        .is_none());
    let (chunk, _) = store
        .read::<serde_json::Value>(&receipt.evidence[0], true)
        .await
        .unwrap()
        .unwrap();
    let snapshot: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(chunk["base64"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(hash(&snapshot).unwrap(), receipt.evidence_sha256);
    assert_eq!(snapshot["item"], "ttl_deleted");
    assert_eq!(snapshot["head"]["queue"][0], old.targets[0].id);
    let (index, index_version) = node.operator_index().await;
    let saved = op.apply(&req, 1).await.unwrap();
    assert_eq!(hash(&receipt).unwrap(), hash(&saved).unwrap());
    assert_eq!(node.operator_index().await.1, index_version);
    assert_eq!(
        store
            .read::<Head>(&req.binding.partition, false)
            .await
            .unwrap()
            .unwrap()
            .1,
        version
    );
    assert_eq!(
        hash(&op.audit(&req.request_id).await.unwrap()).unwrap(),
        hash(&receipt).unwrap()
    );
    // These deny_unknown_fields types and active_index are unchanged from before
    // missing-item retirement. No new receipt/index field is necessary.
    let _: Receipt = serde_json::from_value(serde_json::to_value(&receipt).unwrap()).unwrap();
    let _: Index = serde_json::from_value(serde_json::to_value(index).unwrap()).unwrap();
    assert!(op.active_index().await.unwrap().2.is_empty());
    assert!(op.inspect().await.unwrap().abandonments.is_empty());
    let mut reused = req.clone();
    reused.reason.push_str(" changed");
    assert_eq!(
        op.apply(&reused, 1).await.err().unwrap().code(),
        "request_id_reuse"
    );
    assert_eq!(node.partitions.enqueue(&old).await, Err(Error::Expired));
    let recovered = node.plans().recover_all_page(None, 32).await.unwrap();
    assert!(recovered
        .plans
        .iter()
        .all(|p| p.admission_id != old.admission_id));
    assert_eq!(
        store
            .read::<Head>(&req.binding.partition, false)
            .await
            .unwrap()
            .unwrap()
            .1,
        version
    );
    let lease = node
        .partitions
        .claim(&next, &next.targets[0])
        .await
        .unwrap();
    node.partitions.complete(&lease).await.unwrap();
    let row = store
        .raw(&req.binding.partition, false)
        .await
        .unwrap()
        .unwrap();
    assert!(!row
        .get_document("replica")
        .unwrap()
        .get_bool("active")
        .unwrap());
    drop((op, node));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn expired_retained_item_requires_matching_evidence_and_expired_plan() {
    for fault in [
        "none",
        "kind",
        "partition",
        "work",
        "undecodable",
        "live_plan",
        "absent_plan",
    ] {
        let db = Fixture::new().await;
        let node = db.node(1).await;
        db.hold_expired_interactions().await;
        let mut plan = admit(&node, fault).await;
        expire(&node, &mut plan).await;
        let store = &node.partitions.store;
        match fault {
            "kind" | "partition" => {
                replace_item(&node, &plan, |v| v[fault] = "synthetic-mismatch".into()).await
            }
            "work" => {
                replace_item(&node, &plan, |v| {
                    v["work_id"] = uuid::Uuid::new_v4().to_string().into()
                })
                .await
            }
            "undecodable" => {
                replace_item(&node, &plan, |v| {
                    v.as_object_mut().unwrap().remove("kind");
                })
                .await
            }
            "live_plan" => {
                store.collection(true).unwrap().update_one(doc! {"_id": &plan.admission_id}, doc! {"$set": {"expires_at": DateTime::from_millis(store.clock().await.unwrap().millis + 60_000)}}).await.unwrap();
            }
            "absent_plan" => {
                store
                    .collection(true)
                    .unwrap()
                    .delete_one(doc! {"_id": &plan.admission_id})
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let op = operator(&node);
        let req = request(&node, &plan).await;
        if matches!(fault, "none" | "absent_plan") {
            let req = bound(&op, req).await;
            assert_eq!(
                op.preview(&req).await.unwrap().item_classification,
                Some("expired")
            );
            op.apply(&req, 1).await.unwrap();
            let (head, _) = store
                .read::<Head>(&req.binding.partition, false)
                .await
                .unwrap()
                .unwrap();
            assert!(head.queue.is_empty());
            if fault == "none" {
                // Expose an expired manifest despite delayed physical TTL expiry:
                // recover_head must refuse its logical retention before enqueue.
                store.collection(true).unwrap().update_one(
                    doc! {"_id": &plan.admission_id},
                    doc! {"$set": {"expires_at": DateTime::from_millis(store.clock().await.unwrap().millis + 60_000)}},
                ).await.unwrap();
                let page = node.plans().recover_legacy_page(None, 32).await.unwrap();
                assert!(page.plans.is_empty());
                assert!(page
                    .issues
                    .iter()
                    .any(|issue| issue.record_id == plan.admission_id
                        && issue.error == crate::ingress::routing_store::AdmissionError::Expired));
                assert_eq!(node.partitions.enqueue(&plan).await, Err(Error::Expired));
                assert!(store
                    .read::<Head>(&req.binding.partition, false)
                    .await
                    .unwrap()
                    .unwrap()
                    .0
                    .queue
                    .is_empty());
            }
            let row = store
                .raw(&item_id(&plan.targets[0].id), true)
                .await
                .unwrap()
                .unwrap();
            assert!(
                !store
                    .decode::<Item>(&item_id(&plan.targets[0].id), true, row)
                    .unwrap()
                    .0
                    .done
            );
        } else {
            let expected = if fault == "live_plan" {
                "missing_item_plan_live"
            } else {
                "missing_item_mismatch"
            };
            assert_eq!(op.preview(&req).await.err().unwrap().code(), expected);
            assert_eq!(op.apply(&req, 1).await.err().unwrap().code(), expected);
            assert_unchanged(&node, &req).await;
            assert_eq!(op.inspect().await.unwrap().unreadable.len(), 1);
        }
        drop((op, node));
        db.cleanup().await;
    }
}
