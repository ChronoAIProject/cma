use super::*;
use crate::sandbox::agent_worker_tests::context_overlap_fixture::parents;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

struct Cancelled(Arc<AtomicBool>);
impl Drop for Cancelled {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn preparation_starts_during_parent_read_but_effect_waits_for_validation() {
    let (agent, sandbox, workspace, worktree) = parents().await;
    let (started, preparing) = oneshot::channel();
    let (read_done, read_wait) = oneshot::channel();
    let effect = Arc::new(AtomicBool::new(false));
    let observed = effect.clone();
    let read = read_and_inspect(
        &agent,
        async { Ok(Some(sandbox)) },
        async {
            read_wait.await.unwrap();
            Ok(Some(workspace))
        },
        async { Ok(Some(worktree)) },
        |_, admission| async move {
            started.send(()).unwrap();
            admission.wait().await?;
            observed.store(true, Ordering::SeqCst);
            Ok((RuntimeHandle::new("fixture"), None))
        },
    );
    let control = async {
        preparing.await.unwrap();
        assert!(!effect.load(Ordering::SeqCst));
        read_done.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(read, control);
    assert!(result.unwrap().1.is_ok());
    assert!(effect.load(Ordering::SeqCst));
}

#[tokio::test]
async fn foreign_parent_cancels_preparation_before_runtime_authority() {
    let (agent, sandbox, mut workspace, worktree) = parents().await;
    workspace.owner = layer2_core::SubjectId::new("foreign");
    let (started, preparing) = oneshot::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let observed = cancelled.clone();
    let result = read_and_inspect(
        &agent,
        async { Ok(Some(sandbox)) },
        async {
            preparing.await.unwrap();
            Ok(Some(workspace))
        },
        async { Ok(Some(worktree)) },
        |_, admission| async move {
            let _cancelled = Cancelled(observed);
            started.send(()).unwrap();
            admission.wait().await?;
            panic!("foreign parents must not admit an inspection")
        },
    )
    .await;
    assert_eq!(result.err().unwrap().class, "agent_lineage_invalid");
    assert!(cancelled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn invalid_parent_wins_over_earlier_inspection_failure() {
    let (agent, sandbox, mut workspace, worktree) = parents().await;
    workspace.owner = layer2_core::SubjectId::new("foreign");
    let (failed, failure) = oneshot::channel();
    let result = read_and_inspect(
        &agent,
        async { Ok(Some(sandbox)) },
        async {
            failure.await.unwrap();
            Ok(Some(workspace))
        },
        async { Ok(Some(worktree)) },
        |_, _| async move {
            failed.send(()).unwrap();
            Err(AdapterError::Unavailable("preparation unavailable".into()))
        },
    )
    .await;
    assert_eq!(result.err().unwrap().class, "agent_lineage_invalid");
}

#[tokio::test]
async fn absent_parent_preserves_failure_without_starting_inspection() {
    let (agent, _, workspace, worktree) = parents().await;
    let result = read_and_inspect(
        &agent,
        async { Ok(None) },
        async { Ok(Some(workspace)) },
        async { Ok(Some(worktree)) },
        |_, _| async { panic!("an absent Sandbox cannot start inspection") },
    )
    .await;
    assert_eq!(result.err().unwrap().class, "agent_parent_missing");
}

#[tokio::test]
async fn caller_cancellation_drops_pending_parent_and_inspection() {
    let (agent, sandbox, _, worktree) = parents().await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let observed = cancelled.clone();
    let result = tokio::time::timeout(
        Duration::from_millis(20),
        read_and_inspect(
            &agent,
            async { Ok(Some(sandbox)) },
            std::future::pending(),
            async { Ok(Some(worktree)) },
            |_, admission| async move {
                let _cancelled = Cancelled(observed);
                admission.wait().await?;
                panic!("the parent never validated")
            },
        ),
    )
    .await;
    assert!(result.is_err());
    assert!(cancelled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn valid_parents_preserve_inspection_refusal() {
    let (agent, sandbox, workspace, worktree) = parents().await;
    let result = read_and_inspect(
        &agent,
        async { Ok(Some(sandbox)) },
        async { Ok(Some(workspace)) },
        async { Ok(Some(worktree)) },
        |_, admission| async move {
            admission.wait().await?;
            Err(AdapterError::Rejected("exact process changed".into()))
        },
    )
    .await
    .unwrap();
    assert!(matches!(result.1, Err(AdapterError::Rejected(_))));
}
