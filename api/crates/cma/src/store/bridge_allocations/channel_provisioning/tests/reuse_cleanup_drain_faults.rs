use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_drain_closure_audit_failure_rolls_back_markers_and_fences() {
    let mut f = CleanupFixture::new(true).await;
    f.reset().await;
    let saved = f.request_cleanup().await.unwrap();
    let target = f.original.placement.target();
    let identities = [
        ("agents", target.agent_id.to_string()),
        ("sandboxes", target.sandbox_id.to_string()),
        ("bridge_allocations", f.f.record.id.clone()),
        (
            "bridge_allocation_heads",
            f.f.record.identity.head_id().unwrap(),
        ),
    ];
    let mut before = Vec::new();
    for (collection, id) in &identities {
        before.push(f.row(collection, id).await);
    }
    f.db().run_command(doc! {"collMod":"channel_reuse_cleanup_admission_audits", "validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert_eq!(
        f.close_admission().await.err(),
        Some(AllocationError::Unavailable)
    );
    for ((collection, id), row) in identities.iter().zip(before) {
        assert_eq!(f.row(collection, id).await, row, "changed {collection}");
    }
    for collection in [
        "channel_reuse_cleanup_admissions",
        "channel_reuse_cleanup_admission_audits",
    ] {
        assert_eq!(
            f.db()
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    assert!(f.request_cleanup().await.unwrap() == saved);
    f.db()
        .run_command(doc! {"collMod":"channel_reuse_cleanup_admission_audits", "validator":{}})
        .await
        .unwrap();
    f.close_admission().await.unwrap();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_drain_closure_conflicts_with_stale_command_admission_snapshot() {
    let mut f = CleanupFixture::new(true).await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    let target = f.original.placement.target();
    let request = f
        .ordinary_request(
            &target.agent_id,
            "stale-admission",
            AgentCommand::message("New work"),
        )
        .await;
    let mut session = f.f.f.original.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    f.db()
        .collection::<Document>("agents")
        .find_one(doc! {"_id":target.agent_id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    f.close_admission().await.unwrap();
    let attempted = crate::store::agent_commands::reserve::apply(
        &f.f.f.original.resources,
        &request,
        &mut session,
    )
    .await;
    assert!(
        matches!(attempted, Err(crate::store::agent_commands::ApplyError::Driver(error)) if crate::store::sandbox_mongo::is_transient(&error))
    );
    session.abort_transaction().await.unwrap();
    assert_eq!(
        f.f.f
            .original
            .resources
            .reserve_command(request)
            .await
            .unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    assert_eq!(
        f.db()
            .collection::<Document>("agent_commands")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert!(f.observe().await.unwrap().holds.is_empty());
    f.close().await;
}
