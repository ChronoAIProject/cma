use super::*;
use crate::store::admission_wake_test_support::CommitGate;
use futures_util::FutureExt;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn parent_readiness_wake_follows_durable_commit_and_never_failed_publication() {
    for workspace_ready in [false, true] {
        let gate = Arc::new(CommitGate::default());
        let client = gate.client().await;
        let database = format!("cma_parent_wake_{}", uuid::Uuid::new_v4().simple());
        let store = MongoSandboxStore::open(&client, &database).await.unwrap();
        let wake = store.agent_admitted.clone();
        let ready_a = store.agent_parent_ready.notified();
        let ready_b = store.agent_parent_ready.notified();
        tokio::pin!(ready_a, ready_b);
        ready_a.as_mut().enable();
        ready_b.as_mut().enable();
        let mut parent = sandbox("sbx_parent_ready");
        if !workspace_ready {
            parent.status = SandboxStatus::Installing;
            parent.work = SandboxWorkState::new(SandboxWorkStep::Activate, NOW.into());
        }
        store.create_sandbox(parent.clone()).await.unwrap();
        let child = workspace(&parent, "wks_parent_ready", "project");
        store.create_workspace(child.clone()).await.unwrap();
        let transaction = if workspace_ready {
            let mut next = child.clone();
            let commit = "a".repeat(40);
            next.resolved_commit = Some(commit.clone());
            next.status = WorkspaceStatus::Ready;
            next.ready_at = Some(MINUTE.into());
            next.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, MINUTE.into());
            next.revision += 1;
            SandboxTransaction::SaveWorkspaceAndAppend {
                repository_runtime: None,
                expected_revision: child.revision,
                workspace: Box::new(next),
                event_at: MINUTE.into(),
                event: SandboxEventKind::WorkspaceReady {
                    workspace_id: child.id.clone(),
                    resolved_commit: commit,
                },
            }
        } else {
            let mut next = parent.clone();
            next.status = SandboxStatus::Active;
            next.work = SandboxWorkState::new(
                SandboxWorkStep::WaitForCredentialRotation,
                TWO_MINUTES.into(),
            );
            next.updated_at = MINUTE.into();
            next.revision += 1;
            SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: parent.revision,
                sandbox: Box::new(next),
                event_at: MINUTE.into(),
                event: SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Installing,
                    to: SandboxStatus::Active,
                },
            }
        };
        let mut invalid = transaction.clone();
        match &mut invalid {
            SandboxTransaction::SaveSandboxAndAppend {
                expected_revision, ..
            }
            | SandboxTransaction::SaveWorkspaceAndAppend {
                expected_revision, ..
            } => *expected_revision = 999,
            _ => unreachable!(),
        }
        assert!(store.commit(invalid).await.is_err());
        assert!(wake.notified().now_or_never().is_none());
        assert!(ready_a.as_mut().now_or_never().is_none());
        assert!(ready_b.as_mut().now_or_never().is_none());

        let writer = store.clone();
        let input = transaction.clone();
        gate.arm();
        let task = tokio::spawn(async move { writer.commit(input).await });
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        let premature = wake.notified().now_or_never().is_some();
        let premature_waiter =
            ready_a.as_mut().now_or_never().is_some() || ready_b.as_mut().now_or_never().is_some();
        let before_parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
        let before_child = store.get_workspace(&child.id).await.unwrap().unwrap();
        gate.release();
        task.await.unwrap().unwrap();
        assert!(!premature, "readiness cannot wake before commit");
        assert!(
            !premature_waiter,
            "leased waiters cannot wake before commit"
        );
        assert_eq!(before_parent, parent);
        assert_eq!(before_child, child);
        assert!(wake.notified().now_or_never().is_some());
        assert!(wake.notified().now_or_never().is_none());
        assert!(ready_a.as_mut().now_or_never().is_some());
        assert!(ready_b.as_mut().now_or_never().is_some());
        if workspace_ready {
            assert_eq!(
                store
                    .get_workspace(&child.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                WorkspaceStatus::Ready
            );
        } else {
            let active = store.get_sandbox(&parent.id).await.unwrap().unwrap();
            assert_eq!(active.status, SandboxStatus::Active);
            assert_eq!(active.work.ready_at, TWO_MINUTES);
        }
        // A stale retry must not emit another readiness hint.
        assert!(store.commit(transaction).await.is_err());
        assert!(wake.notified().now_or_never().is_none());
        client.database(&database).drop().await.unwrap();
        client.shutdown().await;
    }
}
