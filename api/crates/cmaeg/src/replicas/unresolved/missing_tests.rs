use super::*;
use crate::{
    ingress::{
        routing_plan::{self, fixture, OrdinaryPlan, RoutingOptions},
        routing_store::Candidate as AdmissionCandidate,
    },
    replicas::{
        mongo_fixture::{Fixture, Node},
        partition_model::{item_id, Authority},
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

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn apply_rechecks_item_lease_binding_and_digest() {
    for fault in [
        "live_item",
        "mismatch",
        "live_lease",
        "version",
        "front",
        "digest",
        "ephemeral_head",
    ] {
        let db = Fixture::new().await;
        let node = db.node(1).await;
        db.hold_expired_interactions().await;
        let mut plan = admit(&node, fault).await;
        expire(&node, &mut plan).await;
        let op = operator(&node);
        let mut req = bound(&op, request(&node, &plan).await).await;
        let store = &node.partitions.store;
        match fault {
            "live_item" => {
                store.collection(true).unwrap().update_one(doc! {"_id": item_id(&req.binding.head_work)}, doc! {"$set": {"expires_at": DateTime::from_millis(store.clock().await.unwrap().millis + 60_000)}}).await.unwrap();
            }
            "mismatch" => {
                replace_item(&node, &plan, |v| v["kind"] = "synthetic-mismatch".into()).await
            }
            "live_lease" => {
                let (mut head, _) = store
                    .read::<Head>(&req.binding.partition, false)
                    .await
                    .unwrap()
                    .unwrap();
                head.epoch = 1;
                head.authority = Some(Authority {
                    instance: "a".repeat(64),
                    claim: uuid::Uuid::new_v4().to_string(),
                    epoch: 1,
                    until_ms: store.clock().await.unwrap().millis + 60_000,
                });
                req.binding.head_version = store
                    .write(
                        &head.id,
                        &head.owner,
                        &head,
                        Write {
                            expected: Some(req.binding.head_version),
                            projection: Some(node.partitions.projection(true)),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                let view = op.inspect().await.unwrap();
                assert_eq!(view.skipped, 1);
                assert!(view.unreadable.is_empty());
            }
            "version" => req.binding.head_version += 1,
            "front" => req.binding.head_work = uuid::Uuid::new_v4().to_string(),
            "digest" => req.plan_digest = Some("synthetic-wrong-digest".into()),
            "ephemeral_head" => {
                let (head, _) = store
                    .read::<Head>(&req.binding.partition, false)
                    .await
                    .unwrap()
                    .unwrap();
                store
                    .write(
                        &head.id,
                        &head.owner,
                        &head,
                        Write {
                            retained_until: Some(store.clock().await.unwrap().millis / 1000 + 60),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                store
                    .collection(false)
                    .unwrap()
                    .delete_one(doc! {"_id": &head.id})
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let code = op.apply(&req, 1).await.err().unwrap().code();
        assert_eq!(
            code,
            match fault {
                "live_item" => "invalid_operator_evidence",
                "mismatch" => "missing_item_mismatch",
                "live_lease" => "live_lease",
                "version" | "front" => "binding_version_mismatch",
                "digest" => "plan_digest_mismatch",
                "ephemeral_head" => "binding_version_mismatch",
                _ => unreachable!(),
            },
            "{fault}"
        );
        if fault != "ephemeral_head" {
            let actual = request(&node, &plan).await;
            assert_unchanged(&node, &actual).await;
        } else {
            assert!(store
                .raw(&req.binding.partition, false)
                .await
                .unwrap()
                .is_none());
            assert!(store
                .raw(&req.binding.partition, true)
                .await
                .unwrap()
                .is_some());
        }
        assert!(op.audit(&req.request_id).await.is_err());
        drop((op, node));
        db.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn concurrent_head_write_rolls_back_receipt_evidence_and_index() {
    let db = Fixture::new().await;
    let node = db.node(1).await;
    db.hold_expired_interactions().await;
    let mut plan = admit(&node, "race").await;
    expire(&node, &mut plan).await;
    let op = operator(&node);
    let req = bound(&op, request(&node, &plan).await).await;
    // Capture the same selection apply uses, then interleave a valid peer append
    // before the transaction. This exercises the database CAS, not select's check.
    let Selected::Missing(selected) = op.select(&req.binding, true).await.unwrap() else {
        panic!("missing selection")
    };
    let snapshot_hash = hash(&selected.snapshot()).unwrap();
    let next = admit(&node, "racing-next").await;
    assert_eq!(
        op.apply_missing(&req, 1, *selected)
            .await
            .err()
            .unwrap()
            .code(),
        "binding_version_mismatch"
    );
    let store = &node.partitions.store;
    let (head, version) = store
        .read::<Head>(&req.binding.partition, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(version, req.binding.head_version + 1);
    assert_eq!(head.queue.len(), 2);
    assert_eq!(head.queue.front(), Some(&plan.targets[0].id));
    assert_eq!(head.queue.back(), Some(&next.targets[0].id));
    assert!(op.audit(&req.request_id).await.is_err());
    assert!(store
        .raw(
            &identifier(
                "replica-abandonment-index-v1",
                &node.partitions.membership.deployment
            ),
            false
        )
        .await
        .unwrap()
        .is_none());
    let audit_id = identifier("replica-abandonment-v1", &req.request_id);
    let count = store
        .collection(true)
        .unwrap()
        .count_documents(doc! {"_id": &audit_id})
        .await
        .unwrap();
    assert_eq!(count, 0);
    let evidence_id = identifier(
        "replica-abandonment-evidence-v1",
        &format!("{audit_id}:{snapshot_hash}:0"),
    );
    assert!(store.raw(&evidence_id, true).await.unwrap().is_none());
    drop((op, node));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn preview_bounds_queue_evidence_and_expired_authority_can_retire() {
    let db = Fixture::new().await;
    let node = db.node(1).await;
    db.hold_expired_interactions().await;
    let mut plan = admit(&node, "preview-front").await;
    expire(&node, &mut plan).await;
    let mut expired = admit(&node, "preview-expired").await;
    expire(&node, &mut expired).await;
    let live = admit(&node, "preview-present").await;
    let mut req = request(&node, &plan).await;
    let store = &node.partitions.store;
    let (mut head, version) = store
        .read::<Head>(&req.binding.partition, false)
        .await
        .unwrap()
        .unwrap();
    for _ in 0..33 {
        head.queue.push_back(uuid::Uuid::new_v4().to_string());
    }
    head.epoch = 1;
    head.authority = Some(Authority {
        instance: "a".repeat(64),
        claim: uuid::Uuid::new_v4().to_string(),
        epoch: 1,
        until_ms: store.clock().await.unwrap().millis - 1,
    });
    req.binding.head_version = store
        .write(
            &head.id,
            &head.owner,
            &head,
            Write {
                expected: Some(version),
                projection: Some(node.partitions.projection(true)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let op = operator(&node);
    let req = bound(&op, req).await;
    let preview = op.preview(&req).await.unwrap();
    assert_eq!(preview.queue_depth, 36);
    assert_eq!(preview.queue_items.len(), 32);
    assert_eq!(preview.queue_items[0].item, "expired");
    assert_eq!(preview.queue_items[1].work, live.targets[0].id);
    assert_eq!(preview.queue_items[1].item, "present");
    assert!(preview.queue_items[2..].iter().all(|v| v.item == "absent"));
    op.apply(&req, 1).await.unwrap();
    let (head, _) = store
        .read::<Head>(&req.binding.partition, false)
        .await
        .unwrap()
        .unwrap();
    assert!(head.authority.is_none());
    assert_eq!(head.queue.len(), 35);
    drop((op, node));
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn missing_retirement_uses_existing_operator_http_and_receipt_readback() {
    use crate::replicas::operator_http::{self, Command};
    let db = Fixture::new().await;
    let node = db.node(1).await;
    db.hold_expired_interactions().await;
    let mut plan = admit(&node, "http").await;
    expire(&node, &mut plan).await;
    let mut req = request(&node, &plan).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = operator_http::router(
        operator(&node),
        db.keys(),
        "https://replica-fixture.invalid",
        port,
    );
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let send = |command: Command| {
        let bytes = serde_json::to_vec(&command).unwrap();
        let ticket = operator_http::ticket(
            &db.keys(),
            "https://replica-fixture.invalid",
            port,
            &bytes,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .unwrap();
        client
            .post(format!("http://127.0.0.1:{port}{}", operator_http::PATH))
            .bearer_auth(ticket)
            .body(bytes)
            .send()
    };
    let dry = send(Command {
        action: "dry_run".into(),
        request: Some(req.clone()),
        audit: None,
    })
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json::<serde_json::Value>()
    .await
    .unwrap();
    assert_eq!(dry["candidate"]["state"], "missing_item_expired");
    assert_eq!(dry["queue_depth"], 1);
    req.plan_digest = Some(dry["plan_digest"].as_str().unwrap().into());
    let applied = send(Command {
        action: "apply".into(),
        request: Some(req.clone()),
        audit: None,
    })
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json::<Receipt>()
    .await
    .unwrap();
    assert_eq!(applied.disposition, "missing_item_retired");
    let saved = send(Command {
        action: "audit".into(),
        request: None,
        audit: Some(req.request_id),
    })
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json::<Receipt>()
    .await
    .unwrap();
    assert_eq!(hash(&applied).unwrap(), hash(&saved).unwrap());
    server.abort();
    let _ = server.await;
    drop(node);
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn live_head_scan_skips_exact_record_reads() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    let db = Fixture::new().await;
    let armed = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let (enabled, observed) = (armed.clone(), reads.clone());
    let node = db
        .node_with_handler(
            1,
            Some(EventHandler::callback(move |event| {
                if let CommandEvent::Started(event) = event {
                    if enabled.load(Ordering::SeqCst)
                        && event.command_name == "find"
                        && event
                            .command
                            .get_document("filter")
                            .ok()
                            .is_some_and(|filter| filter.get_str("_id").is_ok())
                    {
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })),
        )
        .await;
    let plan = admit(&node, "live-scan").await;
    let lease = node
        .partitions
        .claim(&plan, &plan.targets[0])
        .await
        .unwrap();
    armed.store(true, Ordering::SeqCst);
    let scan = operator(&node).scan_heads(4096).await.unwrap();
    armed.store(false, Ordering::SeqCst);
    assert_eq!(scan.skipped, 1);
    assert!(scan.candidates.is_empty());
    assert!(scan.unreadable.is_empty());
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    node.partitions.release(&lease).await.unwrap();
    drop(node);
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn normal_selection_reads_item_once_and_preserves_head_failures() {
    use crate::replicas::diagnostics::{Class, Stage};
    use mongodb::event::{command::CommandEvent, EventHandler};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    let db = Fixture::new().await;
    let key = Arc::new(Mutex::new(None::<String>));
    let reads = Arc::new(AtomicUsize::new(0));
    let (watched, observed) = (key.clone(), reads.clone());
    let node = db
        .node_with_handler(
            1,
            Some(EventHandler::callback(move |event| {
                if let CommandEvent::Started(event) = event {
                    let key = watched.lock().unwrap();
                    if key.is_some()
                        && event.command_name == "find"
                        && event
                            .command
                            .get_document("filter")
                            .ok()
                            .and_then(|filter| filter.get_str("_id").ok())
                            == key.as_deref()
                    {
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })),
        )
        .await;
    let plan = admit(&node, "normal-selection").await;
    let req = request(&node, &plan).await;
    let op = operator(&node);
    *key.lock().unwrap() = Some(item_id(&req.binding.head_work));
    let selected = op.select(&req.binding, false).await.unwrap();
    assert!(matches!(selected, Selected::Normal(_)));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    replace_item(&node, &plan, |v| v["kind"] = "synthetic-mismatch".into()).await;
    reads.store(0, Ordering::SeqCst);
    let error = op.select(&req.binding, false).await.err().unwrap();
    assert!(matches!(error, Failure::Head(error)
        if error.class == Class::Mismatch && error.stage == Stage::ItemCheck));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    drop((op, node));
    db.cleanup().await;
}

async fn seed_work_record(node: &Node, plan: &AdmittedPlan, retained_until: i64) {
    let state: crate::turns::routed::TargetState = serde_json::from_value(serde_json::json!({
        "kind": "ordinary_route_execution_v1", "id": plan.targets[0].id,
        "root": plan.admission_id, "owner": plan.identity.owner,
        "request_hash": plan.request_hash, "retained_until": retained_until,
        "event_expires_at": plan.targets[0].launch.event.expires_at,
        "phase": {"state": "pending"}, "body": null,
        "ownership": {"worker": "synthetic-worker", "claim": uuid::Uuid::new_v4().to_string(),
            "epoch": 1, "until": 0}
    }))
    .unwrap();
    node.partitions
        .store
        .write(
            &state.id,
            &state.owner,
            &state,
            Write {
                retained_until: Some(retained_until),
                ..Default::default()
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn deleted_item_requires_absent_or_expired_work_record() {
    for work_state in ["live", "expired", "absent"] {
        let db = Fixture::new().await;
        let node = db.node(1).await;
        db.hold_expired_interactions().await;
        let mut plan = admit(&node, work_state).await;
        if work_state == "live" {
            seed_work_record(&node, &plan, plan.retained_until).await;
        }
        expire(&node, &mut plan).await;
        if work_state == "expired" {
            seed_work_record(&node, &plan, plan.retained_until).await;
        }
        let store = &node.partitions.store;
        store
            .collection(true)
            .unwrap()
            .delete_one(doc! {"_id": item_id(&plan.targets[0].id)})
            .await
            .unwrap();
        let op = operator(&node);
        let req = request(&node, &plan).await;
        if work_state == "live" {
            assert_eq!(
                op.preview(&req).await.err().unwrap().code(),
                "missing_item_plan_live"
            );
            assert_eq!(
                op.apply(&req, 1).await.err().unwrap().code(),
                "missing_item_plan_live"
            );
            let view = op.inspect().await.unwrap();
            assert!(view.candidates.is_empty());
            assert_eq!(view.unreadable.len(), 1);
            assert_unchanged(&node, &req).await;
        } else {
            let req = bound(&op, req).await;
            assert_eq!(
                op.inspect().await.unwrap().candidates[0].state,
                "missing_item_expired"
            );
            let receipt = op.apply(&req, 1).await.unwrap();
            let (chunk, _) = store
                .read::<serde_json::Value>(&receipt.evidence[0], true)
                .await
                .unwrap()
                .unwrap();
            let snapshot: serde_json::Value = serde_json::from_slice(
                &STANDARD.decode(chunk["base64"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(snapshot["item"], "ttl_deleted");
            assert_eq!(snapshot["work_record"], work_state);
            assert_eq!(hash(&snapshot).unwrap(), receipt.evidence_sha256);
        }
        drop((op, node));
        db.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn apply_rechecks_work_record_and_binds_its_classification() {
    for live in [true, false] {
        let db = Fixture::new().await;
        let node = db.node(1).await;
        db.hold_expired_interactions().await;
        let mut plan = admit(&node, "work-record-race").await;
        let live_retention = plan.retained_until;
        expire(&node, &mut plan).await;
        node.partitions
            .store
            .collection(true)
            .unwrap()
            .delete_one(doc! {"_id": item_id(&plan.targets[0].id)})
            .await
            .unwrap();
        let op = operator(&node);
        let req = bound(&op, request(&node, &plan).await).await;
        seed_work_record(
            &node,
            &plan,
            if live {
                live_retention
            } else {
                plan.retained_until
            },
        )
        .await;
        assert_eq!(
            op.apply(&req, 1).await.err().unwrap().code(),
            if live {
                "missing_item_plan_live"
            } else {
                "plan_digest_mismatch"
            }
        );
        assert_unchanged(&node, &req).await;
        if !live {
            let rebound = bound(&op, req).await;
            op.apply(&rebound, 1).await.unwrap();
        }
        drop((op, node));
        db.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn apply_time_deadline_keeps_missing_retirement_compatible_with_older_listing() {
    use std::time::Duration;
    let db = Fixture::new().await;
    let node = db.node(1).await;
    let mut plan = admit(&node, "rollback-listing").await;
    expire(&node, &mut plan).await;
    let store = &node.partitions.store;
    tokio::time::timeout(Duration::from_secs(90), async {
        while store
            .raw(&item_id(&plan.targets[0].id), true)
            .await
            .unwrap()
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let op = operator(&node);
    let req = bound(&op, request(&node, &plan).await).await;
    let receipt = op.apply(&req, 1).await.unwrap();
    assert_eq!(receipt.disposition, "missing_item_retired");
    let audit_id = identifier("replica-abandonment-v1", &req.request_id);
    let applied_at = time::OffsetDateTime::parse(
        &receipt.timestamp,
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
    .unix_timestamp();
    let (mut persisted, version) = node.operator_index().await;
    assert_eq!(persisted.audits, vec![audit_id.clone()]);
    assert_eq!(persisted.deadlines[&audit_id], applied_at);
    assert_eq!(receipt.rollback_safe_after, receipt.timestamp);

    for later in [false, true] {
        if later {
            // Advance real Mongo server time; Tokio's paused clock cannot move
            // the server clock used by the unchanged history reader.
            tokio::time::timeout(Duration::from_secs(10), async {
                while store.clock().await.unwrap().millis / 1000 < applied_at + 2 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .unwrap();
            assert!(store.clock().await.unwrap().millis / 1000 >= applied_at + 2);
        }
        let (active, observed_version, records) = op.active_index().await.unwrap();
        assert!(active.audits.is_empty());
        assert!(active.deadlines.is_empty());
        assert!(active.rollback_safe_after.is_none());
        assert!(records.is_empty());
        assert_eq!(observed_version, version);
        let listing = op.inspect().await.unwrap();
        assert!(listing.abandonments.is_empty());
        assert!(listing.rollback_safe_after.is_none());
        assert_eq!(listing.outstanding_quarantined_effects, 0);
        assert_eq!(
            hash(&op.audit(&req.request_id).await.unwrap()).unwrap(),
            hash(&receipt).unwrap()
        );
        // Listing must skip the stored entry on each read, not erase it first.
        let (unchanged, observed_version) = node.operator_index().await;
        assert_eq!(hash(&unchanged).unwrap(), hash(&persisted).unwrap());
        assert_eq!(observed_version, version);
    }

    // Only change the index deadline: forcing this same retained receipt into
    // the active window reaches the older reader's incompatible disposition.
    let future = store.clock().await.unwrap().millis / 1000 + 60;
    persisted.deadlines.insert(audit_id, future);
    persisted.rollback_safe_after = Some(utc(future).unwrap());
    store
        .write(
            &identifier("replica-abandonment-index-v1", &persisted.deployment),
            &persisted.deployment,
            &persisted,
            Write {
                expected: Some(version),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(future > store.clock().await.unwrap().millis / 1000);
    assert_eq!(op.active_index().await.err(), Some(Error::Corrupt));
    assert_eq!(op.inspect().await.err(), Some(Error::Corrupt));
    assert_eq!(
        hash(&op.audit(&req.request_id).await.unwrap()).unwrap(),
        hash(&receipt).unwrap()
    );
    drop((op, node));
    db.cleanup().await;
}
