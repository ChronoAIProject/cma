use super::wait_for_desktop_ready;
use std::time::Duration;
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn desktop_ready_after_old_deadline_succeeds() {
    let started = Instant::now();
    wait_for_desktop_ready(true, tokio::time::sleep(Duration::from_secs(60)))
        .await
        .unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(60));
}

#[tokio::test(start_paused = true)]
async fn desktop_never_ready_fails_at_ninety_seconds() {
    let started = Instant::now();
    let error = wait_for_desktop_ready(true, std::future::pending())
        .await
        .unwrap_err();
    assert_eq!(started.elapsed(), Duration::from_secs(90));
    assert_eq!(error.code, "desktop_start_failed");
    assert_eq!(error.detail, "desktop did not become ready");
    assert!(error.retryable);
}

#[tokio::test(start_paused = true)]
async fn desktop_already_ready_does_not_wait() {
    let started = Instant::now();
    wait_for_desktop_ready(true, std::future::ready(()))
        .await
        .unwrap();
    assert_eq!(started.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn desktop_disabled_does_not_poll_readiness() {
    let started = Instant::now();
    wait_for_desktop_ready(false, async {
        panic!("desktop readiness must not be polled without a machine");
    })
    .await
    .unwrap();
    assert_eq!(started.elapsed(), Duration::ZERO);
}
