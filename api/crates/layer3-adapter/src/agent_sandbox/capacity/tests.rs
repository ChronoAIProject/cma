use super::*;
use std::time::Duration;
use RuntimePermitKind::{Request, Stream};
use RuntimeWorkloadClass::{Continuity, Control, Ordinary};

fn configured() -> Arc<RuntimeCapacityAuthority> {
    RuntimeCapacityAuthority::new(RuntimeCapacityConfig {
        stream_limit: 3,
        stream_reserve: 1,
        request_limit: 3,
        request_reserve: 1,
        request_queue_limit: 2,
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn every_class_respects_physical_limit_and_protects_reserve() {
    for kind in [Stream, Request] {
        for privileged in [Continuity, Control] {
            let authority = configured();
            let ordinary: Vec<_> = (0..2)
                .map(|_| authority.try_acquire(kind, Ordinary).unwrap())
                .collect();
            assert_eq!(
                authority.try_acquire(kind, Ordinary).unwrap_err().reason,
                RuntimeCapacityReason::Exhausted
            );
            let reserved = authority.try_acquire(kind, privileged).unwrap();
            assert!(authority.try_acquire(kind, privileged).is_err());
            drop(reserved);
            let resumed = authority.try_acquire(kind, privileged).unwrap();
            drop((resumed, ordinary));
            assert_eq!(authority.semaphore(kind).available_permits(), 3);
        }
    }
}

#[test]
fn qualification_arithmetic_includes_one_instance_loss_and_reserve() {
    let config = RuntimeCapacityConfig {
        stream_reserve: 16,
        request_reserve: 8,
        ..Default::default()
    };
    assert_eq!(config.required_ready_instances(200, 120).unwrap(), 6);
    assert_eq!(config.required_ready_instances(200, 200).unwrap(), 10);
    assert_eq!((4 - 1) * config.usable_streams(), 144);
    let authorities: Vec<_> = (0..5)
        .map(|_| RuntimeCapacityAuthority::new(config).unwrap())
        .collect();
    let active: Vec<_> = (0..200)
        .map(|index| {
            authorities[index % 5]
                .try_acquire(Stream, Ordinary)
                .unwrap()
        })
        .collect();
    assert_eq!(active.len(), 200);
    assert_eq!(
        authorities
            .iter()
            .map(|a| a.available_streams() - config.stream_reserve)
            .sum::<usize>(),
        40
    );
}

#[tokio::test(start_paused = true)]
async fn queue_is_bounded_fifo_and_control_bypasses_it() {
    let authority = configured();
    let mut held: Vec<_> = (0..2)
        .map(|_| authority.try_acquire(Request, Ordinary).unwrap())
        .collect();
    let first = tokio::spawn({
        let a = authority.clone();
        async move { a.acquire_request(Ordinary).await }
    });
    tokio::task::yield_now().await;
    let second = tokio::spawn({
        let a = authority.clone();
        async move { a.acquire_request(Ordinary).await }
    });
    tokio::task::yield_now().await;
    assert_eq!(
        authority
            .acquire_request(Ordinary)
            .await
            .unwrap_err()
            .reason,
        RuntimeCapacityReason::QueueFull
    );
    let control = authority.acquire_request(Control).await.unwrap();
    assert!(!first.is_finished());
    drop(control);
    drop(held.pop());
    let first = first.await.unwrap().unwrap();
    assert!(!second.is_finished());
    drop(first);
    let second = second.await.unwrap().unwrap();
    drop((held, second));
    assert!(authority.state.lock().unwrap().queue.is_empty());
}

#[tokio::test(start_paused = true)]
async fn queue_timeout_and_cancellation_remove_tickets_and_return_permits() {
    let authority = configured();
    let held: Vec<_> = (0..2)
        .map(|_| authority.try_acquire(Request, Ordinary).unwrap())
        .collect();
    let first = tokio::spawn({
        let a = authority.clone();
        async move { a.acquire_request(Ordinary).await }
    });
    tokio::task::yield_now().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(authority.state.lock().unwrap().queue.is_empty());
    assert_eq!(
        authority
            .acquire_request(Ordinary)
            .await
            .unwrap_err()
            .reason,
        RuntimeCapacityReason::QueueTimedOut
    );
    assert!(authority.state.lock().unwrap().queue.is_empty());
    drop(held);
    assert!(authority.acquire_request(Ordinary).await.is_ok());
}

#[tokio::test]
async fn disabled_queue_refuses_immediately_and_clones_share_authority() {
    let authority = RuntimeCapacityAuthority::new(RuntimeCapacityConfig {
        request_limit: 1,
        ..Default::default()
    })
    .unwrap();
    let clone = authority.clone();
    let permit = authority.acquire_request(Ordinary).await.unwrap();
    assert_eq!(
        clone.acquire_request(Ordinary).await.unwrap_err().reason,
        RuntimeCapacityReason::Exhausted
    );
    assert!(permit.proves_request(&clone));
    assert!(!permit.proves_request(&RuntimeCapacityAuthority::new(Default::default()).unwrap()));
    drop(permit);
    assert_eq!(clone.available_requests(), 1);
}

#[test]
fn invalid_limits_and_reserves_fail_closed() {
    for config in [
        RuntimeCapacityConfig {
            stream_limit: 0,
            ..Default::default()
        },
        RuntimeCapacityConfig {
            request_limit: 4097,
            ..Default::default()
        },
        RuntimeCapacityConfig {
            stream_reserve: 64,
            ..Default::default()
        },
        RuntimeCapacityConfig {
            request_reserve: 32,
            ..Default::default()
        },
        RuntimeCapacityConfig {
            request_queue_limit: 1025,
            ..Default::default()
        },
        RuntimeCapacityConfig {
            request_queue_max_wait: Duration::ZERO,
            ..Default::default()
        },
    ] {
        assert!(RuntimeCapacityAuthority::new(config).is_err());
    }
}

#[test]
fn metrics_report_occupancy_release_capacity_and_refusal_without_identity_labels() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let authority = configured();
        let first = authority.try_acquire(Stream, Ordinary).unwrap();
        let second = authority.try_acquire(Stream, Ordinary).unwrap();
        assert!(authority.try_acquire(Stream, Ordinary).is_err());
        assert!(handle
            .render()
            .contains("cma_adapter_runtime_permits_in_use{kind=\"stream\"} 2"));
        drop((first, second));
    });
    let text = handle.render();
    assert!(text.contains("cma_adapter_runtime_permits_in_use{kind=\"stream\"} 0"));
    assert!(text.contains("cma_adapter_runtime_permits_available{kind=\"stream\"} 3"));
    assert!(text.contains("cma_adapter_runtime_refusals_total{kind=\"stream\",class=\"ordinary\",reason=\"exhausted\"} 1"));
    assert!(text.contains("cma_adapter_runtime_capacity{kind=\"request\",scope=\"usable\"} 2"));
    assert!(!text.contains("agent_id"));
}

#[tokio::test]
async fn nested_continuity_reads_use_reserve_and_failure_is_not_runtime_loss() {
    let authority = configured();
    let held: Vec<_> = (0..2)
        .map(|_| authority.try_acquire(Request, Ordinary).unwrap())
        .collect();
    let permit = Continuity
        .scope(authority.acquire_request(Ordinary))
        .await
        .unwrap();
    let failure = Continuity
        .scope(authority.acquire_request(Ordinary))
        .await
        .unwrap_err();
    assert_eq!(failure.class, Continuity);
    let error = crate::AdapterError::from(failure).context("reopen");
    assert!(!error.is_runtime_loss());
    assert!(matches!(error, crate::AdapterError::RuntimeCapacity(_)));
    drop((permit, held));
}
