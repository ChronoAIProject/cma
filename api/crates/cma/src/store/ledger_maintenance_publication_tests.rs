//! Registered beneath prepared_open_ready_tests to reuse the real public port.
use super::*;
use crate::store::ledger_maintenance_test_support as support;
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;

#[path = "ledger_maintenance_publication_test_support.rs"]
mod maintenance;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and publication hooks"]
async fn prepared_publication_succeeds_unheld_and_refuses_both_holds_after_reopen() {
    for hold in [0, 1, 2] {
        let (client, name, store, agent, observed) = fixture().await;
        let done = completion(&agent, &observed.worktree);
        let fence = maintenance::fence_value(&store, &observed.sandbox).await;
        if hold != 0 {
            maintenance::hold(&store, &observed.sandbox, hold == 1).await;
        }
        let before = support::all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let result = reopened
            .commit_prepared_agent_open_ready(done.clone(), observed.clone())
            .await;
        if hold == 0 {
            assert!(result.unwrap());
            assert!(reopened.get_agent(&agent.id).await.unwrap().unwrap() == done.agent);
            assert_ne!(support::all_rows(&store).await, before);
            assert_eq!(
                maintenance::fence_value(&store, &observed.sandbox).await,
                fence + 1
            );
        } else {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(support::all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and publication hooks"]
async fn prepared_publication_refuses_a_hold_with_missing_audit_without_any_writes() {
    for v1 in [false, true] {
        let (client, name, store, agent, observed) = fixture().await;
        maintenance::hold(&store, &observed.sandbox, v1).await;
        let audit = if v1 {
            crate::store::ledger_maintenance_transaction::audit_collection(&store)
        } else {
            crate::store::ledger_maintenance_pre_exclusion_transaction::audit_collection(&store)
        };
        audit.delete_many(doc! {}).await.unwrap();
        let before = support::all_rows(&store).await;
        assert!(matches!(
            store
                .commit_prepared_agent_open_ready(completion(&agent, &observed.worktree), observed)
                .await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and publication hooks"]
async fn publication_refuses_missing_or_foreign_parent_custody_without_writes() {
    for missing in [false, true] {
        let (client, name, store, agent, observed) = fixture().await;
        let id = observed.sandbox.id.as_str();
        if missing {
            store.sandboxes.delete_one(doc! {"_id":id}).await.unwrap();
        } else {
            let mut changed = observed.sandbox.clone();
            changed.owner = layer2_core::SubjectId::new("foreign-fixture");
            store
                .sandboxes
                .replace_one(
                    doc! {"_id":id},
                    crate::store::sandboxes::sandbox_document(&changed, 0).unwrap(),
                )
                .await
                .unwrap();
        }
        let before = support::all_rows(&store).await;
        assert_eq!(
            store
                .commit_prepared_agent_open_ready(completion(&agent, &observed.worktree), observed)
                .await,
            Err(if missing {
                SandboxStoreError::NotFound
            } else {
                SandboxStoreError::Conflict
            })
        );
        assert_eq!(support::all_rows(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and publication hooks"]
async fn invalid_publication_rolls_back_parent_fence_and_a_valid_retry_succeeds() {
    let (client, name, store, agent, observed) = fixture().await;
    let mut invalid = completion(&agent, &observed.worktree);
    invalid.fence += 1;
    let before = support::all_rows(&store).await;
    assert_eq!(
        store
            .commit_prepared_agent_open_ready(invalid, observed.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(support::all_rows(&store).await, before);
    assert!(store
        .commit_prepared_agent_open_ready(completion(&agent, &observed.worktree), observed)
        .await
        .unwrap());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and publication hooks"]
async fn hold_commit_invalidates_an_open_publication_snapshot_and_fresh_publication() {
    for v1 in [false, true] {
        let (client, name, store, agent, observed) = fixture().await;
        let done = completion(&agent, &observed.worktree);
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        maintenance::hold(&other, &observed.sandbox, v1).await;
        let held = support::all_rows(&store).await;
        assert!(matches!(
            apply_publication_observed(&store, &done, Some(&observed), &mut session).await,
            Err(ApplyError::Driver(error)) if is_transient(&error)
        ));
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.commit_prepared_agent_open_ready(done, observed).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, held);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and publication hooks"]
async fn publication_commit_invalidates_an_open_hold_snapshot() {
    use crate::store::ledger_maintenance::ApplyError as HoldError;
    use crate::store::ledger_maintenance_pre_exclusion::Change;
    use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
    let (client, name, store, agent, observed) = fixture().await;
    let (policy, input) = support::pre_exclusion(&store, &observed.sandbox, support::NOW).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    crate::store::ledger_maintenance_snapshot::load_pre_exclusion(
        &store,
        &input.caller,
        &observed.sandbox.id,
        &mut session,
    )
    .await
    .unwrap_or_else(|_| panic!("fixture snapshot failed"));
    let other = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(other
        .commit_prepared_agent_open_ready(completion(&agent, &observed.worktree), observed)
        .await
        .unwrap());
    let published = support::all_rows(&store).await;
    assert!(
        matches!(crate::store::ledger_maintenance_pre_exclusion_transaction::apply(
        &store, &policy, &Change::Admit(input.clone()), &mut session).await,
        Err(HoldError::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(
        store.admit_pre_exclusion(policy, input).await,
        Err(LedgerMaintenanceStoreError::Refused(
            LedgerMaintenanceAdmissionRefusal::Conflict
        ))
    );
    assert_eq!(support::all_rows(&store).await, published);
    client.database(&name).drop().await.unwrap();
}
