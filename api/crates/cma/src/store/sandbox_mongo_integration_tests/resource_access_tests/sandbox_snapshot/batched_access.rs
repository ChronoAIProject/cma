use super::{batch_fixture::*, *};
use layer2_core::sandbox::HierarchyReadError;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_hierarchy_preserves_concealment_and_rejects_invalid_lineage() {
    let (admin, name) = database("sandbox_batch_access").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let template = template(&store, &target).await;
    let caller = Caller {
        subject: template.sandbox.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let foreign = Caller {
        subject: SubjectId::new("foreign-fixture"),
        authorization: caller.authorization.clone(),
    };
    for m in 0..3 {
        let service = service(mode(&store, m));
        assert_eq!(
            service.sandbox(&foreign, &template.sandbox.id).await,
            Err(HierarchyReadError::NotFound)
        );
        assert_eq!(
            service
                .sandbox(&caller, &SandboxId("sbx_absent".into()))
                .await,
            Err(HierarchyReadError::NotFound)
        );
    }
    for defect in [
        "foreign-owner",
        "orphan",
        "missing-tree",
        "duplicate-assignment",
    ] {
        let root = seed(&store, &template, defect, 1, 2).await;
        let rows = store
            .read_sandbox_hierarchy(&root.id)
            .await
            .unwrap()
            .unwrap();
        match defect {
            "foreign-owner" => {
                store
                    .workspaces
                    .update_many(
                        doc! {"sandbox_id":root.id.as_str()},
                        doc! {"$set":{"owner":"foreign-fixture","record.owner":"foreign-fixture"}},
                    )
                    .await
                    .unwrap();
            }
            "orphan" => {
                store.worktrees.update_many(doc! {"sandbox_id":root.id.as_str()},
                    doc! {"$set":{"workspace_id":"wks_missing","record.workspace_id":"wks_missing"}}).await.unwrap();
            }
            "missing-tree" => {
                store
                    .worktrees
                    .delete_one(doc! {"_id":rows.worktrees[0].id.as_str()})
                    .await
                    .unwrap();
            }
            "duplicate-assignment" => {
                store
                    .worktrees
                    .update_one(
                        doc! {"_id":rows.worktrees[1].id.as_str()},
                        doc! {"$set":{"record.agent_id":rows.agents[0].id.as_str()}},
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        for m in 0..3 {
            assert!(
                matches!(
                    service(mode(&store, m)).sandbox(&caller, &root.id).await,
                    Err(HierarchyReadError::Corrupt(_))
                ),
                "{defect} in mode {m}"
            );
        }
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}
