use super::platform_stop_integration_tests::workspace;
use super::*;
use layer2_core::sandbox::store::SandboxSessionStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ordinary_inactivity_fences_workspace_work_after_the_evidence_snapshot() {
    for leased in [false, true] {
        let (client, name, store) = database().await;
        let (sandbox, ready) = fixture(&store, "sbx_workspace_race").await;
        let idle = workspace(&sandbox, WorkspaceWorkStep::None);
        store.create_workspace(idle.clone()).await.unwrap();
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        evidence::live_activities(&store, &[ready], &mut session, LATER)
            .await
            .unwrap();
        let mut working = idle.clone();
        working
            .work
            .schedule(WorkspaceWorkStep::Prepare, LATER.into());
        if leased {
            working
                .work
                .claim(
                    "workspace-worker".into(),
                    LATER,
                    "2026-09-05T01:00:00Z".into(),
                )
                .unwrap();
        }
        working.revision += 1;
        store
            .save_workspace(idle.revision, working.clone())
            .await
            .unwrap();
        let result = transaction::apply(
            &store,
            &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
            &mut session,
        )
        .await;
        assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
        let _ = session.abort_transaction().await;
        assert_eq!(
            store
                .schedule_inactivity_pause(pause(&sandbox.id))
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::NotEligible
        );
        assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        assert_eq!(store.get_workspace(&idle.id).await.unwrap(), Some(working));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agentless_inactivity_requires_complete_old_child_and_session_evidence() {
    for case in [
        "empty",
        "idle_workspace",
        "recent_workspace",
        "unknown_ready",
        "workspace_work",
        "desktop",
        "recent_runtime",
        "recent_activity",
        "bad_activity",
    ] {
        let (client, name, store) = database().await;
        let mut sandbox = active_sandbox("sbx_agentless");
        if case == "recent_runtime" {
            sandbox.runtime.as_mut().unwrap().bound_at = LATER.into();
        }
        if case == "recent_activity" {
            sandbox.last_activity_at = LATER.into();
        }
        if case == "bad_activity" {
            sandbox.last_activity_at = "unknown".into();
        }
        store.create_sandbox(sandbox.clone()).await.unwrap();
        if case.contains("workspace") || case == "unknown_ready" {
            let mut child = workspace(&sandbox, WorkspaceWorkStep::None);
            if case == "recent_workspace" {
                child.ready_at = Some(LATER.into());
            }
            if case == "unknown_ready" {
                child.ready_at = None;
            }
            if case == "workspace_work" {
                child
                    .work
                    .schedule(WorkspaceWorkStep::Prepare, IDLE_AT.into());
            }
            store.create_workspace(child).await.unwrap();
        }
        if case == "desktop" {
            store
                .open_sandbox_session(&sandbox.id, "desktop", "2026-09-05T01:00:00Z")
                .await
                .unwrap();
        }
        let expected = if ["empty", "idle_workspace"].contains(&case) {
            SandboxMaintenanceOutcome::Scheduled
        } else {
            SandboxMaintenanceOutcome::NotEligible
        };
        assert_eq!(
            store
                .preview_inactivity_pause(pause(&sandbox.id))
                .await
                .unwrap(),
            expected,
            "{case}"
        );
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox.clone())
        );
        assert_eq!(
            store
                .schedule_inactivity_pause(pause(&sandbox.id))
                .await
                .unwrap(),
            expected,
            "{case}"
        );
        let after = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
        assert_eq!(
            after.status,
            if expected == SandboxMaintenanceOutcome::Scheduled {
                SandboxStatus::Pausing
            } else {
                SandboxStatus::Active
            },
            "{case}"
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agentless_pause_conflicts_with_new_workspace_or_owner_session() {
    use layer2_core::sandbox::store::{
        WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionStore,
    };
    for new_work in [false, true] {
        let (client, name, store) = database().await;
        let sandbox = active_sandbox("sbx_empty_race");
        store.create_sandbox(sandbox.clone()).await.unwrap();
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        assert!(evidence::idle_workspaces(&store, &sandbox, &mut session)
            .await
            .unwrap()
            .unwrap()
            .is_empty());
        if new_work {
            let mut child = workspace(&sandbox, WorkspaceWorkStep::Prepare);
            child.status = WorkspaceStatus::Preparing;
            child.ready_at = None;
            child.resolved_commit = None;
            store
                .admit_workspace(WorkspaceAdmission {
                    binding: None,
                    workspace: child,
                    kind: WorkspaceAdmissionKind::Manual,
                    event_at: LATER.into(),
                })
                .await
                .unwrap();
        } else {
            store
                .open_sandbox_session(&sandbox.id, "new-desktop", "2026-09-05T01:00:00Z")
                .await
                .unwrap();
        }
        let result = transaction::apply(
            &store,
            &MaintenanceRequest::Inactivity(pause(&sandbox.id)),
            &mut session,
        )
        .await;
        assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
        let _ = session.abort_transaction().await;
        assert_eq!(
            store
                .schedule_inactivity_pause(pause(&sandbox.id))
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::NotEligible
        );
        assert_eq!(
            store
                .get_sandbox(&sandbox.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            SandboxStatus::Active
        );
        client.database(&name).drop().await.unwrap();
    }
}
