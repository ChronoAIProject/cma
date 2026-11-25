use super::*;

#[test]
fn committed_content_reconciliation_accepts_only_original_monotonic_renewal() {
    use crate::store::work_lease_update::{sandbox_matches, workspace_matches};
    let mut sandbox = sandbox("sbx_committed_renewal");
    sandbox
        .work
        .claim(
            "original".into(),
            "2026-09-05T00:00:00Z",
            "2026-09-05T00:00:05Z".into(),
        )
        .unwrap();
    let mut workspace = workspace(&sandbox, "wks_committed_renewal", "committed");
    workspace
        .work
        .claim(
            "original".into(),
            "2026-09-05T00:00:00Z",
            "2026-09-05T00:00:05Z".into(),
        )
        .unwrap();
    let mut renewed_sandbox = sandbox.clone();
    let mut renewed_workspace = workspace.clone();
    // Both dates are expired: this checks committed identity, not write authority.
    renewed_sandbox.work.lease.as_mut().unwrap().expires_at = "2026-09-05T00:00:10Z".into();
    renewed_workspace.work.lease.as_mut().unwrap().expires_at = "2026-09-05T00:00:10Z".into();
    assert!(sandbox_matches(Some(&renewed_sandbox), &sandbox));
    assert!(workspace_matches(Some(&renewed_workspace), &workspace));
    assert!(!sandbox_matches(Some(&sandbox), &renewed_sandbox));
    assert!(!workspace_matches(Some(&workspace), &renewed_workspace));
    renewed_sandbox.work.lease.as_mut().unwrap().fence += 1;
    renewed_workspace.work.lease.as_mut().unwrap().holder = "successor".into();
    assert!(!sandbox_matches(Some(&renewed_sandbox), &sandbox));
    assert!(!workspace_matches(Some(&renewed_workspace), &workspace));
    let mut changed = sandbox.clone();
    changed.name = "different-result".into();
    assert!(!sandbox_matches(Some(&changed), &sandbox));
    let mut changed = workspace.clone();
    changed.revision += 1;
    assert!(!workspace_matches(Some(&changed), &workspace));
    let mut malformed = sandbox.clone();
    malformed.work.lease.as_mut().unwrap().expires_at = "zzzz".into();
    assert!(!sandbox_matches(Some(&malformed), &sandbox));
    let mut malformed = workspace.clone();
    malformed.work.lease.as_mut().unwrap().expires_at = "zzzz".into();
    assert!(!workspace_matches(Some(&malformed), &workspace));
    assert!(!sandbox_matches(None, &sandbox));
    assert!(!workspace_matches(None, &workspace));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_content_saves_preserve_independent_renewal_and_complete_normally() {
    let (client, name) = database("work_snapshot_renewal").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = peer(&name).await;
    seed(&first).await;
    let mut sandbox = first
        .claim_sandbox_work(
            &SandboxId(Kind::Sandbox.id().into()),
            "original",
            at(0),
            at(5),
        )
        .await
        .unwrap()
        .unwrap();
    let mut workspace = first
        .claim_workspace_work(
            &WorkspaceId(Kind::Workspace.id().into()),
            "original",
            at(0),
            at(5),
        )
        .await
        .unwrap()
        .unwrap();
    let until = at(12);
    assert_eq!(
        second
            .renew_sandbox_work(&sandbox, &at(0), &until)
            .await
            .unwrap(),
        Some(until.clone())
    );
    assert_eq!(
        second
            .renew_workspace_work(&workspace, &at(0), &until)
            .await
            .unwrap(),
        Some(until.clone())
    );
    let sandbox_revision = sandbox.revision;
    sandbox.name = "renamed-after-renewal".into();
    sandbox.revision += 1;
    first
        .save_sandbox(sandbox_revision, sandbox.clone())
        .await
        .unwrap();
    let workspace_revision = workspace.revision;
    workspace.name = "renamed-workspace".into();
    workspace.revision += 1;
    first
        .save_workspace(workspace_revision, workspace.clone())
        .await
        .unwrap();
    let observed = second.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(observed.work.lease.as_ref().unwrap().expires_at, until);
    assert_eq!(observed.name, sandbox.name);
    assert_eq!(observed.revision, sandbox.revision);
    let observed = second.get_workspace(&workspace.id).await.unwrap().unwrap();
    assert_eq!(observed.work.lease.as_ref().unwrap().expires_at, until);
    assert_eq!(observed.name, workspace.name);
    assert_eq!(observed.revision, workspace.revision);

    // A terminal mutation clears this exact lease; preservation must not retain it.
    let revision = workspace.revision;
    let lease = workspace.work.lease.clone().unwrap();
    workspace
        .work
        .advance(&lease.holder, lease.fence, WorkspaceWorkStep::None, at(0))
        .unwrap();
    let failure = TypedFailure {
        class: "fixture_failed".into(),
        detail: "test completion".into(),
    };
    workspace
        .transition(WorkspaceStatus::Failed, at(0), Some(failure.clone()))
        .unwrap();
    first
        .commit(SandboxTransaction::SaveWorkspaceAndAppend {
            repository_runtime: None,
            expected_revision: revision,
            workspace: Box::new(workspace.clone()),
            event_at: at(0),
            event: SandboxEventKind::WorkspaceFailed {
                workspace_id: workspace.id.clone(),
                failure,
            },
        })
        .await
        .unwrap();
    assert!(second
        .get_workspace(&workspace.id)
        .await
        .unwrap()
        .unwrap()
        .work
        .lease
        .is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn transactional_snapshot_save_preserves_renewal_and_literal_data() {
    let (client, name) = database("work_snapshot_transaction").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = peer(&name).await;
    seed(&first).await;
    let mut sandbox = first
        .claim_sandbox_work(
            &SandboxId(Kind::Sandbox.id().into()),
            "original",
            at(0),
            at(5),
        )
        .await
        .unwrap()
        .unwrap();
    let until = at(12);
    second
        .renew_sandbox_work(&sandbox, &at(0), &until)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.name = "transaction-renamed".into();
    sandbox.revision += 1;
    first
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision: revision,
            sandbox: Box::new(sandbox.clone()),
            event_at: at(0),
            event: SandboxEventKind::WorkStepCompleted {
                step: sandbox.work.step,
            },
        })
        .await
        .unwrap();
    let observed = second.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(observed.work.lease.as_ref().unwrap().expires_at, until);
    assert_eq!(observed.name, sandbox.name);

    let mut fields = crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap();
    let literal = doc! {"value":"$record.owner","nested":{"$add":[1_i32,2_i32]}};
    fields.insert("literal_probe", literal.clone());
    first
        .sandboxes
        .update_one(
            doc! {"_id":sandbox.id.as_str()},
            crate::store::work_lease_update::replacement(fields),
        )
        .await
        .unwrap();
    let row = Kind::Sandbox.row(&second).await;
    assert_eq!(row.get_document("literal_probe").unwrap(), &literal);
    assert_eq!(row.get_str("work_lease_expires_at").unwrap(), until);
    client.database(&name).drop().await.unwrap();
}
