use super::*;
use crate::store::sandbox_mongo_integration_tests::transaction_reconciliation_tests::command_fence::fixture::{
    assert_attempts, Fixture,
};
use std::time::Duration;

#[derive(Clone, Copy)]
enum Root {
    Conversation,
    Checkpoint,
}
async fn apply(
    store: &MongoSandboxStore,
    agent: &Agent,
    root: Root,
) -> Result<(), SandboxStoreError> {
    match root {
        Root::Conversation => store
            .commit_agent_conversation(update(agent, "retained native result"))
            .await
            .map(|_| ()),
        Root::Checkpoint => store
            .commit_agent_message_update(
                agent.clone(),
                None,
                partial(agent, "retained native result"),
                None,
            )
            .await
            .map(|_| ()),
    }
}
async fn exercise(root: Root, failures: usize, labels: &[&str], stale: bool) {
    let mut f = Fixture::new().await;
    let agent = working(&f.store, "m2-conversation-fence").await;
    let logs = f.store.agent_log(&agent.id, 0, 100).await.unwrap();
    let authority = f.own(&agent.id).await;
    f.arm(failures, labels, stale);
    let task = tokio::spawn({
        let store = f.store.clone();
        let agent = agent.clone();
        let authority = authority.clone();
        async move { authority.run(apply(&store, &agent, root)).await }
    });
    if stale {
        f.takeover(&authority).await;
    }
    let result = tokio::time::timeout(Duration::from_secs(20), task).await;
    let (hits, commands) = f.clear().await;
    let after = f.store.get_agent(&agent.id).await.unwrap().unwrap();
    let next_logs = f.store.agent_log(&agent.id, 0, 100).await.unwrap();
    let checkpoint = f.store.agent_message_checkpoint(&agent.id).await.unwrap();
    f.cleanup().await;
    let result = result.expect("must remain bounded").unwrap().unwrap();
    let unknown = labels.contains(&"UnknownTransactionCommitResult");
    if failures == 1 && !unknown && !stale {
        result.expect("known-aborted conversation ownership fence must retry");
        assert_attempts(&commands, 2);
        assert_eq!(hits, 1);
        match root {
            Root::Conversation => {
                assert_eq!(after, update(&agent, "retained native result").agent);
                assert_eq!(next_logs.len(), logs.len() + 1);
                assert!(checkpoint.is_none());
            }
            Root::Checkpoint => {
                assert_eq!(after, agent);
                assert_eq!(next_logs, logs);
                assert!(checkpoint.is_some());
            }
        }
    } else {
        assert!(
            result.is_err(),
            "uncertain or stale publication must refuse"
        );
        assert_eq!(after, agent);
        assert_eq!(next_logs, logs);
        assert!(checkpoint.is_none());
        let attempts = if unknown {
            1
        } else if stale {
            2
        } else {
            5
        };
        assert_attempts(&commands, attempts);
        assert_eq!(hits, if stale { 1 } else { attempts });
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_known_abort() {
    exercise(Root::Conversation, 1, &["TransientTransactionError"], false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_checkpoint_known_abort() {
    exercise(Root::Checkpoint, 1, &["TransientTransactionError"], false).await;
}
async fn typed_fence_conversation_exhaustion_unknown_and_both_labels_case(
    root: Root,
    labels: Vec<&'static str>,
) {
    exercise(root, 5, &labels, false).await;
}
async fn typed_fence_conversation_recheck_stale_token_case(root: Root) {
    exercise(root, 1, &["TransientTransactionError"], true).await;
}

#[path = "typed_fence_tests_cases.rs"]
mod cases;
