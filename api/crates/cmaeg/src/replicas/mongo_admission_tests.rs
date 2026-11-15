//! Exercise the complete root-to-queue path with independent runtime clients.
use super::super::{
    mongo_fixture::Fixture,
    partition_model::{partition_id, Head},
};
use crate::ingress::{
    routing_plan::{self, fixture, OrdinaryPlan, RoutingOptions},
    routing_store::{Candidate, PlanStore},
};
use cmaeg_core::{
    ids::{ConversationId, EventId, IdempotencyKey},
    seal::{event_ref::activity_digest, EventRef},
};
use std::{collections::BTreeSet, sync::Arc};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn concurrent_root_and_queue_admissions_all_return_original_custody() {
    let metrics = crate::telemetry::install().unwrap();
    let aborted_before = aborted_count(&metrics);
    let rereads_before = reread_count(&metrics);
    let fixture_db = Fixture::new().await;
    let nodes = vec![
        fixture_db.node(1).await,
        fixture_db.node(2).await,
        fixture_db.node(3).await,
    ];
    let stores: Vec<_> = nodes
        .iter()
        .map(|node| {
            PlanStore::new(
                node.partitions.store.records.clone(),
                node.partitions.store.keys.clone(),
            )
            .with_admission_queue(Arc::new(node.partitions.clone()))
        })
        .collect();
    let keys = nodes[0].partitions.store.keys.clone();
    let mut first = None;
    for index in 0..12 {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut request =
            fixture::request(fixture::targets, fixture::message("concurrent fixture"));
        let mut event = request
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        request.activity.event_id = EventId::new(format!("concurrent-{index}")).unwrap();
        request.idempotency_key = IdempotencyKey::from_event(&request.activity.event_id);
        request.conversation_id =
            ConversationId::derive(&keys, &request.channel.id, 1, &event.partition_key);
        event.event_id = request.activity.event_id.clone();
        event.activity_sha256 = Some(activity_digest(&request.activity));
        event.conversation_id = request.conversation_id.clone();
        event.expires_at = now + 900;
        request.event_ref = EventRef::seal(&event, &keys).unwrap();
        let OrdinaryPlan::Routed(plan) = routing_plan::plan(
            &request,
            &keys,
            now,
            RoutingOptions {
                enabled: true,
                attributes_signed: true,
                non_human_providers: &BTreeSet::new(),
            },
        )
        .unwrap() else {
            panic!("routed fixture required");
        };
        let candidate =
            Candidate::prepare(&request, &plan, b"same original bytes", &keys, now).unwrap();
        let (a, b, c) = tokio::join!(
            stores[0].admit(&candidate),
            stores[1].admit(&candidate),
            stores[2].admit(&candidate)
        );
        let accepted = [&a, &b, &c].iter().filter(|result| result.is_ok()).count();
        let root_present = stores[0]
            .load(candidate.identity())
            .await
            .unwrap()
            .is_some();
        println!("replica_contention_round index={index} calls=3 accepted={accepted} root_present={root_present} known_aborted_delta={} verified_reread_delta={}", aborted_count(&metrics).saturating_sub(aborted_before), reread_count(&metrics).saturating_sub(rereads_before));
        let a = a.unwrap_or_else(|error| panic!("first admission failed: {error:?}"));
        let b = b.unwrap_or_else(|error| panic!("second admission failed: {error:?}"));
        let c = c.unwrap_or_else(|error| panic!("third admission failed: {error:?}"));
        assert_eq!(a.admission_id, b.admission_id);
        assert_eq!(a.admission_id, c.admission_id);
        assert!(a.targets.iter().zip(&b.targets).all(|(a, b)| a.id == b.id));
        assert!(a.targets.iter().zip(&c.targets).all(|(a, c)| a.id == c.id));
        first.get_or_insert(a);
    }
    let first = first.unwrap();
    for target in &first.targets {
        let id = partition_id(&nodes[0].partitions.membership.deployment, &first, target);
        let (head, _) = nodes[0]
            .partitions
            .store
            .read::<Head>(&id, false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.queue.len(), 12);
        assert_eq!(head.queue.front(), Some(&target.id));
    }
    println!("replica_contention clients=3 unique_events=12 accepted_calls=36 targets_per_event=3");
    drop(nodes);
    fixture_db.cleanup().await;
}

fn aborted_count(metrics: &metrics_exporter_prometheus::PrometheusHandle) -> u64 {
    counter(metrics, "cmaeg_replica_transaction_retries_total ")
}

fn reread_count(metrics: &metrics_exporter_prometheus::PrometheusHandle) -> u64 {
    counter(
        metrics,
        "cmaeg_replica_admission_rereads_total{outcome=\"verified\"} ",
    )
}

fn counter(metrics: &metrics_exporter_prometheus::PrometheusHandle, name: &str) -> u64 {
    metrics
        .render()
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(0)
}
