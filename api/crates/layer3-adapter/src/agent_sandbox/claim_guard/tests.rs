use super::*;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

fn observed(uid: &str, restarts: i32, ready: bool, crash: bool) -> Sample {
    let (member, pod) = resources(uid, restarts, ready, crash);
    sample(&member, &pod).unwrap()
}

fn resources(uid: &str, restarts: i32, ready: bool, crash: bool) -> (DynamicObject, Pod) {
    let member: DynamicObject = serde_json::from_value(json!({
        "apiVersion":"agents.x-k8s.io/v1beta1", "kind":"Sandbox",
        "metadata":{"name":"member", "uid":"member-uid", "generation":2},
        "status":{"conditions":[{"type":"Ready", "status":"True", "observedGeneration":2}]}
    }))
    .unwrap();
    let pod: Pod = serde_json::from_value(json!({
        "metadata":{"name":"pod", "uid":uid},
        "status":{"phase":"Running", "podIP":"10.0.0.7",
            "conditions":[{"type":"Ready", "status":if ready {"True"} else {"False"}}],
            "containerStatuses":[{"name":"runtime", "image":"image", "imageID":"image-id",
                "ready":ready, "restartCount":restarts,
                "state":if crash {json!({"waiting":{"reason":"CrashLoopBackOff"}})} else {json!({"running":{}})}
            }]}
    })).unwrap();
    (member, pod)
}

#[test]
fn stale_member_generation_and_incomplete_pod_readiness_are_pending() {
    for change in [
        "generation",
        "container",
        "pod",
        "terminating",
        "missing_runtime",
    ] {
        let (mut member, mut pod) = resources("pod-uid", 0, true, false);
        match change {
            "generation" => member.metadata.generation = Some(3),
            "container" => {
                pod.status
                    .as_mut()
                    .unwrap()
                    .container_statuses
                    .as_mut()
                    .unwrap()[0]
                    .ready = false
            }
            "pod" => {
                pod.status.as_mut().unwrap().conditions.as_mut().unwrap()[0].status = "False".into()
            }
            "terminating" => {
                pod.metadata.deletion_timestamp =
                    Some(serde_json::from_value(json!("2026-11-13T12:00:00Z")).unwrap())
            }
            _ => pod.status.as_mut().unwrap().container_statuses = None,
        }
        let observed = sample(&member, &pod).unwrap();
        assert!(observed.binding.is_none(), "{change}");
        assert!(!observed.crash_loop, "{change}");
    }
}

#[test]
fn stale_unschedulable_condition_does_not_override_current_pod_state() {
    let (member, mut pod) = resources("pod-uid", 0, false, false);
    pod.spec = Some(
        serde_json::from_value(json!({
            "containers": [{"name": "runtime", "image": "image"}],
            "nodeName": "test-node"
        }))
        .unwrap(),
    );
    let status = pod.status.as_mut().unwrap();
    status.phase = Some("Pending".into());
    status.conditions = Some(
        serde_json::from_value(json!([
            {"type": "PodScheduled", "status": "False", "reason": "Unschedulable"}
        ]))
        .unwrap(),
    );
    assert!(sample(&member, &pod).is_ok());
    pod.spec.as_mut().unwrap().node_name = None;
    pod.metadata.deletion_timestamp =
        Some(serde_json::from_value(json!("2026-11-26T12:00:00Z")).unwrap());
    assert!(sample(&member, &pod).is_ok());
}

#[test]
fn scheduler_diagnostics_report_only_unique_classes_for_the_pending_pod() {
    let (member, mut pod) = resources("fixture-pod-uid", 0, false, false);
    pod.spec = Some(serde_json::from_value(json!({"containers": []})).unwrap());
    pod.status.as_mut().unwrap().phase = Some("Pending".into());
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let _span = tracing::warn_span!("claim", owner = "fixture-owner").entered();
        for message in [
            "0/6 nodes are available: 2 Insufficient cpu, 2 Insufficient memory, 1 node(s) had volume node affinity conflict, 1 node(s) had untolerated taint {fixture-key: fixture-value}, 1 node(s) didn't match Pod's node affinity/selector. preemption: 1 exceeded quota: fixture-preemption.",
            "exceeded quota: fixture-quota, requested: requests.cpu=1",
            "node fixture-node reported an unrecognized condition",
        ] {
            pod.status.as_mut().unwrap().conditions = Some(
                serde_json::from_value(json!([{
                    "type": "PodScheduled", "status": "False", "reason": "Unschedulable",
                    "message": message
                }]))
                .unwrap(),
            );
            assert!(matches!(
                sample(&member, &pod),
                Err(AdapterError::NativeClaimNotReady(NativeClaimGuardReason::Unschedulable))
            ));
        }
    });
    let bytes = output.lock().unwrap();
    let log = std::str::from_utf8(&bytes).unwrap();
    for class in ["cpu", "memory", "placement", "volume", "quota"] {
        assert_eq!(
            log.matches(&format!("scheduler_class=\"{class}\"")).count(),
            1,
            "{log}"
        );
    }
    assert!(!log.contains("fixture-"), "{log}");
    assert!(!log.contains("requested:"), "{log}");
    assert_eq!(log.lines().count(), 5, "{log}");
}

#[tokio::test(start_paused = true)]
async fn healthy_and_pending_then_ready_keep_the_original_binding() {
    for pending in [false, true] {
        let samples = Mutex::new(VecDeque::from(if pending {
            vec![
                observed("pod-uid", 8, false, false),
                observed("pod-uid", 8, true, false),
            ]
        } else {
            vec![observed("pod-uid", 8, true, false)]
        }));
        let start = tokio::time::Instant::now();
        let binding = wait("bind", || async {
            Ok(samples.lock().unwrap().pop_front().unwrap())
        })
        .await
        .unwrap();
        assert_eq!(binding.sandbox_uid, "member-uid");
        assert_eq!(binding.pod_uid, "pod-uid");
        assert_eq!(start.elapsed(), if pending { POLL } else { Duration::ZERO });
    }
}

#[tokio::test(start_paused = true)]
async fn crash_loop_exits_immediately_and_rising_restarts_exit_on_next_sample() {
    for (samples, reason, elapsed) in [
        (
            vec![observed("pod-uid", 4, false, true)],
            NativeClaimGuardReason::CrashLoop,
            Duration::ZERO,
        ),
        (
            vec![
                observed("pod-uid", 4, false, false),
                observed("pod-uid", 5, true, false),
            ],
            NativeClaimGuardReason::Restarting,
            POLL,
        ),
    ] {
        let samples = Mutex::new(VecDeque::from(samples));
        let start = tokio::time::Instant::now();
        let error = wait("home", || async {
            Ok(samples.lock().unwrap().pop_front().unwrap())
        })
        .await
        .unwrap_err();
        assert_eq!(error, AdapterError::NativeClaimNotReady(reason));
        assert!(!error.is_runtime_loss());
        assert_eq!(error.clone().context("private detail"), error);
        assert_eq!(start.elapsed(), elapsed);
    }
}

#[tokio::test(start_paused = true)]
async fn replacement_pod_does_not_inherit_restart_history() {
    let samples = Mutex::new(VecDeque::from([
        observed("old-pod", 2, false, false),
        observed("new-pod", 9, true, false),
    ]));
    let binding = wait("bind", || async {
        Ok(samples.lock().unwrap().pop_front().unwrap())
    })
    .await
    .unwrap();
    assert_eq!(binding.pod_uid, "new-pod");
}

#[tokio::test(start_paused = true)]
async fn pending_readiness_and_stalled_api_read_are_bounded() {
    for stalled in [false, true] {
        let start = tokio::time::Instant::now();
        let error = wait("bind", || async {
            if stalled {
                std::future::pending::<Result<Sample>>().await
            } else {
                Ok(observed("pod-uid", 0, false, false))
            }
        })
        .await
        .unwrap_err();
        assert_eq!(
            error,
            AdapterError::NativeClaimNotReady(if stalled {
                NativeClaimGuardReason::TimedOut
            } else {
                NativeClaimGuardReason::Pending
            })
        );
        assert_eq!(start.elapsed(), BUDGET);
    }
}

#[tokio::test(start_paused = true)]
async fn prior_pending_cannot_expedite_a_stalled_or_unavailable_observation() {
    for prior_pending in [false, true] {
        for stalled in [false, true] {
            let reads = std::sync::atomic::AtomicUsize::new(0);
            let start = tokio::time::Instant::now();
            let error = wait("bind", || async {
                let read = reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if prior_pending && read == 0 {
                    return Ok(observed("pod-uid", 0, false, false));
                }
                if stalled {
                    std::future::pending::<Result<Sample>>().await
                } else {
                    Err(AdapterError::Unavailable("transport".into()))
                }
            })
            .await
            .unwrap_err();
            assert_eq!(
                error,
                AdapterError::NativeClaimNotReady(NativeClaimGuardReason::TimedOut)
            );
            assert_eq!(start.elapsed(), BUDGET);
            if prior_pending {
                assert!(reads.load(std::sync::atomic::Ordering::Relaxed) >= 2);
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn identity_and_api_refusals_are_not_retried_as_readiness() {
    for error in [
        AdapterError::Rejected("wrong owner".into()),
        AdapterError::AtCapacity("limit".into()),
    ] {
        let start = tokio::time::Instant::now();
        assert_eq!(
            wait("home", || async { Err(error.clone()) })
                .await
                .unwrap_err(),
            error
        );
        assert_eq!(start.elapsed(), Duration::ZERO);
    }
}

#[tokio::test(start_paused = true)]
async fn metrics_are_bounded_and_include_cancellation() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    wait("bind", || async {
        Ok(observed("private-pod", 0, true, false))
    })
    .await
    .unwrap();
    assert!(wait("home", || async {
        Ok(observed("private-pod", 3, false, true))
    })
    .await
    .is_err());
    assert!(wait("home", || async {
        Err(AdapterError::NativeClaimNotReady(
            NativeClaimGuardReason::Unschedulable,
        ))
    })
    .await
    .is_err());
    assert!(tokio::time::timeout(
        Duration::from_millis(1),
        wait("bind", std::future::pending::<Result<Sample>>)
    )
    .await
    .is_err());
    let rendered = handle.render();
    for outcome in ["ready", "crash_loop", "unschedulable", "cancelled"] {
        assert!(
            rendered
                .lines()
                .any(|line| line.starts_with("cma_native_claim_guard_total{")
                    && line.contains(&format!("outcome=\"{outcome}\""))
                    && line.ends_with(" 1")),
            "{rendered}"
        );
    }
    assert!(rendered.contains("cma_native_claim_guard_seconds_count"));
    assert!(!rendered.contains("private-pod"));
}
