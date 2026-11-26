use super::*;
use std::time::Duration;

mod probe;
mod races;
use probe::{Fixture, Pause};

fn ready(original: &Workspace) -> Workspace {
    let mut next = original.clone();
    next.record_resolved_commit("a".repeat(40)).unwrap();
    next.work.schedule(WorkspaceWorkStep::None, MINUTE.into());
    next.transition(WorkspaceStatus::Ready, MINUTE.into(), None)
        .unwrap();
    next
}

fn publication(original: &Workspace) -> SandboxTransaction {
    let next = ready(original);
    SandboxTransaction::SaveWorkspaceAndAppend {
        repository_runtime: None,
        expected_revision: original.revision,
        event_at: MINUTE.into(),
        event: SandboxEventKind::WorkspaceReady {
            workspace_id: next.id.clone(),
            resolved_commit: next.resolved_commit.clone().unwrap(),
        },
        workspace: Box::new(next),
    }
}

async fn retire(store: &MongoSandboxStore, id: &SandboxId, retired: bool) {
    let mut parent = store.get_sandbox(id).await.unwrap().unwrap();
    let from = parent.status;
    let expected_revision = parent.revision;
    parent.begin_retirement(MINUTE.into()).unwrap();
    store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision,
            sandbox: Box::new(parent.clone()),
            event_at: MINUTE.into(),
            event: SandboxEventKind::StatusChanged {
                from,
                to: SandboxStatus::Retiring,
            },
        })
        .await
        .unwrap();
    if retired {
        let expected_revision = parent.revision;
        parent
            .transition(SandboxStatus::Retired, TWO_MINUTES.into(), None)
            .unwrap();
        parent
            .work
            .schedule(SandboxWorkStep::None, TWO_MINUTES.into());
        store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision,
                sandbox: Box::new(parent),
                event_at: TWO_MINUTES.into(),
                event: SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Retiring,
                    to: SandboxStatus::Retired,
                },
            })
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ready_refuses_retiring_retired_and_missing_parent_without_event_or_wake() {
    for status in [
        Some(SandboxStatus::Retiring),
        Some(SandboxStatus::Retired),
        None,
    ] {
        let f = Fixture::new().await;
        if let Some(status) = status {
            retire(&f.peer, &f.parent.id, status == SandboxStatus::Retired).await;
        } else {
            f.peer
                .sandboxes
                .delete_one(doc! {"_id":f.parent.id.as_str()})
                .await
                .unwrap();
        }
        let before = f.rows().await;
        let wake = f.store.agent_parent_ready.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        let result = f.store.commit(publication(&f.original)).await;
        assert_eq!(
            result,
            Err(if status.is_some() {
                SandboxStoreError::Conflict
            } else {
                SandboxStoreError::NotFound
            })
        );
        assert_eq!(f.rows().await, before);
        assert!(tokio::time::timeout(Duration::from_millis(20), wake)
            .await
            .is_err());
        f.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ready_preserves_active_installing_progress_and_single_revision_publication() {
    for status in [SandboxStatus::Active, SandboxStatus::Installing] {
        let mut f = Fixture::new().await;
        if status == SandboxStatus::Installing {
            f.parent.status = status;
            f.parent.revision += 1;
            f.peer.save_sandbox(1, f.parent.clone()).await.unwrap();
        }
        let (a, b) = tokio::join!(
            f.store.commit(publication(&f.original)),
            f.peer.commit(publication(&f.original))
        );
        assert_eq!([&a, &b].iter().filter(|result| result.is_ok()).count(), 1);
        assert!([a, b].contains(&Err(SandboxStoreError::Conflict)));
        assert_eq!(
            f.peer.get_workspace(&f.original.id).await.unwrap(),
            Some(ready(&f.original))
        );
        let events = f.peer.sandbox_events(&f.parent.id, 0, 10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].kind,
            SandboxEventKind::WorkspaceReady { .. }
        ));
        f.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn generic_save_cannot_bypass_ready_parent_fence_or_omit_ready_event() {
    for retired in [false, true] {
        let f = Fixture::new().await;
        if retired {
            retire(&f.peer, &f.parent.id, true).await;
        }
        let before = f.rows().await;
        let result = f
            .store
            .save_workspace(f.original.revision, ready(&f.original))
            .await;
        if retired {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(f.rows().await, before);
        } else {
            result.unwrap();
            let events = f.peer.sandbox_events(&f.parent.id, 0, 10).await.unwrap();
            assert_eq!(events.len(), 1);
            assert!(matches!(
                events[0].kind,
                SandboxEventKind::WorkspaceReady { .. }
            ));
        }
        f.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn removal_wins_workspace_revision_and_cleanup_completes_after_parent_retirement() {
    for retired in [false, true] {
        let f = Fixture::new().await;
        let mut removing = f.original.clone();
        removing
            .transition(WorkspaceStatus::Removing, MINUTE.into(), None)
            .unwrap();
        removing
            .work
            .schedule(WorkspaceWorkStep::Remove, MINUTE.into());
        f.peer
            .save_workspace(f.original.revision, removing.clone())
            .await
            .unwrap();
        retire(&f.peer, &f.parent.id, retired).await;
        assert_eq!(
            f.store.commit(publication(&f.original)).await,
            Err(SandboxStoreError::Conflict)
        );
        let mut removed = removing.clone();
        removed
            .transition(WorkspaceStatus::Removed, TWO_MINUTES.into(), None)
            .unwrap();
        removed
            .work
            .schedule(WorkspaceWorkStep::None, TWO_MINUTES.into());
        f.store
            .commit(SandboxTransaction::SaveWorkspaceAndAppend {
                repository_runtime: None,
                expected_revision: removing.revision,
                workspace: Box::new(removed.clone()),
                event_at: TWO_MINUTES.into(),
                event: SandboxEventKind::WorkspaceRemoved {
                    workspace_id: removed.id.clone(),
                },
            })
            .await
            .unwrap();
        assert_eq!(
            f.peer.get_workspace(&removed.id).await.unwrap(),
            Some(removed)
        );
        assert!(!f
            .peer
            .sandbox_events(&f.parent.id, 0, 10)
            .await
            .unwrap()
            .iter()
            .any(|e| matches!(e.kind, SandboxEventKind::WorkspaceReady { .. })));
        f.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ready_event_conflict_rolls_back_workspace_and_parent_head() {
    let f = Fixture::new().await;
    let conflict = SandboxEvent {
        sandbox_id: f.parent.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    f.peer
        .sandbox_events
        .insert_one(event_document(&conflict).unwrap())
        .await
        .unwrap();
    let before = f.rows().await;
    assert_eq!(
        f.store.commit(publication(&f.original)).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(f.rows().await, before);
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ready_refuses_parent_owner_changes_and_workspace_reparenting() {
    for change_parent in [false, true] {
        let mut f = Fixture::new().await;
        let mut transaction = publication(&f.original);
        if change_parent {
            f.parent.owner = SubjectId("changed-owner".into());
            f.parent.revision += 1;
            f.peer.save_sandbox(1, f.parent.clone()).await.unwrap();
        } else {
            let mut other = sandbox("sbx_other");
            other.owner = SubjectId("other-owner".into());
            f.peer.create_sandbox(other.clone()).await.unwrap();
            let SandboxTransaction::SaveWorkspaceAndAppend { workspace, .. } = &mut transaction
            else {
                unreachable!()
            };
            workspace.sandbox_id = other.id;
            workspace.owner = other.owner;
            assert_eq!(
                f.store.save_workspace(1, *workspace.clone()).await,
                Err(SandboxStoreError::Conflict)
            );
        }
        let before = f.rows().await;
        assert_eq!(
            f.store.commit(transaction).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(f.rows().await, before);
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_ready_metadata_save_does_not_republish_after_retirement() {
    let f = Fixture::new().await;
    f.store.commit(publication(&f.original)).await.unwrap();
    retire(&f.peer, &f.parent.id, true).await;
    let events = f.peer.sandbox_events(&f.parent.id, 0, 20).await.unwrap();
    let mut next = ready(&f.original);
    let expected = next.revision;
    next.next_worktree_number += 1;
    next.revision += 1;
    f.store
        .save_workspace(expected, next.clone())
        .await
        .unwrap();
    assert_eq!(f.peer.get_workspace(&next.id).await.unwrap(), Some(next));
    assert_eq!(
        f.peer.sandbox_events(&f.parent.id, 0, 20).await.unwrap(),
        events
    );
    f.close().await;
}
