use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ledger_maintenance_refuses_workspace_creation_removal_and_recovery() {
    use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
    use layer2_core::sandbox::ProvisioningExecution;

    let (client, name) = database("workspace_hold").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (independent, _) = database("workspace_hold_peer").await;
    let parent_rows = independent
        .database(&name)
        .collection::<Document>(SANDBOXES);
    let parent = sandbox("sbx_workspace_hold");
    store.create_sandbox(parent.clone()).await.unwrap();
    let filter = doc! {"_id": parent.id.as_str()};
    let removable = workspace(&parent, "wks_removable", "removable");
    store.create_workspace(removable.clone()).await.unwrap();
    let mut failed = workspace(&parent, "wks_failed", "failed");
    failed.profile.digest = layer2_core::profiles::content_digest(&failed.profile.content).unwrap();
    failed.work.execution = Some(ProvisioningExecution::default());
    store.create_workspace(failed.clone()).await.unwrap();
    let expected = failed.revision;
    failed.work.schedule(WorkspaceWorkStep::None, NOW.into());
    failed
        .transition(
            WorkspaceStatus::Failed,
            NOW.into(),
            Some(TypedFailure {
                class: "prepare_failed".into(),
                detail: "fixture preparation failed".into(),
            }),
        )
        .unwrap();
    store
        .save_workspace(expected, failed.clone())
        .await
        .unwrap();
    let caller = Caller::new(
        parent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let candidate = workspace(&parent, "wks_new", "new-project");
    let admission = workspace_admission(candidate.clone());
    let removable_filter = doc! {"_id": removable.id.as_str()};
    let failed_filter = doc! {"_id": failed.id.as_str()};
    let original_removable = store
        .workspaces
        .find_one(removable_filter.clone())
        .await
        .unwrap()
        .unwrap();
    let original_failed = store
        .workspaces
        .find_one(failed_filter.clone())
        .await
        .unwrap()
        .unwrap();

    for hold in [Bson::Document(doc! {"phase": "requested"}), Bson::Null] {
        parent_rows
            .update_one(
                filter.clone(),
                doc! {"$set": {LEDGER_MAINTENANCE_FIELD: hold.clone()}},
            )
            .await
            .unwrap();
        let held_parent = parent_rows.find_one(filter.clone()).await.unwrap().unwrap();
        assert_eq!(held_parent.get(LEDGER_MAINTENANCE_FIELD), Some(&hold));
        assert_eq!(
            store.admit_workspace(admission.clone()).await,
            Err(SandboxStoreError::SandboxNotReady),
        );
        assert_eq!(
            store
                .begin_workspace_removal(&removable.id, &parent.owner, NOW.into())
                .await,
            Err(SandboxStoreError::SandboxNotReady),
        );
        assert_eq!(
            store
                .begin_workspace_prepare_recovery(&failed.id, &caller, "recover", NOW.into())
                .await,
            Err(SandboxStoreError::SandboxNotReady),
        );
        assert_eq!(
            parent_rows.find_one(filter.clone()).await.unwrap(),
            Some(held_parent)
        );
        assert_eq!(store.get_workspace(&candidate.id).await.unwrap(), None);
        assert_eq!(
            store
                .workspaces
                .find_one(removable_filter.clone())
                .await
                .unwrap()
                .as_ref(),
            Some(&original_removable)
        );
        assert_eq!(
            store
                .workspaces
                .find_one(failed_filter.clone())
                .await
                .unwrap()
                .as_ref(),
            Some(&original_failed)
        );
        assert!(store
            .sandbox_events(&parent.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }

    parent_rows
        .update_one(filter, doc! {"$unset": {LEDGER_MAINTENANCE_FIELD: ""}})
        .await
        .unwrap();
    assert!(matches!(
        store.admit_workspace(admission).await.unwrap(),
        WorkspaceAdmissionResult::Created(_)
    ));
    assert_eq!(
        store
            .begin_workspace_removal(&removable.id, &parent.owner, NOW.into())
            .await
            .unwrap()
            .status,
        WorkspaceStatus::Removing,
    );
    assert_eq!(
        store
            .begin_workspace_prepare_recovery(&failed.id, &caller, "recover", NOW.into())
            .await
            .unwrap()
            .status,
        WorkspaceStatus::Preparing,
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_profile_workspace_origin_replays_the_existing_admission() {
    let (client, name) = database("workspace_origin").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_workspace_origin");
    parent.status = SandboxStatus::Installing;
    let mut first = workspace(&parent, "wks_origin_first", "profile-project");
    first.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        first.profile.id.clone(),
        first.profile.revision,
    )];
    store.create_sandbox(parent.clone()).await.unwrap();

    let created = store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: first.clone(),
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: NOW.into(),
        })
        .await
        .unwrap();
    assert_eq!(created, WorkspaceAdmissionResult::Created(first.clone()));

    let mut replay = first.clone();
    replay.id = WorkspaceId("wks_origin_replay".into());
    let replayed = store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: replay,
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: "2026-09-05T00:00:01Z".into(),
        })
        .await
        .unwrap();
    assert_eq!(replayed, WorkspaceAdmissionResult::Existing(first));
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_sandbox_profile_workspace_replays_create_one_aggregate() {
    let (client, name) = database("workspace_origin_concurrent").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_workspace_origin_concurrent");
    parent.status = SandboxStatus::Installing;
    let mut template = workspace(&parent, "wks_origin_concurrent_0", "concurrent-project");
    template.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        template.profile.id.clone(),
        template.profile.revision,
    )];
    store.create_sandbox(parent.clone()).await.unwrap();

    let results = futures_util::future::join_all((0..8).map(|index| {
        let store = store.clone();
        let mut candidate = template.clone();
        candidate.id = WorkspaceId(format!("wks_origin_concurrent_{index}"));
        async move {
            store
                .admit_workspace(WorkspaceAdmission {
                    binding: None,
                    workspace: candidate,
                    kind: WorkspaceAdmissionKind::SandboxProfile,
                    event_at: NOW.into(),
                })
                .await
        }
    }))
    .await;
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(WorkspaceAdmissionResult::Created(_))))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(WorkspaceAdmissionResult::Existing(_))))
            .count(),
        7
    );
    let admitted_ids = results
        .into_iter()
        .map(|result| match result.unwrap() {
            WorkspaceAdmissionResult::Created(workspace)
            | WorkspaceAdmissionResult::Existing(workspace) => workspace.id,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(admitted_ids.len(), 1);
    assert_eq!(
        store
            .list_workspaces(&parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_service_records_each_mongo_profile_admission_once() {
    let (client, name) = database("workspace_service_use_count").await;
    let sandbox_store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let profile_store = Arc::new(MongoProfileStore::open(&client, &name).await.unwrap());

    let manual_profile = workspace_service_profile("wkp_mongo_manual", "manual-project");
    profile_store
        .create_workspace_profile(manual_profile.clone())
        .await
        .unwrap();
    let manual_revision = profile_store
        .publish_workspace_profile(&manual_profile.id, 1, NOW.into())
        .await
        .unwrap();
    let manual_reference =
        WorkspaceProfileRef::new(manual_revision.profile_id.clone(), manual_revision.revision);

    let automatic_profile = workspace_service_profile("wkp_mongo_automatic", "auto-project");
    profile_store
        .create_workspace_profile(automatic_profile.clone())
        .await
        .unwrap();
    let automatic_revision = profile_store
        .publish_workspace_profile(&automatic_profile.id, 1, NOW.into())
        .await
        .unwrap();
    let automatic_reference = WorkspaceProfileRef::new(
        automatic_revision.profile_id.clone(),
        automatic_revision.revision,
    );

    let mut manual_parent = sandbox("sbx_workspace_service_manual");
    manual_parent.name = "workspace-service-manual".into();
    sandbox_store
        .create_sandbox(manual_parent.clone())
        .await
        .unwrap();
    let mut automatic_parent = sandbox("sbx_workspace_service_automatic");
    automatic_parent.name = "workspace-service-automatic".into();
    automatic_parent.status = SandboxStatus::Installing;
    automatic_parent.profile.content.workspace_profiles = vec![automatic_reference.clone()];
    sandbox_store
        .create_sandbox(automatic_parent.clone())
        .await
        .unwrap();

    let service = WorkspaceService::new(sandbox_store.clone(), profile_store.clone());
    let owner = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    service
        .create(
            &owner,
            &manual_parent.id,
            CreateWorkspace {
                profile: manual_reference.clone(),
                confirmed_digest: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .create(
                &owner,
                &manual_parent.id,
                CreateWorkspace {
                    profile: manual_reference.clone(),
                    confirmed_digest: None,
                },
            )
            .await
            .unwrap_err(),
        WorkspaceServiceError::NameConflict
    );
    assert_eq!(
        profile_store
            .workspace_profile(&manual_reference.id)
            .await
            .unwrap()
            .unwrap()
            .uses_count,
        1
    );

    let automatic = futures_util::future::join_all(
        (0..8)
            .map(|_| service.create_profile_workspace(&automatic_parent, &automatic_reference, 0)),
    )
    .await;
    let ids = automatic
        .into_iter()
        .map(|result| result.unwrap().id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        profile_store
            .workspace_profile(&automatic_reference.id)
            .await
            .unwrap()
            .unwrap()
            .uses_count,
        1
    );
    assert_eq!(
        sandbox_store
            .list_workspaces(&automatic_parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_profile_workspace_rejects_a_forged_durable_slot_reference() {
    let (client, name) = database("workspace_origin_forged").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = sandbox("sbx_workspace_origin_forged");
    parent.status = SandboxStatus::Installing;
    parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        WorkspaceProfileId("wkp_expected".into()),
        7,
    )];
    store.create_sandbox(parent.clone()).await.unwrap();

    let mut forged = workspace(&parent, "wks_origin_forged", "forged-project");
    forged.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    assert_eq!(
        store
            .admit_workspace(WorkspaceAdmission {
                binding: None,
                workspace: forged,
                kind: WorkspaceAdmissionKind::SandboxProfile,
                event_at: NOW.into(),
            })
            .await
            .unwrap_err(),
        SandboxStoreError::WorkspaceOriginConflict
    );
    assert!(store
        .list_workspaces(&parent.id, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn durable_parent_slot_change_rejects_an_obsolete_workspace_replay() {
    let (client, name) = database("workspace_origin_changed").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut stale_parent = sandbox("sbx_workspace_origin_changed");
    stale_parent.status = SandboxStatus::Installing;
    let mut first = workspace(&stale_parent, "wks_origin_obsolete", "obsolete-project");
    first.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    stale_parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        first.profile.id.clone(),
        first.profile.revision,
    )];
    store.create_sandbox(stale_parent.clone()).await.unwrap();
    assert!(matches!(
        store
            .admit_workspace(WorkspaceAdmission {
                binding: None,
                workspace: first.clone(),
                kind: WorkspaceAdmissionKind::SandboxProfile,
                event_at: NOW.into(),
            })
            .await
            .unwrap(),
        WorkspaceAdmissionResult::Created(_)
    ));

    let mut durable_parent = stale_parent.clone();
    durable_parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        WorkspaceProfileId("wkp_replacement".into()),
        2,
    )];
    durable_parent.revision += 1;
    store.save_sandbox(1, durable_parent).await.unwrap();

    let mut replay = first;
    replay.id = WorkspaceId("wks_origin_obsolete_replay".into());
    assert_eq!(
        store
            .admit_workspace(WorkspaceAdmission {
                binding: None,
                workspace: replay,
                kind: WorkspaceAdmissionKind::SandboxProfile,
                event_at: "2026-09-05T00:00:01Z".into(),
            })
            .await
            .unwrap_err(),
        SandboxStoreError::WorkspaceOriginConflict
    );
    assert_eq!(
        store
            .list_workspaces(&stale_parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store
            .sandbox_events(&stale_parent.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
