//! A failed cold allocation must settle its original creation operation.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn failed_bootstrap_parent_finishes_queued_creation_without_admitting_an_agent() {
    for status in [
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ] {
        let f = Fixture::new().await;
        let profile = SandboxProfileRef {
            id: f.sandbox.profile.id.clone(),
            revision: f.sandbox.profile.revision,
        };
        // Cold admission may reserve this child before the runtime fails.
        let workspace = f
            .service
            .reserve_bootstrap_workspace(
                &f.owner.caller,
                &f.sandbox.id,
                &profile,
                &f.workspace_profile,
                &f.record.id,
            )
            .await
            .unwrap();
        let child_before = f.store.get_workspace(&workspace).await.unwrap().unwrap();
        let mut parent = f.store.get_sandbox(&f.sandbox.id).await.unwrap().unwrap();
        let revision = parent.revision;
        parent.status = status;
        parent.revision += 1;
        f.store
            .save_sandbox(revision, parent.clone())
            .await
            .unwrap();
        // Resume the real durable worker after its previous claim expired.
        f.db.collection::<Document>("agent_lifecycle_operations")
            .update_one(
                doc! {"_id": &f.record.id},
                doc! {"$set": {"status": "queued", "lease_expires_at": DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
        f.service.worker_slots.add_permits(1);
        f.service.spawn_worker(f.record.id.clone());
        let finished = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let record = f.record().await;
                if record.status == "failed" {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("failed parent remained queued instead of completing creation");
        assert_eq!(finished.sandbox_id.as_deref(), Some(f.sandbox.id.as_str()));
        assert!(finished.agent_id.is_none());
        assert!(
            finished
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Sandbox provisioning failed")),
            "{:?}",
            finished.error
        );
        assert!(finished.lease_token.is_none());
        assert_eq!(
            f.store.get_sandbox(&f.sandbox.id).await.unwrap(),
            Some(parent)
        );
        assert_eq!(
            f.store.get_workspace(&workspace).await.unwrap(),
            Some(child_before)
        );
        assert!(f
            .store
            .list_agents(&f.owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .is_empty());
        f.db.drop().await.unwrap();
    }
}
