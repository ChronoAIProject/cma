use super::*;

#[tokio::test]
async fn discovery_waits_for_previous_lease_and_open_rechecks_atomically() {
    let leases = DesktopLeases::default();
    let previous = leases.acquire("sandbox").await.unwrap();
    assert!(matches!(
        leases.acquire("sandbox").await,
        Err(DesktopError::Busy)
    ));
    // Different Sandboxes remain independent while this stream is closing.
    let other = leases.acquire("other").await.unwrap();
    drop(previous);
    // Discovery only probes; another viewer may win before our stream upgrade.
    drop(leases.acquire("sandbox").await.unwrap());
    let winner = leases.acquire("sandbox").await.unwrap();
    assert!(matches!(
        leases.acquire("sandbox").await,
        Err(DesktopError::Busy)
    ));
    drop(winner);
    drop(other);
    let successor = leases.acquire("sandbox").await.unwrap();
    assert_eq!(leases.0.lock().await.len(), 1);
    drop(successor);
}
