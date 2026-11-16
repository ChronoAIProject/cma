//! Faults join the real leaf membership command. Root-only retry coverage
//! cannot establish that composed interruption and queued mutations are safe.
use super::*;
use futures_util::FutureExt;
mod fixture;
mod scenarios;
use fixture::{assert_attempts, Fixture};
use scenarios::{Kind, Request, KINDS};

async fn exercise(kind: Kind, failures: usize, labels: &[&str], stale: bool) {
    let mut f = Fixture::new().await;
    let outcome =
        std::panic::AssertUnwindSafe(exercise_inner(&mut f, kind, failures, labels, stale))
            .catch_unwind()
            .await;
    // Also release the owned database on an assertion panic or bounded deadline.
    let _ = f.clear().await;
    f.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
async fn exercise_inner(
    f: &mut Fixture,
    kind: Kind,
    failures: usize,
    labels: &[&str],
    stale: bool,
) {
    let (id, request) = Request::prepare(&f.store, kind).await;
    let authority = f.own(&id).await;
    let before = snapshot(&f.store).await;
    let old_agent = f.store.get_agent(&id).await.unwrap().unwrap();
    f.arm(failures, labels, stale, request.nth_member());
    let start = tokio::time::Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn({
        let store = f.store.clone();
        let (authority, request) = (authority.clone(), request.clone());
        async move { authority.run(request.run(&store)).await }
    });
    if stale {
        f.takeover(&authority).await;
    }
    let completed = tokio::time::timeout(Duration::from_secs(10), tasks.join_next()).await;
    if completed.is_err() {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    let elapsed = start.elapsed();
    let (hits, commands, observed) = f.clear().await;
    let after = snapshot(&f.store).await;
    let new_agent = f.store.get_agent(&id).await.unwrap().unwrap();
    // Outer unwind handling drops the database even when the assertions fail.
    let result = completed
        .expect("bounded admission")
        .unwrap()
        .unwrap()
        .unwrap();
    let unknown = labels.contains(&"UnknownTransactionCommitResult");
    let attempts = commands
        .iter()
        .filter(|c| matches!(c.get_bool("startTransaction"), Ok(true)))
        .count();
    assert_attempts(&commands, attempts);
    assert_eq!(
        observed.len(),
        hits,
        "each injected leaf failure must reach the driver monitor"
    );
    assert!(observed
        .iter()
        .all(|(_, code, transient, uncertain)| *code == 112
            && *transient == labels.contains(&"TransientTransactionError")
            && *uncertain == unknown));
    assert_eq!(
        new_agent.conversation, old_agent.conversation,
        "admission cannot execute a native turn"
    );
    if failures == 1 && !unknown && !stale {
        result.expect("known-aborted leaf fence must retry in a fresh transaction");
        assert_eq!(hits, 1);
        assert_eq!(attempts, 2);
        assert_eq!(after[2].len(), before[2].len() + request.added_commands());
    } else {
        assert!(
            result.is_err(),
            "uncertain/stale/exhausted leaf must not publish"
        );
        assert_eq!(after, before);
        if unknown {
            assert_eq!((hits, attempts), (1, 1));
        } else if stale {
            assert_eq!((hits, attempts), (1, 2));
        } else {
            assert!(attempts > 1 && attempts < 200);
            assert_eq!(hits, attempts);
            assert!(elapsed < Duration::from_secs(8));
        }
    }
    // Every injected ordinary attempt passed its root member write first.
    // Its exact second member command carried the same transaction identity.
    if matches!(kind, Kind::Ordinary) {
        let member_writes = commands
            .iter()
            .filter(|c| matches!(c.get_str("update"), Ok("cma_agent_manager_members")))
            .count();
        assert_eq!(member_writes, if stale { 3 } else { 2 * attempts });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn ordinary_admission_retries_leaf_abort_after_successful_root_fence() {
    exercise(Kind::Ordinary, 1, &["TransientTransactionError"], false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn composed_pair_targeted_and_queue_admissions_retry_leaf_abort() {
    for kind in [Kind::Pair, Kind::Targeted, Kind::Edit, Kind::Cancel] {
        exercise(kind, 1, &["TransientTransactionError"], false).await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish() {
    for kind in KINDS {
        for labels in [
            vec!["UnknownTransactionCommitResult"],
            vec![
                "TransientTransactionError",
                "UnknownTransactionCommitResult",
            ],
        ] {
            exercise(kind, 1, &labels, false).await;
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_refuses_durable_takeover_with_live_local_token() {
    for kind in KINDS {
        exercise(kind, 1, &["TransientTransactionError"], true).await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_exhaustion_is_bounded_without_partial_admission() {
    for kind in KINDS {
        exercise(kind, usize::MAX, &["TransientTransactionError"], false).await;
    }
}
