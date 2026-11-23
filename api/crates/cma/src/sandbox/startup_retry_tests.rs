use super::{retry_unavailable, DELAYS};
use layer2_core::sandbox::SandboxStoreError;
use std::{collections::VecDeque, sync::Mutex, time::Duration};
use tokio::time::Instant;

fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("Kubernetes runtime stream opening timed out".into())
}

async fn run(
    outcomes: Vec<Result<(), SandboxStoreError>>,
) -> (Result<(), SandboxStoreError>, usize, Duration) {
    let outcomes = Mutex::new(VecDeque::from(outcomes));
    let calls = Mutex::new(0);
    let started = Instant::now();
    let result = retry_unavailable(&DELAYS, || {
        *calls.lock().unwrap() += 1;
        let next = outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected attempt");
        async move { next }
    })
    .await;
    let calls = *calls.lock().unwrap();
    (result, calls, started.elapsed())
}

#[tokio::test(start_paused = true)]
async fn first_success_runs_once_without_waiting() {
    let (result, calls, waited) = run(vec![Ok(())]).await;
    assert!(result.is_ok());
    assert_eq!((calls, waited), (1, Duration::ZERO));
}

#[tokio::test(start_paused = true)]
async fn transient_unavailability_is_retried_before_serving() {
    let (result, calls, waited) = run(vec![Err(unavailable()), Err(unavailable()), Ok(())]).await;
    assert!(result.is_ok());
    assert_eq!((calls, waited), (3, Duration::from_secs(20)));
}

#[tokio::test(start_paused = true)]
async fn persistent_unavailability_still_ends_startup_after_the_last_attempt() {
    let (result, calls, waited) = run(vec![
        Err(unavailable()),
        Err(unavailable()),
        Err(unavailable()),
    ])
    .await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    assert_eq!((calls, waited), (3, Duration::from_secs(20)));
}

#[tokio::test(start_paused = true)]
async fn non_transient_errors_stay_fatal_without_a_retry() {
    for error in [SandboxStoreError::Conflict, SandboxStoreError::NotFound] {
        let (result, calls, waited) = run(vec![Err(error)]).await;
        assert!(matches!(
            result,
            Err(SandboxStoreError::Conflict | SandboxStoreError::NotFound)
        ));
        assert_eq!((calls, waited), (1, Duration::ZERO));
    }
    let (result, calls, waited) =
        run(vec![Err(unavailable()), Err(SandboxStoreError::Conflict)]).await;
    assert!(matches!(result, Err(SandboxStoreError::Conflict)));
    assert_eq!((calls, waited), (2, Duration::from_secs(5)));
}
