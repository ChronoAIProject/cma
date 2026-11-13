//! Exact receipt recovery is not a new platform commitment.
use super::*;
use layer2_core::budget::{BudgetError, Closure, ClosureReason};

async fn revive(
    f: &Fixture,
    key: &str,
    revision: u64,
) -> Result<SandboxControlRecord, SandboxControlServiceError> {
    f.service
        .admit(
            &context("recipient"),
            "fixture",
            SandboxControlSubmission {
                sandbox_id: f.parent.id.clone(),
                command: SandboxControl::Revive,
                expected_revision: revision,
                idempotency_key: key.into(),
            },
        )
        .await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn platform_stop_preserves_exact_revive_receipts_and_refuses_new_or_changed_keys() {
    let (client, name) = database("platform_control_replay").await;
    let f = Fixture::new(&client, &name).await;
    super::super::budget_tests::configure_metered(&f.store, &f.parent.owner).await;
    f.service.attach_platform_spend(f.store.clone());
    let admitted = revive(&f, "retained", f.parent.revision).await.unwrap();
    let (start, _) = super::super::budget_tests::open_platform()
        .period_bounds()
        .unwrap();
    let closure = Closure {
        reason: ClosureReason::SpendStop,
        since: start,
        settings_revision: 1,
        committed_at_entry: 0,
    };
    f.store
        .model_budget
        .update_one(
            doc! {"_id": crate::store::budget::period_key(start)},
            doc! {"$push": {"record.closures": mongodb::bson::serialize_to_bson(&closure).unwrap()},
            "$inc": {"record.revision": 1_i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        revive(&f, "retained", f.parent.revision).await.unwrap(),
        admitted
    );
    assert!(matches!(
        revive(&f, "new", f.parent.revision).await,
        Err(SandboxControlServiceError::Budget(
            BudgetError::PlatformExhausted
        ))
    ));
    assert!(matches!(
        revive(&f, "retained", f.parent.revision + 1).await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::IdempotencyConflict
        ))
    ));
    assert_eq!(
        f.store
            .sandbox_controls
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.store.get_sandbox(&f.parent.id).await.unwrap().unwrap(),
        f.parent
    );
    client.database(&name).drop().await.unwrap();
}
