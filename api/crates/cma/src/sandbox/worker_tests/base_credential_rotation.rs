use super::*;

#[tokio::test]
async fn rotation_runs_at_twelve_hours_and_revokes_only_after_delivery() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let first_active = harness.drive_active(&created.id, now).await;
    let first = first_active.credential.current.clone().unwrap();
    assert_eq!(
        first_active.work.ready_at,
        rfc3339_at(stamp_time(&first.issued_at) + CREDENTIAL_ROTATION_AGE)
    );
    let rotation_at = stamp_time(&first_active.work.ready_at);

    harness.run(&created.id, rotation_at).await;
    harness.run(&created.id, rotation_at).await;
    harness.run(&created.id, rotation_at).await;
    let delivered = harness.stored(&created.id).await;
    assert_eq!(
        delivered.work.step,
        SandboxWorkStep::RevokeRotatedCredential
    );
    assert_eq!(delivered.credential.current.as_ref().unwrap().generation, 2);
    assert_eq!(
        delivered.credential.revoke_debt[0].provider_key_id,
        first.provider_key_id
    );
    harness.run(&created.id, rotation_at).await;
    harness.run(&created.id, rotation_at).await;
    let active = harness.stored(&created.id).await;

    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.credential.current.as_ref().unwrap().generation, 2);
    assert!(active.credential.revoke_debt.is_empty());
    assert_eq!(
        harness.actions.snapshot(),
        ["mint:1", "mint:2", "revoke:provider-key-1"]
    );
    assert_eq!(
        harness
            .backend
            .requests()
            .await
            .iter()
            .filter(|(_, request)| matches!(request, RequestFrame::Credential(_)))
            .count(),
        2
    );
}

#[tokio::test]
async fn expiry_margin_controls_delivery_and_the_earlier_rotation_deadline() {
    let too_short = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    too_short.credentials.expire_after(CREDENTIAL_EXPIRY_MARGIN);
    let created = too_short.create().await;
    let now = start_time(&created);
    too_short.run(&created.id, now).await;
    too_short.run(&created.id, now).await;
    too_short.run(&created.id, now).await;
    let failed = too_short.stored(&created.id).await;
    assert_eq!(failed.status, SandboxStatus::Failed);
    assert_eq!(failed.failure.as_ref().unwrap().class, "terminal_security");
    assert!(too_short
        .backend
        .requests()
        .await
        .iter()
        .all(|(_, request)| !matches!(request, RequestFrame::Credential(_))));

    let early = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    early.credentials.expire_after(Duration::from_secs(60 * 60));
    let created = early.create().await;
    let active = early.drive_active(&created.id, start_time(&created)).await;
    let authority = active.credential.current.as_ref().unwrap();
    assert_eq!(
        active.work.ready_at,
        rfc3339_at(stamp_time(&authority.expires_at) - CREDENTIAL_EXPIRY_MARGIN)
    );
}
