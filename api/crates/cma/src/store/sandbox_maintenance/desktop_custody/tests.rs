use super::native_probe::Fixture;
use super::*;
use mongodb::bson::Timestamp;

fn watermark() -> Timestamp {
    Timestamp {
        time: 10,
        increment: 2,
    }
}

#[tokio::test]
async fn missing_commit_watermark_keeps_desktop_closed_without_guessing_a_read() {
    let fixture = Fixture::new().await;
    let registry = Arc::new(Registry::new(1));
    let result = run_with_registry(
        registry.clone(),
        &fixture.id,
        Duration::from_secs(1),
        |mut custody| async move {
            custody.commit_started();
            custody
                .reconcile(None, |_| async {
                    panic!("missing watermark cannot authorize a read")
                })
                .await
        },
    )
    .await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    fixture.assert_blocked().await;
    assert_eq!(registry.guards.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn receipt_mismatch_keeps_desktop_closed_and_does_not_evict_custody() {
    let fixture = Fixture::new().await;
    let registry = Arc::new(Registry::new(1));
    let result = run_with_registry(
        registry.clone(),
        &fixture.id,
        Duration::from_secs(1),
        |mut custody| async move {
            custody.commit_started();
            custody
                .reconcile(Some(watermark()), |actual| async move {
                    assert_eq!(actual, watermark());
                    Ok(false)
                })
                .await
        },
    )
    .await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    fixture.assert_blocked().await;
    // Retrying or filling the registry cannot replace this unresolved hold.
    assert!(registry.reserve(&fixture.id).is_err());
    assert!(registry.reserve(&SandboxId::generate()).is_err());
    fixture.assert_blocked().await;
}

#[tokio::test]
async fn reconciliation_read_error_keeps_desktop_closed() {
    let fixture = Fixture::new().await;
    let registry = Arc::new(Registry::new(1));
    let result = run_with_registry(
        registry.clone(),
        &fixture.id,
        Duration::from_secs(1),
        |mut custody| async move {
            custody.commit_started();
            custody
                .reconcile(Some(watermark()), |_| async {
                    Err(SandboxStoreError::Unavailable(
                        "controlled snapshot read failure".into(),
                    ))
                })
                .await
        },
    )
    .await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    fixture.assert_blocked().await;
    assert_eq!(registry.guards.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn panic_during_commit_retains_the_registry_owned_guard() {
    let fixture = Fixture::new().await;
    let registry = Arc::new(Registry::new(1));
    let result: Result<(), _> = run_with_registry(
        registry.clone(),
        &fixture.id,
        Duration::from_secs(1),
        |mut custody| async move {
            custody.commit_started();
            panic!("controlled panic after commit dispatch");
        },
    )
    .await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    fixture.assert_blocked().await;
    assert_eq!(registry.guards.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn proved_commit_releases_desktop_only_after_receipt_confirmation() {
    let fixture = Fixture::new().await;
    let registry = Arc::new(Registry::new(1));
    run_with_registry(
        registry.clone(),
        &fixture.id,
        Duration::from_secs(1),
        |mut custody| async move {
            custody.commit_started();
            custody
                .reconcile(Some(watermark()), |actual| async move {
                    assert_eq!(actual, watermark());
                    Ok(true)
                })
                .await
        },
    )
    .await
    .unwrap();
    assert!(registry.guards.lock().unwrap().is_empty());
    fixture.assert_native_open().await;
}

#[tokio::test]
async fn definite_abort_and_precommit_refusal_release_without_an_unknown_hold() {
    let fixture = Fixture::new().await;
    let registry = Arc::new(Registry::new(1));
    for dispatched in [false, true] {
        let result: Result<(), _> = run_with_registry(
            registry.clone(),
            &fixture.id,
            Duration::from_secs(1),
            move |mut custody| async move {
                if dispatched {
                    custody.commit_started();
                    custody.confirmed(); // Shared commit classifier proved abort.
                }
                Err(SandboxStoreError::Conflict)
            },
        )
        .await;
        assert!(matches!(result, Err(SandboxStoreError::Conflict)));
        assert!(registry.guards.lock().unwrap().is_empty());
        fixture.assert_native_open().await;
    }
}

#[tokio::test]
async fn claimed_work_scope_is_refused_before_authority_can_be_lost_at_spawn() {
    use layer2_core::sandbox::store::{ClaimedWork, WorkLeaseGrant};

    let fixture = Fixture::new().await;
    let registry = Arc::new(Registry::new(1));
    let authority = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(
            crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
                fixture.id.as_str(),
            ),
        )),
        tokio::time::Instant::now(),
    );
    let result: Result<(), _> = authority
        .run(
            run_with_registry(
                registry.clone(),
                &fixture.id,
                Duration::from_secs(1),
                |_| async { panic!("claimed-work admission must refuse before effects") },
            ),
            || async { WorkLeaseGrant::new("2026-11-15T00:00:15Z".into(), 15_000).map(Some) },
        )
        .await;
    assert!(matches!(result, Err(SandboxStoreError::Conflict)));
    assert!(registry.guards.lock().unwrap().is_empty());
    fixture.assert_native_open().await;
}
