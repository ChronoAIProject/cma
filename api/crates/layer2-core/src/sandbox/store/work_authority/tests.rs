use super::*;
use crate::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
use crate::sandbox::{
    WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};
use std::sync::atomic::AtomicUsize;

fn authority() -> WorkAuthority {
    let now = "2026-11-14T00:00:00Z";
    let mut work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, now.into());
    work.claim("original".into(), now, "2026-11-14T00:00:15Z".into())
        .unwrap();
    WorkAuthority::new(
        ClaimedWork::Workspace(Box::new(Workspace {
            id: crate::WorkspaceId("wks_renewal".into()),
            sandbox_id: crate::SandboxId("sbx_renewal".into()),
            owner: crate::SubjectId("owner".into()),
            origin: WorkspaceOrigin::Manual,
            profile: WorkspaceProfileSnapshot {
                id: crate::WorkspaceProfileId("wkp_renewal".into()),
                revision: 1,
                digest: "a".repeat(64),
                content: WorkspaceProfileContent {
                    location: WorkspaceLocation::Directory {
                        name: "project".into(),
                    },
                    name: "project".into(),
                    setup_commands: vec![],
                    agent_profiles: vec![],
                },
            },
            name: "project".into(),
            path: "/home/cma/project".into(),
            source: WorkspaceSource::Empty,
            resolved_commit: None,
            status: WorkspaceStatus::Preparing,
            work,
            next_worktree_number: 1,
            created_at: now.into(),
            ready_at: None,
            failed: None,
            removed_at: None,
            revision: 2,
        })),
        Instant::now(),
    )
}

fn grant() -> Result<Option<WorkLeaseGrant>, SandboxStoreError> {
    WorkLeaseGrant::new("2026-11-14T00:00:15Z".into(), 15_000).map(Some)
}

#[tokio::test(start_paused = true)]
async fn long_effect_is_renewed_every_three_seconds_and_scope_closes() {
    let authority = authority();
    let starts = Mutex::new(Vec::new());
    let start = Instant::now();
    let result = authority
        .run(
            async {
                assert_eq!(WorkAuthority::current()?.len(), 1);
                tokio::time::sleep(Duration::from_secs(32)).await;
                Ok(42)
            },
            || async {
                starts.lock().unwrap().push(Instant::now() - start);
                grant()
            },
        )
        .await;
    assert_eq!(result, Ok(42));
    assert_eq!(
        *starts.lock().unwrap(),
        (0..=10)
            .map(|tick| Duration::from_secs(tick * 3))
            .collect::<Vec<_>>()
    );
    assert!(authority.deadline().is_err());
    assert!(!WorkAuthority::active());
}

#[tokio::test(start_paused = true)]
async fn refused_initial_renewal_never_polls_effect() {
    let polls = AtomicUsize::new(0);
    let result = authority()
        .run(
            async {
                polls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || async { Ok(None) },
        )
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(polls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn late_reply_cannot_revive_original_deadline_or_poll_effect() {
    let authority = authority();
    tokio::time::advance(Duration::from_secs(14)).await;
    let polls = AtomicUsize::new(0);
    let result = authority
        .run(
            async {
                polls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                grant()
            },
        )
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    assert!(authority.deadline().is_err());
}

#[tokio::test(start_paused = true)]
async fn renewal_uncertainty_drops_pending_work_within_bounded_call() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let work_guard = Dropped(dropped.clone());
    let calls = AtomicUsize::new(0);
    let start = Instant::now();
    let result = authority()
        .run(
            async {
                let _guard = work_guard;
                std::future::pending::<Result<(), SandboxStoreError>>().await
            },
            || async {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    grant()
                } else {
                    std::future::pending().await
                }
            },
        )
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(start.elapsed(), Duration::from_secs(6));
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn renewal_can_wait_for_a_lock_held_by_the_work() {
    let row = tokio::sync::Mutex::new(());
    let calls = AtomicUsize::new(0);
    let start = Instant::now();
    let result = authority()
        .run(
            async {
                let _locked = row.lock().await;
                tokio::time::sleep(Duration::from_secs(4)).await;
                Ok(42)
            },
            || async {
                if calls.fetch_add(1, Ordering::SeqCst) != 0 {
                    let _locked = row.lock().await;
                }
                grant()
            },
        )
        .await;
    assert_eq!(result, Ok(42));
    assert_eq!(start.elapsed(), Duration::from_secs(4));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn request_start_bounds_even_a_successful_delayed_receipt() {
    let authority = authority();
    let start = Instant::now();
    tokio::time::advance(Duration::from_secs(2)).await;
    authority.accept(start, grant().unwrap().unwrap()).unwrap();
    assert_eq!(
        authority.deadline().unwrap(),
        start + Duration::from_secs(15)
    );
    tokio::time::advance(Duration::from_secs(13)).await;
    assert!(authority
        .accept(Instant::now(), grant().unwrap().unwrap())
        .is_err());
}
