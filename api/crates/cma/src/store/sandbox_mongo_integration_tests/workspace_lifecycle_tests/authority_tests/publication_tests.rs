use super::*;
use layer2_core::sandbox::store::{ClaimedWork, WorkAuthority, WorkLeaseGrant};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_claim_identity_cannot_authorize_renewal_or_publication() {
    let (client, name) = database("work_scalar_identity").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let independent = peer(&name).await;
    seed(&store).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        kind.claim(&store, "original", at(0), at(15))
            .await
            .unwrap()
            .unwrap();
        let claim = captured(&store, kind).await;
        WorkAuthority::new(claim.clone(), tokio::time::Instant::now())
            .run(
                async {
                    let original = kind.row(&independent).await;
                    let mut invalid = vec![
                        doc! {"record.work.lease.holder":["original"]},
                        doc! {"record.work.lease.fence":[1_i64]},
                        doc! {"record.work.lease.fence":1.0_f64},
                    ];
                    invalid.push(match &claim {
                        ClaimedWork::Sandbox(_) => doc! {"record.lifecycle.operation":{
                        "authority":{"operation_id":[Bson::Null]}}},
                        ClaimedWork::Workspace(row) => {
                            doc! {"sandbox_id":[row.sandbox_id.as_str()],
                            "record.sandbox_id":[row.sandbox_id.as_str()]}
                        }
                    });
                    for fields in invalid {
                        kind.set(&independent, fields).await;
                        let malformed = kind.row(&independent).await;
                        assert!(renew(&store, &claim).await?.is_none());
                        assert!(save(&store, &claim).await.is_err());
                        assert_eq!(kind.row(&independent).await, malformed);
                        kind.rows(&independent)
                            .replace_one(doc! {"_id":kind.id()}, original.clone())
                            .await
                            .unwrap();
                    }
                    Ok(())
                },
                || renew(&store, &claim),
            )
            .await
            .unwrap();
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn restore_cleanup_accepts_renewed_snapshot_but_refuses_expired_claim() {
    use layer2_core::sandbox::store::SandboxLifecycleExecutionStore;
    let (client, name) = database("work_cleanup_snapshot").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let independent = peer(&name).await;
    seed(&store).await;
    let claimed = store
        .claim_sandbox_work(
            &SandboxId(Kind::Sandbox.id().into()),
            "original",
            at(0),
            at(5),
        )
        .await
        .unwrap()
        .unwrap();
    let original = ClaimedWork::Sandbox(Box::new(claimed.clone()));
    WorkAuthority::new(original.clone(), tokio::time::Instant::now())
        .run(
            async {
                let plan = store.retired_runtime_cleanup_plan(&claimed).await?;
                assert!(plan.agents.is_empty() && plan.workspaces.is_empty());
                Kind::Sandbox
                    .set(
                        &independent,
                        doc! {"work_lease_expires_at":at(-1),
                        "record.work.lease.expires_at":at(-1)},
                    )
                    .await;
                let before = Kind::Sandbox.row(&independent).await;
                assert!(store.retired_runtime_cleanup_plan(&claimed).await.is_err());
                assert_eq!(Kind::Sandbox.row(&independent).await, before);
                Ok(())
            },
            || renew(&independent, &original),
        )
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

async fn captured(store: &MongoSandboxStore, kind: Kind) -> ClaimedWork {
    match kind {
        Kind::Sandbox => ClaimedWork::Sandbox(Box::new(
            store
                .get_sandbox(&SandboxId(kind.id().into()))
                .await
                .unwrap()
                .unwrap(),
        )),
        Kind::Workspace => ClaimedWork::Workspace(Box::new(
            store
                .get_workspace(&WorkspaceId(kind.id().into()))
                .await
                .unwrap()
                .unwrap(),
        )),
    }
}

async fn renew(
    store: &MongoSandboxStore,
    claim: &ClaimedWork,
) -> Result<Option<WorkLeaseGrant>, SandboxStoreError> {
    match claim {
        ClaimedWork::Sandbox(row) => store.renew_sandbox_authority(row, &at(0), &at(15)).await,
        ClaimedWork::Workspace(row) => store.renew_workspace_authority(row, &at(0), &at(15)).await,
    }
}

async fn save(store: &MongoSandboxStore, claim: &ClaimedWork) -> Result<(), SandboxStoreError> {
    match claim {
        ClaimedWork::Sandbox(row) => {
            let mut next = row.as_ref().clone();
            next.name = "authorized-publication".into();
            next.revision += 1;
            store.save_sandbox(row.revision, next).await
        }
        ClaimedWork::Workspace(row) => {
            let mut next = row.as_ref().clone();
            next.name = "authorized-publication".into();
            next.revision += 1;
            store.save_workspace(row.revision, next).await
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn worker_publication_preserves_current_renewal_and_rejects_server_expiry() {
    let (client, name) = database("work_publication_expiry").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let independent = peer(&name).await;
    seed(&first).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        kind.claim(&first, "original", at(0), at(5))
            .await
            .unwrap()
            .unwrap();
        let original = captured(&first, kind).await;
        let authority = WorkAuthority::new(original.clone(), tokio::time::Instant::now());
        authority.run(async {
            let renewed = kind.row(&independent).await;
            save(&first, &original).await?;
            let published = kind.row(&independent).await;
            assert_eq!(published.get_str("work_lease_expires_at").unwrap(), renewed.get_str("work_lease_expires_at").unwrap());
            let next = captured(&first, kind).await;
            let expired = at(-1);
            kind.set(&independent, doc! {"work_lease_expires_at":&expired,"record.work.lease.expires_at":&expired}).await;
            let before = kind.row(&independent).await;
            assert_eq!(save(&first, &next).await, Err(SandboxStoreError::Conflict));
            assert_eq!(kind.row(&independent).await, before);
            Ok(())
        }, || renew(&first, &original)).await.unwrap();
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn predecessor_cannot_publish_successor_snapshot_under_original_scope() {
    let (client, name) = database("work_publication_takeover").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let successor = peer(&name).await;
    seed(&first).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        kind.claim(&first, "original", at(0), at(5))
            .await
            .unwrap()
            .unwrap();
        let original = captured(&first, kind).await;
        let authority = WorkAuthority::new(original.clone(), tokio::time::Instant::now());
        authority.run(async {
            let expired = at(-1);
            kind.set(&successor, doc! {"work_lease_expires_at":&expired,"record.work.lease.expires_at":&expired}).await;
            let other = successor.clone();
            assert_eq!(tokio::spawn(async move {
                kind.claim(&other, "successor", at(0), at(15)).await
            }).await.unwrap()?, Some(2));
            let new_claim = captured(&successor, kind).await;
            let before = kind.row(&successor).await;
            // Even supplying the successor's revision/lease must not replace
            // the original claim retained by the awaiting predecessor.
            assert_eq!(save(&first, &new_claim).await, Err(SandboxStoreError::Conflict));
            assert_eq!(kind.row(&successor).await, before);
            Ok(())
        }, || renew(&first, &original)).await.unwrap();
        let next = captured(&successor, kind).await;
        WorkAuthority::new(next.clone(), tokio::time::Instant::now())
            .run(save(&successor, &next), || renew(&successor, &next))
            .await
            .unwrap();
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_policy_publication_refuses_changed_parent_without_child_mutation() {
    let (client, name) = database("work_publication_parent").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let independent = peer(&name).await;
    seed(&first).await;
    let kind = Kind::Workspace;
    kind.claim(&first, "original", at(0), at(10))
        .await
        .unwrap()
        .unwrap();
    let original = captured(&first, kind).await;
    WorkAuthority::new(original.clone(), tokio::time::Instant::now())
        .run(
            async {
                independent
                    .sandboxes
                    .update_one(
                        doc! {"_id":Kind::Sandbox.id()},
                        doc! {
                            "$set":{"owner":"replacement","record.owner":"replacement"},
                            "$inc":{"revision":1_i64,"record.revision":1_i64}
                        },
                    )
                    .await
                    .unwrap();
                let child = kind.row(&independent).await;
                let parent = Kind::Sandbox.row(&independent).await;
                assert_eq!(
                    save(&first, &original).await,
                    Err(SandboxStoreError::Conflict)
                );
                assert_eq!(kind.row(&independent).await, child);
                assert_eq!(Kind::Sandbox.row(&independent).await, parent);
                Ok(())
            },
            || renew(&first, &original),
        )
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_parent_runtime_is_captured_before_effect_and_fenced_at_publication() {
    let (client, name) = database("work_captured_parent").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let independent = peer(&name).await;
    seed(&first).await;
    Kind::Workspace
        .claim(&first, "original", at(0), at(15))
        .await
        .unwrap()
        .unwrap();
    let claim = captured(&first, Kind::Workspace).await;
    let parent = first
        .get_sandbox(&SandboxId(Kind::Sandbox.id().into()))
        .await
        .unwrap()
        .unwrap();
    WorkAuthority::new(claim.clone(), tokio::time::Instant::now())
        .with_parent(parent)
        .unwrap()
        .run(
            async {
                independent
                    .sandboxes
                    .update_one(
                        doc! {"_id":Kind::Sandbox.id()},
                        doc! {"$set":{"record.lifecycle.runtime_generation_high_water":1_i64},
                        "$inc":{"revision":1_i64,"record.revision":1_i64}},
                    )
                    .await
                    .unwrap();
                let parent = Kind::Sandbox.row(&independent).await;
                let child = Kind::Workspace.row(&independent).await;
                assert_eq!(
                    renew(&first, &claim).await,
                    Err(SandboxStoreError::Conflict)
                );
                assert_eq!(save(&first, &claim).await, Err(SandboxStoreError::Conflict));
                assert_eq!(Kind::Sandbox.row(&independent).await, parent);
                assert_eq!(Kind::Workspace.row(&independent).await, child);
                Ok(())
            },
            || renew(&first, &claim),
        )
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn protected_completion_clears_exact_claim_and_appends_one_event() {
    let (client, name) = database("work_protected_completion").await;
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_renewable_work(true);
    seed(&store).await;
    let claimed = store
        .claim_sandbox_work(
            &SandboxId(Kind::Sandbox.id().into()),
            "original",
            at(0),
            at(15),
        )
        .await
        .unwrap()
        .unwrap();
    let authority = WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claimed.clone())),
        tokio::time::Instant::now(),
    );
    crate::store::work_publication::scope(
        &store,
        authority.run(
            async {
                let mut next = claimed.clone();
                let token = next.work.lease.clone().unwrap();
                next.work
                    .advance(
                        &token.holder,
                        token.fence,
                        SandboxWorkStep::WaitForCredentialRotation,
                        at(60),
                    )
                    .unwrap();
                next.revision += 1;
                store
                    .commit(SandboxTransaction::SaveSandboxAndAppend {
                        command_revival: None,
                        released_runtime: None,
                        expected_revision: claimed.revision,
                        sandbox: Box::new(next.clone()),
                        event_at: at(0),
                        event: SandboxEventKind::WorkStepCompleted {
                            step: claimed.work.step,
                        },
                    })
                    .await?;
                assert_eq!(store.get_sandbox(&claimed.id).await?, Some(next));
                assert_eq!(store.sandbox_events(&claimed.id, 0, 10).await?.len(), 1);
                // The now completed claim cannot authorize another mutation, even in
                // the same still locally live execution scope.
                assert!(store
                    .renew_sandbox_authority(&claimed, &at(0), &at(15))
                    .await?
                    .is_none());
                Ok(())
            },
            || async {
                store
                    .renew_sandbox_authority(&claimed, &at(0), &at(15))
                    .await
            },
        ),
    )
    .await
    .unwrap();
    client.database(&name).drop().await.unwrap();
}
