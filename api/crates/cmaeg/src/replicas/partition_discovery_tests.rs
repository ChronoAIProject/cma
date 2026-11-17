use super::*;
use crate::replicas::partition_model::{item_id, Item};
use crate::replicas::{mongo_fixture::Fixture, partition_tests};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn cached_frontier_after_atomic_completion_is_not_corruption() {
    let database = Fixture::new().await;
    let node = database.node(1).await;
    let (_server, mut plan) = partition_tests::plan().await;
    plan.targets.truncate(1);
    plan.retained_until = time::OffsetDateTime::now_utc().unix_timestamp() + 3600;
    let target = &plan.targets[0];
    node.partitions.enqueue(&plan).await.unwrap();
    let heads = node.partitions.active_heads().await.unwrap();
    let lease = node.partitions.claim(&plan, target).await.unwrap();
    node.partitions.complete(&lease).await.unwrap();
    // Preserve the exact pre-completion snapshot through the next read without
    // relying on host speed to hit the production 100ms cache window.
    *node.partitions.snapshot.lock().await = Snapshot {
        at: Instant::now(),
        heads,
    };
    let frontier = Frontier {
        partitions: node.partitions.clone(),
        plans: node.plans(),
    };
    let page = frontier.page(None).await.unwrap();
    assert!(page.plans.is_empty());
    assert!(page.issues.is_empty());
    // A valid completion stays absent even if an admission retry arrives.
    node.partitions.enqueue(&plan).await.unwrap();
    let id = crate::replicas::partition_model::partition_id(
        &node.partitions.membership.deployment,
        &plan,
        target,
    );
    let (mut current, version) = node
        .partitions
        .head(&id, &plan.identity.owner)
        .await
        .unwrap();
    assert!(current.queue.is_empty());
    // A truly contradictory durable head must still be refused. Only the
    // cached, already-advanced hint above is safe to discard.
    current.queue.push_back(target.id.clone());
    node.partitions
        .store
        .write(
            &id,
            &plan.identity.owner,
            &current,
            crate::replicas::store::Write {
                expected: Some(version),
                projection: Some(node.partitions.projection(true)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    *node.partitions.snapshot.lock().await = Snapshot {
        at: Instant::now(),
        heads: BTreeMap::from([(id, current)]),
    };
    let page = frontier.page(None).await.unwrap();
    assert!(page.plans.is_empty());
    assert_eq!(page.issues.len(), 1);
    assert_eq!(page.issues[0].error, AdmissionError::Corrupt);
    drop(node);
    database.cleanup().await;
}

async fn admitted(
    server: &crate::ingress::routing_store::test_server::Server,
    node: &crate::replicas::mongo_fixture::Node,
    index: usize,
) -> crate::ingress::routing_store::AdmittedPlan {
    use crate::ingress::{routing_plan::fixture, routing_store::Candidate};
    let mut activity = fixture::message("isolation fixture");
    activity.event_id = fixture::event_id(&format!("isolation-{index}"));
    activity.conversation.id = (1000 + index).to_string();
    let request = fixture::request(
        |value| {
            fixture::targets(value);
            value["partition"]["key"] = "conversation_and_sender".into();
            value["partition"]["custom_key"] = "fixed".into();
        },
        activity,
    );
    let candidate = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"fixture",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let mut plan = server.store().admit(&candidate).await.unwrap();
    plan.targets.truncate(1);
    plan.retained_until = time::OffsetDateTime::now_utc().unix_timestamp() + 3600;
    node.partitions.enqueue(&plan).await.unwrap();
    plan
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn bad_head_does_not_block_healthy_dispatch_hints() {
    use crate::replicas::{partition_model::partition_id, store::Write};
    for fault in [
        "missing_item",
        "item_mismatch",
        "missing_plan",
        "done_queued",
        "request_mismatch",
        "target_mismatch",
    ] {
        let database = Fixture::new().await;
        let node = database.node(1).await;
        let server = crate::ingress::routing_store::test_server::Server::start().await;
        let bad = admitted(&server, &node, 0).await;
        let healthy = admitted(&server, &node, 1).await;
        assert_ne!(
            partition_id(
                &node.partitions.membership.deployment,
                &bad,
                &bad.targets[0]
            ),
            partition_id(
                &node.partitions.membership.deployment,
                &healthy,
                &healthy.targets[0]
            )
        );
        let item_key = item_id(&bad.targets[0].id);
        let (mut item, version) = node
            .partitions
            .store
            .read::<Item>(&item_key, true)
            .await
            .unwrap()
            .unwrap();
        match fault {
            "missing_item" => {
                node.partitions
                    .store
                    .records
                    .clone()
                    .interactions()
                    .delete(&item_key.parse().unwrap(), version)
                    .await
                    .unwrap();
            }
            "missing_plan" => {
                server.memory.rows.lock().unwrap().remove(&bad.admission_id);
            }
            _ => {
                if fault == "item_mismatch" {
                    item.partition = uuid::Uuid::new_v4().to_string();
                } else if fault == "request_mismatch" {
                    item.request_hash = "different".into();
                } else if fault == "target_mismatch" {
                    item.target = uuid::Uuid::new_v4().to_string();
                } else {
                    item.done = true;
                }
                node.partitions
                    .store
                    .write(
                        &item_key,
                        &bad.identity.owner,
                        &item,
                        Write {
                            expected: Some(version),
                            retained_until: Some(bad.retained_until),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
            }
        }
        let frontier = Frontier {
            partitions: node.partitions.clone(),
            plans: server.store(),
        };
        let result = frontier.page(None).await;
        assert!(result.is_ok(), "one {fault} head failed the whole page");
        let page = result.unwrap();
        assert_eq!(page.plans.len(), 1, "{fault}");
        assert_eq!(page.plans[0].targets[0].id, healthy.targets[0].id);
        assert_eq!(page.issues.len(), 1, "{fault}");
        let error = page.issues[0].diagnostic.as_ref().unwrap();
        let (class, stage) = match fault {
            "missing_item" => (Class::MissingItem, Stage::ItemRead),
            "missing_plan" => (Class::MissingPlan, Stage::PlanLoad),
            "item_mismatch" => (Class::Mismatch, Stage::ItemCheck),
            "request_mismatch" | "target_mismatch" => (Class::Mismatch, Stage::PlanCheck),
            _ => (Class::Corrupt, Stage::ItemCheck),
        };
        assert_eq!((error.class, error.stage), (class, stage));
        let operator = crate::replicas::unresolved::Operator::new(
            node.partitions.clone(),
            server.store(),
            node.journal(),
        );
        let view = operator.inspect().await.unwrap();
        if fault == "missing_item" {
            assert!(view.unreadable.is_empty());
            assert_eq!(view.candidates.len(), 1);
            assert_eq!(view.candidates[0].state, "missing_item_expired");
            assert_eq!(view.candidates[0].binding.head_work, bad.targets[0].id);
        } else {
            assert_eq!(view.unreadable.len(), 1, "{fault}");
            assert_eq!(
                view.unreadable[0].age_seconds.is_some(),
                stage == Stage::PlanCheck
            );
            assert_eq!(
                (view.unreadable[0].class, view.unreadable[0].stage),
                (class, stage)
            );
        }

        assert_eq!(
            page.issues[0].record_id,
            partition_id(
                &node.partitions.membership.deployment,
                &bad,
                &bad.targets[0]
            )
        );
        // The hint is actionable under the real partition fence, despite the bad peer.
        let lease = node
            .partitions
            .claim(&healthy, &healthy.targets[0])
            .await
            .unwrap();
        node.partitions.complete(&lease).await.unwrap();
        drop(node);
        database.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn invalid_head_does_not_break_discovery_or_two_node_placement() {
    use crate::replicas::{partition_model::partition_id, store::Write};
    let database = Fixture::new().await;
    let nodes = [database.node(1).await, database.node(2).await];
    let server = crate::ingress::routing_store::test_server::Server::start().await;
    let bad = admitted(&server, &nodes[0], 0).await;
    let healthy = admitted(&server, &nodes[0], 1).await;
    let id = partition_id(
        &nodes[0].partitions.membership.deployment,
        &bad,
        &bad.targets[0],
    );
    let (mut head, version) = nodes[0]
        .partitions
        .head(&id, &bad.identity.owner)
        .await
        .unwrap();
    head.queue.push_back(head.queue[0].clone());
    nodes[0]
        .partitions
        .store
        .write(
            &id,
            &head.owner,
            &head,
            Write {
                expected: Some(version),
                projection: Some(nodes[0].partitions.projection(true)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let a = nodes[0].partitions.active_heads().await;
    assert!(a.is_ok(), "invalid head failed discovery");
    let a = a.unwrap();
    let b = nodes[1].partitions.active_heads().await.unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
    assert!(a.contains_key(&partition_id(
        &nodes[0].partitions.membership.deployment,
        &healthy,
        &healthy.targets[0]
    )));
    // Membership snapshots converge on the next heartbeat.
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert_eq!(
        nodes[0]
            .partitions
            .membership
            .assignments(a.into_keys())
            .unwrap(),
        nodes[1]
            .partitions
            .membership
            .assignments(b.into_keys())
            .unwrap()
    );
    drop(nodes);
    database.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn operator_list_preserves_unreadable_head_identity() {
    use crate::replicas::{partition_model::partition_id, store::Write, unresolved::Operator};
    let database = Fixture::new().await;
    let node = database.node(1).await;
    let server = crate::ingress::routing_store::test_server::Server::start().await;
    let bad = admitted(&server, &node, 0).await;
    let id = partition_id(
        &node.partitions.membership.deployment,
        &bad,
        &bad.targets[0],
    );
    let (mut head, version) = node
        .partitions
        .head(&id, &bad.identity.owner)
        .await
        .unwrap();
    head.kind = "invalid_head".into();
    let version = node
        .partitions
        .store
        .write(
            &id,
            &head.owner,
            &head,
            Write {
                expected: Some(version),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let operator = Operator::new(node.partitions.clone(), server.store(), node.journal());
    let result = operator.inspect().await;
    assert!(result.is_ok(), "one corrupt head failed operator list");
    let value = serde_json::to_value(result.unwrap()).unwrap();
    assert_eq!(value["unreadable"][0]["partition"], id);
    assert_eq!(
        value["unreadable"][0]["partition_hash"],
        ScanError::new(Class::Corrupt, Stage::HeadValidate, Some(&id)).partition_hash
    );
    assert_eq!(value["unreadable"][0]["head_version"], version);
    assert_eq!(value["unreadable"][0]["class"], "corrupt");
    assert_eq!(value["unreadable"][0]["stage"], "head_validate");
    drop(operator);
    drop(node);
    database.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn undecodable_and_empty_heads_are_isolated_with_physical_versions() {
    use crate::replicas::{partition_model::partition_id, store::Write, unresolved::Operator};
    let database = Fixture::new().await;
    let node = database.node(1).await;
    let server = crate::ingress::routing_store::test_server::Server::start().await;
    let mut expected = BTreeMap::new();
    for index in 0..2 {
        let plan = admitted(&server, &node, index).await;
        let id = partition_id(
            &node.partitions.membership.deployment,
            &plan,
            &plan.targets[0],
        );
        let (mut head, version) = node
            .partitions
            .head(&id, &plan.identity.owner)
            .await
            .unwrap();
        head.queue.clear();
        let mut value = serde_json::to_value(&head).unwrap();
        if index == 0 {
            value["queue"] = serde_json::json!(7);
        }
        let version = node
            .partitions
            .store
            .write(
                &id,
                &head.owner,
                &value,
                Write {
                    expected: Some(version),
                    projection: Some(node.partitions.projection(true)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        expected.insert(
            id,
            (version, if index == 0 { "head_decode" } else { "front" }),
        );
    }
    admitted(&server, &node, 2).await;
    assert_eq!(node.partitions.active_heads().await.unwrap().len(), 1);
    let operator = Operator::new(node.partitions.clone(), server.store(), node.journal());
    let value = serde_json::to_value(operator.inspect().await.unwrap()).unwrap();
    assert_eq!(value["unreadable"].as_array().unwrap().len(), 2);
    for row in value["unreadable"].as_array().unwrap() {
        let (version, stage) = expected.get(row["partition"].as_str().unwrap()).unwrap();
        assert_eq!(row["head_version"], *version);
        assert_eq!(row["stage"], *stage);
        assert!(row["age_seconds"].is_null());
    }
    drop(operator);
    drop(node);
    database.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn transient_head_advances_pages_and_retries_without_starving_dispatch() {
    use crate::replicas::partition_model::partition_id;
    use crate::turns::routed::scheduler::{
        Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem,
    };
    use std::time::Duration;
    struct Dispatch {
        partitions: Partitions,
        plans: BTreeMap<String, crate::ingress::routing_store::AdmittedPlan>,
        sent: tokio::sync::mpsc::UnboundedSender<String>,
    }
    impl TargetExecutor for Dispatch {
        fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
            Box::pin(async move {
                let plan = &self.plans[&work.target_id];
                let lease = self.partitions.claim(plan, &plan.targets[0]).await.unwrap();
                self.partitions.complete(&lease).await.unwrap();
                self.sent.send(work.target_id.clone()).unwrap();
                Disposition::Settled
            })
        }
    }
    let database = Fixture::new().await;
    let node = database.node(1).await;
    let server = crate::ingress::routing_store::test_server::Server::start().await;
    let mut ordered = BTreeMap::new();
    for index in 0..34 {
        let plan = admitted(&server, &node, index).await;
        ordered.insert(
            partition_id(
                &node.partitions.membership.deployment,
                &plan,
                &plan.targets[0],
            ),
            plan,
        );
    }
    // Put the persistent failure at the beginning, with healthy heads beyond page one.
    let (bad_id, bad) = ordered.pop_first().unwrap();
    *server.memory.unavailable_read_id.lock().unwrap() = Some(bad.admission_id.clone());
    let frontier = Frontier {
        partitions: node.partitions.clone(),
        plans: server.store(),
    };
    for _ in 0..3 {
        let first = frontier.page(None).await.unwrap();
        assert_eq!(first.plans.len(), 31);
        assert_eq!(first.issues.len(), 1);
        let error = first.issues[0].diagnostic.as_ref().unwrap();
        assert_eq!(error.class, Class::Unavailable);
        assert_eq!(error.stage, Stage::PlanLoad);
        let next = first.next_cursor.as_deref().unwrap();
        let second = frontier.page(Some(next)).await.unwrap();
        assert_eq!(second.plans.len(), 2);
        assert!(second.next_cursor.is_none());
        // Immediate sweeps keep other hints moving while this head backs off.
        assert!(frontier.page(None).await.unwrap().issues.is_empty());
        // Deterministically advance only this retry deadline; no Mongo lease clock changes.
        node.partitions.isolation.lock().unwrap().retry_now(&bad_id);
    }
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let expected = ordered
        .values()
        .map(|p| p.targets[0].id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let executor = Dispatch {
        partitions: node.partitions.clone(),
        plans: ordered
            .into_values()
            .map(|p| (p.targets[0].id.clone(), p))
            .collect(),
        sent,
    };
    let (scheduler, _handle) = Scheduler::new(
        Arc::new(frontier.clone()),
        Arc::new(executor),
        ScheduleConfig {
            concurrency: 1,
            queue_capacity: 64,
            page_delay: Duration::from_millis(10),
            sweep_delay: Duration::from_millis(10),
            drain_timeout: Duration::from_secs(1),
        },
    )
    .unwrap();
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(scheduler.run(receiver));
    let mut dispatched = std::collections::BTreeSet::new();
    tokio::time::timeout(Duration::from_secs(15), async {
        while dispatched.len() < expected.len() {
            dispatched.insert(received.recv().await.unwrap());
        }
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    let report = task.await.unwrap();
    assert_eq!(dispatched, expected);
    assert_eq!(report.started, 33);
    assert_eq!(report.finished, 33);
    assert!(!report.recovery_failed);
    // Restored evidence is retried under the same durable head, not quarantined forever.
    *server.memory.unavailable_read_id.lock().unwrap() = None;
    node.partitions.isolation.lock().unwrap().retry_now(&bad_id);
    *node.partitions.snapshot.lock().await = Snapshot::default();
    let recovered = frontier.page(None).await.unwrap();
    assert_eq!(recovered.plans.len(), 1);
    assert_eq!(recovered.plans[0].targets[0].id, bad.targets[0].id);
    assert!(recovered.issues.is_empty());
    drop(frontier);
    drop(node);
    database.cleanup().await;
}
