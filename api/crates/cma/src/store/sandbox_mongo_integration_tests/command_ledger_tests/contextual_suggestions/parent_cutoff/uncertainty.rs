use super::*;
use std::time::Duration;

async fn uncertain(f: &Fixture) {
    f.client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":"alwaysOn",
            "data":{"failCommands":["commitTransaction"], "appName":&f.app,
                "blockConnection":true, "blockTimeMS":200_i32,
                "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired",
                    "errmsg":"synthetic uncertain suggestion acknowledgement"}},
        })
        .await
        .unwrap();
}

async fn clear(f: &Fixture) {
    f.client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":"off",
        })
        .await
        .unwrap();
}

fn same_transaction(gate: &Gate) {
    assert_eq!(
        gate.attempts(),
        1,
        "unknown commits must not replay the business transaction"
    );
    let commits = gate.commits();
    assert_eq!(commits.len(), 5);
    for command in &commits {
        assert_eq!(command.get("lsid"), commits[0].get("lsid"));
        assert_eq!(command.get("txnNumber"), commits[0].get("txnNumber"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated Mongo replica set with failpoints"]
async fn delayed_unknown_completion_commits_once_before_pause_and_is_hidden_afterward() {
    let f = Fixture::new().await;
    let release = f.gate.arm(Point::CommitAck);
    uncertain(&f).await;
    let completion = f.complete();
    f.gate.wait().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.row().await.get_str("status").unwrap() != "ready" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        f.pause(&f.other).await.unwrap(),
        SandboxLifecycleCommandResult::Accepted(_)
    ));
    let ready = f.row().await;
    drop(release);
    let result = completion.await.unwrap();
    clear(&f).await;
    assert!(
        result.is_err(),
        "suggestion completion preserves the unknown result"
    );
    same_transaction(&f.gate);
    assert_eq!(f.row().await, ready);
    let parent = f
        .other
        .sandboxes
        .find_one(doc! {"_id":f.parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.get_i64("suggestion_completion_fence").unwrap(), 1);
    assert!(f
        .other
        .claim_suggestions(unix_millis())
        .await
        .unwrap()
        .is_none());
    f.assert_hidden().await;
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated Mongo replica set with failpoints"]
async fn pause_reconciled_after_unknown_commit_prevents_new_ready_completion() {
    let f = Fixture::new().await;
    let release = f.gate.arm(Point::CommitAck);
    uncertain(&f).await;
    let store = f.store.clone();
    let command = SandboxLifecycleCommand::pause(
        f.parent.id.clone(),
        f.parent.owner.clone(),
        NOW.into(),
        None,
    );
    let pause = tokio::spawn(async move { store.schedule_sandbox_lifecycle(command).await });
    f.gate.wait().await;
    drop(release);
    let result = pause.await.unwrap();
    clear(&f).await;
    assert!(matches!(
        result.unwrap(),
        SandboxLifecycleCommandResult::Accepted(_)
    ));
    same_transaction(&f.gate);
    f.other
        .complete_suggestions(&f.claim, f.completion())
        .await
        .unwrap();
    assert_eq!(f.row().await.get_str("status").unwrap(), "expired");
    let parent = f
        .other
        .sandboxes
        .find_one(doc! {"_id":f.parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!parent.contains_key("suggestion_completion_fence"));
    f.assert_hidden().await;
    f.cleanup().await;
}
