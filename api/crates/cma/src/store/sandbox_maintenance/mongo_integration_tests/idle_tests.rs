use super::platform_stop_integration_tests::workspace;
use super::*;
use layer2_core::sandbox::store::SandboxSessionStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ordinary_inactivity_fences_workspace_work_after_the_evidence_snapshot() {
    for leased in [false, true] {
        let (client, name, store) = database().await;
        let (sandbox, ready) = fixture(&store, "sbx_workspace_race").await;
        let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
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
        replica
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
        let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
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
            replica
                .admit_workspace(WorkspaceAdmission {
                    binding: None,
                    workspace: child,
                    kind: WorkspaceAdmissionKind::Manual,
                    event_at: LATER.into(),
                })
                .await
                .unwrap();
        } else {
            replica
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_pause_rechecks_input_and_desktop_activity_after_preview_on_another_store() {
    use layer2_core::sandbox::store::{InactivityPauseCause, SandboxMutationGuard};
    use layer2_core::sandbox::QueuedAgentMessage;
    for activity in ["recent_log", "queued_input", "desktop"] {
        let (client, name, store) = database().await;
        let other_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let replica = MongoSandboxStore::open(&other_client, &name).await.unwrap();
        let (sandbox, ready) = fixture(&store, "sbx_preview_activity").await;
        let (_, policy) = policy_tests::enable(&client, &name).await;
        let request = SandboxInactivityPauseRequest {
            cause: InactivityPauseCause::OperatorInactivity,
            expected_policy_revision: Some(policy.revision),
            ..policy_tests::global(&sandbox.id)
        };
        assert_eq!(
            store
                .preview_inactivity_pause(policy_tests::global(&sandbox.id))
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::Scheduled
        );
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        evidence::live_activities(&store, std::slice::from_ref(&ready), &mut session, LATER)
            .await
            .unwrap();
        if activity == "desktop" {
            replica
                .open_sandbox_session(&sandbox.id, "desktop", "2026-09-05T01:00:00Z")
                .await
                .unwrap();
        } else {
            let mut next = ready.clone();
            next.revision += 1;
            next.conversation.last_seq += 1;
            let content = "Synthetic activity after preview.".to_owned();
            let log = if activity == "queued_input" {
                next.conversation
                    .queued_messages
                    .push(QueuedAgentMessage::new(2, content.clone(), LATER.into()).unwrap());
                AgentLogKind::OwnerMessage { content }
            } else {
                AgentLogKind::AgentMessage { content }
            };
            replica
                .commit_agent_conversation(AgentConversationMutation {
                    settings_fence: None,
                    command_claim: None,
                    execution_fence: None,
                    trigger_execution_fence: None,
                    admission_guard: (activity == "queued_input")
                        .then(|| SandboxMutationGuard::capture(&sandbox)),
                    expected_agent_revision: ready.revision,
                    expected_last_seq: ready.conversation.last_seq,
                    runtime_generation: 1,
                    agent: next,
                    appended_at: LATER.into(),
                    agent_logs: vec![log],
                })
                .await
                .unwrap();
        }
        let admitted = replica.get_agent(&ready.id).await.unwrap();
        let logs = replica.agent_log(&ready.id, 0, 100).await.unwrap();
        let result = transaction::apply(
            &store,
            &MaintenanceRequest::Inactivity(request.clone()),
            &mut session,
        )
        .await;
        assert!(
            matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)),
            "{activity}"
        );
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.schedule_inactivity_pause(request).await.unwrap(),
            SandboxMaintenanceOutcome::NotEligible,
            "{activity}"
        );
        assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        assert_eq!(store.get_agent(&ready.id).await.unwrap(), admitted);
        assert_eq!(store.agent_log(&ready.id, 0, 100).await.unwrap(), logs);
        assert!(store
            .sandbox_events(&ready.sandbox_id, 0, 100)
            .await
            .unwrap()
            .is_empty());
        client.database(&name).drop().await.unwrap();
    }
}

// #1330: the owner finishes adding a Workspace while the existing live Agent
// stays quiet with an old log. The Sandbox's recorded activity must be a full
// interval old before preview or admission schedule the pause.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn live_agent_inactivity_waits_a_full_interval_after_recent_sandbox_activity() {
    const OWNER_WORK_AT: &str = "2026-09-05T00:20:00Z";
    for active_turn in [false, true] {
        let (client, name, store) = database().await;
        let mut sandbox = active_sandbox("sbx_recent_owner_work");
        sandbox.last_activity_at = OWNER_WORK_AT.into();
        store.create_sandbox(sandbox.clone()).await.unwrap();
        let mut agent = ready_agent(&sandbox, "agt_sbx_recent_owner_work");
        if active_turn {
            agent.status = AgentStatus::Working;
            agent.conversation.active_turn = Some(1);
        }
        insert_agent(&store, &agent, IDLE_AT).await;
        let mut added = workspace(&sandbox, WorkspaceWorkStep::None);
        added.id = WorkspaceId("wks_added_by_owner".into());
        added.name = "added".into();
        added.path = "/home/cma/added".into();
        added.created_at = OWNER_WORK_AT.into();
        added.ready_at = Some(OWNER_WORK_AT.into());
        store.create_workspace(added).await.unwrap();

        for (requested_at, due) in [
            (LATER, false),
            // The threshold boundary is strict, as for Agent logs.
            ("2026-09-05T00:35:00Z", false),
            ("2026-09-05T00:35:01Z", !active_turn),
        ] {
            let request = SandboxInactivityPauseRequest {
                requested_at: requested_at.into(),
                ..pause(&sandbox.id)
            };
            let expected = if due {
                SandboxMaintenanceOutcome::Scheduled
            } else {
                SandboxMaintenanceOutcome::NotEligible
            };
            assert_eq!(
                store
                    .preview_inactivity_pause(request.clone())
                    .await
                    .unwrap(),
                expected,
                "{requested_at} active_turn={active_turn}"
            );
            if !due {
                assert_eq!(
                    store.schedule_inactivity_pause(request).await.unwrap(),
                    expected,
                    "{requested_at} active_turn={active_turn}"
                );
                assert_eq!(
                    store.get_sandbox(&sandbox.id).await.unwrap(),
                    Some(sandbox.clone()),
                    "{requested_at} active_turn={active_turn}"
                );
                assert!(store
                    .sandbox_events(&sandbox.id, 0, 100)
                    .await
                    .unwrap()
                    .is_empty());
                continue;
            }
            assert_eq!(
                store.schedule_inactivity_pause(request).await.unwrap(),
                expected
            );
            let after = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
            assert_eq!(after.status, SandboxStatus::Pausing);
            assert_eq!(after.revision, sandbox.revision + 1);
        }
        client.database(&name).drop().await.unwrap();
    }
}
