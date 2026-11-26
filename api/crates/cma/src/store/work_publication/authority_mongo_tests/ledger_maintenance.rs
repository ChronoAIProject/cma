use super::*;
use crate::store::sandbox_mongo_integration_tests::sandbox;
use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
use mongodb::bson::Bson;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn ledger_maintenance_holds_sandbox_work_across_clients() {
    let f = Fixture::new().await;
    let (claimed, agents, mutation) = f.phase().await;
    let sibling = sandbox("sbx_ledger_maintenance_sibling");
    f.independent.create_sandbox(sibling.clone()).await.unwrap();
    let listed_at = work_lease_deadline();
    let due = f.store.due_sandbox_work(&listed_at, 10).await.unwrap();
    assert!(due.contains(&claimed.id));
    assert!(due.contains(&sibling.id));

    WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        Instant::now(),
    )
    .run(
        async {
            check_held_worker(&f, &claimed, &agents, &mutation, &sibling, &listed_at).await;
            Ok(())
        },
        || async {
            f.independent
                .renew_sandbox_authority(&claimed, &now(), &work_lease_deadline())
                .await
        },
    )
    .await
    .unwrap();
    assert!(f
        .store
        .claim_sandbox_work(&sibling.id, "successor", now(), work_lease_deadline())
        .await
        .unwrap()
        .is_some());
    f.drop_database().await;
}

async fn check_held_worker(
    f: &Fixture,
    claimed: &Sandbox,
    agents: &[Agent],
    mutation: &SandboxLifecycleMutation,
    sibling: &Sandbox,
    listed_at: &str,
) {
    let before = f.observations(claimed, agents).await;
    // Presence also holds an unknown or malformed record. An ordinary worker
    // must not interpret it as permission to recover the Sandbox.
    for hold in [Bson::Document(doc! {"phase": "requested"}), Bson::Null] {
        let mut fields = Document::new();
        fields.insert(LEDGER_MAINTENANCE_FIELD, hold.clone());
        let updated = f
            .independent
            .sandboxes
            .update_one(doc! {"_id":claimed.id.as_str()}, doc! {"$set":fields})
            .await
            .unwrap();
        assert_eq!(updated.matched_count, 1);
        let due = f.store.due_sandbox_work(listed_at, 10).await.unwrap();
        assert_eq!(due, vec![sibling.id.clone()]);
        assert!(f
            .store
            .claim_sandbox_work(
                &claimed.id,
                "successor",
                listed_at.into(),
                work_lease_deadline()
            )
            .await
            .unwrap()
            .is_none());
        assert!(f
            .store
            .renew_sandbox_authority(claimed, &now(), &work_lease_deadline())
            .await
            .unwrap()
            .is_none());

        assert!(matches!(
            crate::store::work_publication::scope(
                &f.store,
                f.store.commit_sandbox_lifecycle(mutation.clone()),
            )
            .await,
            Err(SandboxStoreError::Unavailable(_)),
        ));
        assert_eq!(
            f.store.commit_sandbox_lifecycle(mutation.clone()).await,
            Err(SandboxStoreError::Conflict),
        );
        let persisted = f
            .store
            .sandboxes
            .find_one(doc! {"_id":claimed.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.get(LEDGER_MAINTENANCE_FIELD), Some(&hold));
        assert_eq!(f.observations(claimed, agents).await, before);
    }
}
