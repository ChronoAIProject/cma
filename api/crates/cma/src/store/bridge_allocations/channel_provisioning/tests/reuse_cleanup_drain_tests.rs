use super::*;

#[path = "reuse_cleanup_drain_effects.rs"]
mod effects;
#[path = "reuse_cleanup_drain_faults.rs"]
mod faults;
#[path = "reuse_cleanup_drain_fixture.rs"]
mod fixture;
#[path = "reuse_cleanup_trigger_tests.rs"]
mod trigger;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_drain_closes_exact_child_and_preserves_replay_and_siblings() {
    let mut f = CleanupFixture::new(true).await;
    f.reset().await;
    assert!(
        f.close_admission().await.is_err(),
        "stage A must already exist"
    );
    let original = f.request_cleanup().await.unwrap();
    assert!(
        f.observe().await.is_err(),
        "observation must not silently close admission"
    );
    let target = f.original.placement.target().clone();
    let existing = f
        .ordinary_request(
            &target.agent_id,
            "before-close",
            AgentCommand::message("Accepted work"),
        )
        .await;
    let resources = &f.f.f.original.resources;
    let accepted = resources.reserve_command(existing.clone()).await.unwrap();
    let before_parent = f.f.sandbox().await;
    let before_sibling = f.row("agents", f.f.f.sibling.id.as_str()).await;
    let before_runtime = f.f.f.original.backend.requests().await;
    let (one, two) = tokio::join!(f.close_admission(), f.close_admission());
    let one = one.unwrap();
    assert!(one == two.unwrap());
    assert!(f.request_cleanup().await.unwrap() == original);
    assert!(matches!(
        resources.reserve_command(existing).await.unwrap(),
        CommandReservationResult::Existing(_)
    ));
    let CommandReservationResult::Created(accepted) = accepted else {
        panic!()
    };
    assert_eq!(
        resources
            .get_command(&accepted.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(*accepted)
    );
    let next = f
        .ordinary_request(
            &target.agent_id,
            "after-close",
            AgentCommand::message("New work"),
        )
        .await;
    assert_eq!(
        resources.reserve_command(next).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    assert!(f
        .operations()
        .send_message(
            &caller().caller,
            &target.agent_id,
            "Legacy new input".into()
        )
        .await
        .is_err());
    assert_eq!(f.f.sandbox().await, before_parent);
    assert_eq!(
        f.row("agents", f.f.f.sibling.id.as_str()).await,
        before_sibling
    );
    assert_eq!(f.f.f.original.backend.requests().await, before_runtime);
    let sibling = f
        .ordinary_request(
            &f.f.f.sibling.id,
            "ordinary-sibling",
            AgentCommand::message("Unrelated work"),
        )
        .await;
    assert!(matches!(
        resources.reserve_command(sibling).await.unwrap(),
        CommandReservationResult::Created(_)
    ));
    let observation = f.observe().await.unwrap();
    assert!(observation.admission == one);
    assert!(observation
        .holds
        .contains(&ChannelReuseCleanupHold::PendingCommand));
    assert_eq!(observation.runtime, before_parent.runtime.unwrap());
    assert_eq!(observation.sandbox_revision, before_parent.revision);
    assert!(observation.workspace_revision > 0 && observation.agent_revision > 0);
    assert!(observation.observed_at_ms >= one.closed_at_ms);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_drain_reopened_store_rechecks_audit_and_current_lifecycle() {
    let mut f = CleanupFixture::new(false).await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    let closed = f.close_admission().await.unwrap();
    let quiet = f.observe().await.unwrap();
    assert!(quiet.holds.is_empty());
    let original = &f.f.f.original;
    let invokes = Arc::new(
        crate::store::MongoBridgeInvokeStore::open(
            original.bridges.clone(),
            original.resources.clone(),
            &original.database,
        )
        .await
        .unwrap(),
    );
    let reopened = crate::store::MongoBridgeAllocationStore::open(
        original.bridges.clone(),
        Some(invokes),
        &original.database,
    )
    .await
    .unwrap();
    assert!(
        reopened
            .close_channel_reuse_cleanup_admission(&f.permit().await, &f.request())
            .await
            .unwrap()
            == closed
    );
    let audit = f
        .row("channel_reuse_cleanup_admission_audits", &closed.id)
        .await;
    f.db()
        .collection::<Document>("channel_reuse_cleanup_admission_audits")
        .delete_one(doc! {"_id":&closed.id})
        .await
        .unwrap();
    assert_eq!(f.observe().await.err(), Some(AllocationError::Corrupt));
    f.db()
        .collection::<Document>("channel_reuse_cleanup_admission_audits")
        .insert_one(audit)
        .await
        .unwrap();
    let permit = f.permit().await;
    f.f.pause().await;
    assert!(reopened
        .observe_channel_reuse_cleanup_quiescence(&permit, &f.request())
        .await
        .is_err());
    assert_eq!(f.f.sandbox().await.status, SandboxStatus::Paused);
    assert!(f
        .row("agents", closed.agent_id.as_str())
        .await
        .contains_key(CLEANUP_ADMISSION_FIELD));
    f.close().await;
}
