use super::*;
use crate::store::sandbox_mongo_integration_tests::sandbox;
use layer2_core::sandbox::{SandboxStatus, SandboxWorkState};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn terminal_purge_retries_aborted_authority_fence_and_removes_hierarchy_once() {
    for scoped in [false, true] {
        let f = Fixture::new().await;
        let mut parent = sandbox("sbx_purge_fence");
        parent.status = SandboxStatus::Retired;
        parent.credential.delivered = false;
        parent.credential.current = None;
        parent.work = SandboxWorkState::new(SandboxWorkStep::Purge, MINUTE.into());
        parent.retired_at = Some(MINUTE.into());
        parent.purge_after = Some(MINUTE.into());
        f.independent.create_sandbox(parent.clone()).await.unwrap();
        f.independent
            .workspaces
            .insert_one(doc! {"_id":"wks_purge", "sandbox_id":parent.id.as_str()})
            .await
            .unwrap();
        f.independent
            .checkpoints
            .insert_one(doc! {"_id":"checkpoint_purge", "sandbox_id":parent.id.as_str()})
            .await
            .unwrap();
        let claimed = f
            .independent
            .claim_sandbox_work(
                &parent.id,
                "purge-worker",
                MINUTE.into(),
                work_lease_deadline(),
            )
            .await
            .unwrap()
            .unwrap();
        let result = WorkAuthority::new(
            ClaimedWork::Sandbox(Box::new(claimed.clone())),
            Instant::now(),
        )
        .run(
            async {
                f.fail("update", 1, false).await;
                let work = f.store.purge_sandbox(&claimed.id, claimed.revision, MINUTE);
                if scoped {
                    super::super::scope(&f.store, work).await
                } else {
                    work.await
                }
            },
            || async {
                f.independent
                    .renew_sandbox_authority(&claimed, &now(), &work_lease_deadline())
                    .await
            },
        )
        .await;
        f.clear().await;
        let starts = f.starts();
        let stored = f.independent.get_sandbox(&parent.id).await.unwrap();
        let workspaces = f
            .independent
            .workspaces
            .count_documents(doc! {"sandbox_id":parent.id.as_str()})
            .await
            .unwrap();
        let checkpoints = f
            .independent
            .checkpoints
            .count_documents(doc! {"sandbox_id":parent.id.as_str()})
            .await
            .unwrap();
        f.drop_database().await;
        result.expect("purge must retry known-aborted authority writes, not report lost custody");
        assert_fresh_transactions(&starts, 2);
        assert!(stored.is_none());
        assert_eq!((workspaces, checkpoints), (0, 0));
    }
}
