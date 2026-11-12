use super::*;
use crate::store::sandbox_mongo::is_transient;

#[tokio::test]
#[ignore = "requires an owned disposable replica set and recovery hooks"]
async fn duplicate_unpublished_admissions_keep_one_success_and_one_conflict() {
    for _ in 0..8 {
        let (client, name, store) = database().await;
        let request = Recovery::fixture(&store, false).await;
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        let fence = maintenance::fence_value(&store, request.parent()).await;
        let (a, b) = tokio::join!(request.apply(&store), request.apply(&other));
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let error = a.err().or_else(|| b.err()).unwrap();
        let kind = match &error {
            SandboxStoreError::Conflict => "conflict",
            SandboxStoreError::Unavailable(detail) if detail.contains("exhausted retries") => {
                "retry_exhausted"
            }
            SandboxStoreError::Unavailable(_) => "unavailable",
            SandboxStoreError::Corrupt(_) => "corrupt",
            _ => "other",
        };
        assert!(matches!(error, SandboxStoreError::Conflict), "loser={kind}");
        assert_eq!(
            maintenance::fence_value(&store, request.parent()).await,
            fence + 1
        );
        assert_eq!(
            store
                .sandbox_events(&request.parent().id, 0, 100)
                .await
                .unwrap()
                .len(),
            1
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and recovery hooks"]
async fn read_check_holds_no_write_lock_and_cannot_authorize_a_late_recovery() {
    for v1 in [false, true] {
        let (client, name, store) = database().await;
        let request = reopen_fixture(&store, "sbx_recovery_check").await;
        let parent = &request.snapshot.retained.sandbox;
        let before = support::all_rows(&store).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        crate::store::ledger_maintenance_recovery::check(&store, &parent.id, &mut session)
            .await
            .unwrap();
        assert_eq!(support::all_rows(&store).await, before);
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            maintenance::hold(&other, parent, v1),
        )
        .await
        .expect("read-only check must not block maintenance admission");
        let held = support::all_rows(&store).await;
        assert!(
            matches!(transaction::apply(&store, &request, &mut session).await,
            Err(ApplyError::Driver(error)) if is_transient(&error))
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.admit_retained_reopen(request).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(support::all_rows(&store).await, held);
        client.database(&name).drop().await.unwrap();
    }
}
