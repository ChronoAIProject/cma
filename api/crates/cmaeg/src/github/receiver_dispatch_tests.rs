use super::super::retryable_problem;
use super::{dispatch_batch, DispatchChannel};
use cmaeg_core::problem::{Problem, ProblemCode};

fn pending(count: usize) -> Vec<DispatchChannel> {
    (0..count)
        .map(|index| DispatchChannel {
            channel_id: format!("channel-{index}"),
            record_id: format!("record-{index}"),
            channel_version: 1,
            status: "pending".into(),
            reason: None,
            admission_id: None,
            attempts: 0,
            next_attempt_at: 100,
        })
        .collect()
}

#[test]
fn every_channel_settles_across_multiple_bounded_batches() {
    let mut channels = pending(49);
    let mut admitted = Vec::new();
    for _ in 0..4 {
        let batch = dispatch_batch(&channels, 100);
        assert!(!batch.is_empty());
        assert!(batch.len() <= 16);
        for index in batch {
            channels[index].status = "dispatched".into();
            admitted.push(index);
        }
    }
    assert_eq!(admitted, (0..49).collect::<Vec<_>>());
    assert!(dispatch_batch(&channels, 100).is_empty());
}

#[test]
fn terminal_and_future_entries_leave_the_budget_for_due_work() {
    let mut channels = pending(65);
    for channel in &mut channels[..16] {
        channel.status = "dispatched".into();
    }
    for channel in &mut channels[16..32] {
        channel.status = "refused".into();
    }
    for channel in &mut channels[32..48] {
        channel.next_attempt_at = 101;
    }
    assert_eq!(dispatch_batch(&channels, 100), (48..64).collect::<Vec<_>>());
    channels[64].next_attempt_at = 99;
    assert_eq!(dispatch_batch(&channels, 99), vec![64]);
}

#[test]
fn repeated_failures_yield_to_older_due_work_after_reload() {
    let mut channels = pending(49);
    let mut attempted = vec![false; channels.len()];
    // Each tick arrives after every retry deadline: filtering alone is insufficient.
    for tick in 0..4 {
        let now = 100 + tick * 1000;
        let batch = dispatch_batch(&channels, now);
        assert_eq!(batch.len(), 16);
        for index in batch {
            attempted[index] = true;
            channels[index].attempts += 1;
            channels[index].next_attempt_at = now + 300;
        }
        channels = serde_json::from_slice(&serde_json::to_vec(&channels).unwrap()).unwrap();
    }
    assert!(attempted.into_iter().all(|attempted| attempted));
}

#[test]
fn provider_refusals_retry_but_definitive_route_refusals_settle() {
    assert!(retryable_problem(&Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "provider unavailable",
    )));
    assert!(retryable_problem(&Problem::new(
        ProblemCode::RateLimited,
        429,
        "busy",
    )));
    assert!(!retryable_problem(&Problem::new(
        ProblemCode::AdmissionRefused,
        422,
        "route removed",
    )));
}
