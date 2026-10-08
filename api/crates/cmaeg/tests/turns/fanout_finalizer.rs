use super::{
    fanout_support::*,
    routing_fixture::{NYX, REVIEWER},
};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::{DeliveryCommand, TurnOutcome},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Notify;

fn filler(h: &Harness) -> DeliveryCommand {
    DeliveryCommand::StreamOutcome {
        turn: Arc::new(h.base.request("filler")),
        outcome: TurnOutcome::Accepted {
            response_id: "filler".into(),
        },
    }
}

#[tokio::test]
async fn a_full_queue_never_blocks_acceptance_and_failure_survives_handler_drop() {
    let mut h = Harness::new().await;
    h.delivery.try_submit(filler(&h)).unwrap();
    h.recorder
        .outcomes
        .lock()
        .unwrap()
        .insert(REVIEWER.into(), refused("provider_rejected"));
    let started = Instant::now();
    let result = h
        .launcher()
        .launch_routed(h.request(&["default", REVIEWER]), None)
        .await;
    elapsed_bound(started);
    drop(result); // The ingress future and its returned response are gone.
    assert_eq!(h.base.tracker.len(), 1);
    assert!(matches!(
        h.failure().await,
        DeliveryCommand::StreamOutcome { .. }
    ));
    assert!(
        matches!(h.failure().await, DeliveryCommand::ConsumerFailed { consumer, code: "provider_rejected", .. } if consumer.as_str() == REVIEWER)
    );
    idle(&h.base.tracker).await;
    assert!(h.outcomes.try_recv().is_err());
}

#[tokio::test]
async fn refused_finalizer_launches_nothing_and_drops_all_child_leases() {
    for draining in [false, true] {
        let h = Harness::new().await;
        if draining {
            h.base.tracker.start_draining();
        } else {
            for _ in 0..4096 {
                h.base
                    .tracker
                    .spawn(&h.base.request("occupied"), std::future::pending())
                    .unwrap();
            }
        }
        let request = h.request(&[REVIEWER, "default"]);
        let base = request
            .event_ref
            .open(&keys(), time::OffsetDateTime::now_utc().unix_timestamp())
            .unwrap()
            .payload;
        let expected = cmaeg_core::ids::ConversationId::derive_for_consumer(
            &keys(),
            &request.channel.id,
            request.channel.definition.partition.version,
            &cmaeg_core::ids::ConsumerId::parse(REVIEWER).unwrap(),
            &base.partition_key,
        );
        let result = h.launcher().launch_routed(request, None).await;
        assert_eq!(result.primary_conversation, expected);
        assert_eq!(result.consumers[0].conversation_id, expected);
        assert_eq!(result.consumers[1].conversation_id, base.conversation_id);
        let LaunchOutcome::Refused(refusal) = result.combined else {
            panic!()
        };
        assert_eq!(
            refusal.code,
            if draining {
                "draining"
            } else {
                "turn_capacity"
            }
        );
        assert_eq!(refusal.problem.status, 503);
        assert!(h.recorder.requests.lock().unwrap().is_empty());
        let leases = (0..4)
            .map(|_| {
                h.capacity
                    .try_lease(
                        &h.base.channel.pointer.record_id,
                        &h.base.channel.definition.owner.subject,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        drop(leases);
        h.base.tracker.drain(Instant::now()).await;
        idle(&h.base.tracker).await;
    }
}

#[test]
fn finalizer_counts_deadline_closed_and_unanswered_without_late_delivery() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut h = Harness::new().await;
                h.delivery.try_submit(filler(&h)).unwrap();
                for id in [REVIEWER, NYX] {
                    h.recorder
                        .outcomes
                        .lock()
                        .unwrap()
                        .insert(id.into(), refused("provider_busy"));
                }
                h.wrap(h.recorder.clone(), Duration::from_millis(30))
                    .launch_routed(h.request(&["default", REVIEWER, NYX]), None)
                    .await;
                idle(&h.base.tracker).await;
                assert!(matches!(
                    h.failure().await,
                    DeliveryCommand::StreamOutcome { .. }
                ));
                assert!(h.outcomes.try_recv().is_err());
                assert!(handle
                    .render()
                    .contains("cmaeg_fanout_consumer_failed_dropped_total{reason=\"deadline\"} 2"));

                h.outcomes.close();
                h.launcher()
                    .launch_routed(h.request(&["default", REVIEWER]), None)
                    .await;
                idle(&h.base.tracker).await;
                assert!(handle
                    .render()
                    .contains("cmaeg_fanout_consumer_failed_dropped_total{reason=\"closed\"} 1"));

                let block = Arc::new(Recorder {
                    block: Some(Arc::new(Notify::new())),
                    ..Default::default()
                });
                let launcher = h.wrap(block.clone(), Duration::from_secs(1));
                assert!(tokio::time::timeout(
                    Duration::from_millis(10),
                    launcher.launch_routed(h.request(&["default", REVIEWER]), None)
                )
                .await
                .is_err());
                idle(&h.base.tracker).await;
                assert!(handle.render().contains(
                    "cmaeg_fanout_consumer_failed_dropped_total{reason=\"unanswered\"} 1"
                ));
            });
    });
}

#[tokio::test]
async fn drain_aborts_a_finalizer_waiting_on_delivery() {
    let h = Harness::new().await;
    h.delivery.try_submit(filler(&h)).unwrap();
    h.recorder
        .outcomes
        .lock()
        .unwrap()
        .insert(REVIEWER.into(), refused("provider_busy"));
    h.launcher()
        .launch_routed(h.request(&["default", REVIEWER]), None)
        .await;
    assert_eq!(h.base.tracker.len(), 1);
    assert!(!h.base.tracker.drain(Instant::now()).await);
    idle(&h.base.tracker).await;
}
