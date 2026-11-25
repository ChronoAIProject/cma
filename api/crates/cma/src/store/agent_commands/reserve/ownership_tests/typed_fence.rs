//! Faults reach current admission fences without restoring foreground ownership.
//! Composed commands retain their original resource revisions after known aborts.
use crate::store::sandbox_maintenance::mongo_integration_tests::fixture;
use crate::store::sandbox_mongo::MongoSandboxStore;
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::{
    record, work_tests::live_request,
};
use futures_util::FutureExt;
use futures_util::TryStreamExt;
use layer2_core::sandbox::{agent_io::*, store::*, Agent, Sandbox};
use layer2_core::AgentId;
use mongodb::bson::{doc, Document};
use std::time::Duration;
#[path = "typed_fence/fixture.rs"]
mod fixture;
#[path = "typed_fence/pair.rs"]
mod pair;
#[path = "typed_fence/scenarios.rs"]
mod scenarios;
use fixture::{assert_attempts, Fixture};
use scenarios::{Kind, Outcome, Request};

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
    let mut before = snapshot(&f.store).await;
    let old_agent = f.store.get_agent(&id).await.unwrap().unwrap();
    f.arm(failures, labels, stale, 1);
    let start = tokio::time::Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn({
        let store = f.store.clone();
        let (authority, request) = (authority.clone(), request.clone());
        async move { authority.run(request.run(&store)).await }
    });
    if stale {
        f.takeover(&authority).await;
        let original = before[0]
            .iter_mut()
            .find(|row| row.get_str("_id").ok() == Some(id.as_str()))
            .unwrap();
        original.insert("revision", old_agent.revision as i64 + 1);
        original
            .get_document_mut("record")
            .unwrap()
            .insert("revision", old_agent.revision as i64 + 1);
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
        assert!(matches!(
            result.expect("known-aborted leaf fence must retry in a fresh transaction"),
            Outcome::Reserved | Outcome::QueuedInput(QueuedInputOutcome::Applied(_))
        ));
        assert_eq!(hits, 1);
        assert_eq!(attempts, 2);
        assert_eq!(after[2].len(), before[2].len() + request.added_commands());
    } else {
        if stale && matches!(kind, Kind::Edit) {
            assert!(
                matches!(
                    result,
                    Ok(Outcome::QueuedInput(QueuedInputOutcome::StaleObservation))
                ),
                "stale queued edit must retain its typed refusal"
            );
        } else {
            assert!(
                result.is_err(),
                "uncertain/stale/exhausted leaf must not publish"
            );
        }
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
    // Public admission does not acquire or advance a runtime owner. Its own
    // Agent fence still runs exactly once per reached admission attempt.
    assert!(!commands.iter().any(|command| matches!(
        command.get_str("update").ok(),
        Some("cma_agent_manager_members" | "cma_agent_owners")
    )));
    let fences = commands
        .iter()
        .filter(|command| {
            command.get_str("update").ok() == Some("agents")
                && command.get_array("updates").is_ok_and(|updates| {
                    updates.iter().any(|update| {
                        update
                            .as_document()
                            .and_then(|row| row.get_document("u").ok())
                            .and_then(|change| change.get_document("$inc").ok())
                            .is_some_and(|fields| {
                                fields.contains_key("agent_command_admission_fence")
                            })
                    })
                })
        })
        .count();
    let successful_extra_fences = if failures == 1 && !unknown && !stale {
        // A composed pair admits both commands in the successful transaction.
        request.added_commands() - 1
    } else {
        0
    };
    assert_eq!(
        fences,
        if stale {
            1
        } else {
            attempts + successful_extra_fences
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn ordinary_admission_retries_leaf_abort_after_successful_root_fence() {
    exercise(Kind::Ordinary, 1, &["TransientTransactionError"], false).await;
}
async fn composed_pair_targeted_and_queue_admissions_retry_leaf_abort_case(kind: Kind) {
    exercise(kind, 1, &["TransientTransactionError"], false).await;
}
async fn leaf_unknown_and_both_labels_never_replay_or_publish_case(
    kind: Kind,
    labels: Vec<&'static str>,
) {
    exercise(kind, 1, &labels, false).await;
}
async fn leaf_retry_refuses_durable_takeover_with_live_local_token_case(kind: Kind) {
    exercise(kind, 1, &["TransientTransactionError"], true).await;
}
async fn leaf_retry_exhaustion_is_bounded_without_partial_admission_case(kind: Kind) {
    exercise(kind, usize::MAX, &["TransientTransactionError"], false).await;
}

async fn snapshot(store: &MongoSandboxStore) -> Vec<Vec<Document>> {
    let mut rows = Vec::new();
    for collection in [
        &store.agents,
        &store.sandboxes,
        &store.agent_commands,
        &store.agent_command_orders,
        &store.command_authorization_audits,
    ] {
        rows.push(
            collection
                .find(doc! {})
                .sort(doc! {"_id": 1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    rows
}

#[path = "typed_fence_cases.rs"]
mod cases;
