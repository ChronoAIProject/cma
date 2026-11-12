//! Registered under the ordinary worker tests; reuse the actual success fixture.
use super::*;
use crate::store::ledger_maintenance_publication::test_support as maintenance;
use crate::store::ledger_maintenance_test_support as support;
use layer2_core::sandbox::RuntimeBinding;

async fn publication(store: &MongoSandboxStore) -> (Sandbox, AgentWorkMutation) {
    let (mut parent, mut tree, claimed) = claimed_agent_work_fixture(store, "held_publish").await;
    let revision = parent.revision;
    parent.runtime = Some(RuntimeBinding {
        runtime_id: "fixture-runtime".into(),
        runtime_version: "fixture-image".into(),
        provision_generation: Some(1),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    });
    parent.lifecycle.runtime_generation_high_water = 1;
    support::retain_home(&mut parent);
    parent.revision += 1;
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    let claimed = ready_publication_claim(claimed, "fixture-conversation");
    replace_agent_fixture(store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let expected_revision = tree.revision;
    tree.transition(WorktreeStatus::Ready).unwrap();
    let logs = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "fixture-conversation".into(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    next.conversation.last_seq += logs.len() as u64;
    let event = SandboxEventKind::AgentStatusChanged {
        agent_id: next.id.clone(),
        from: AgentStatus::Starting,
        to: AgentStatus::Ready,
    };
    (
        parent,
        AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: next,
            worktree: Some(VersionedWorktree {
                expected_revision,
                worktree: tree,
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(event),
            agent_logs: logs,
        },
    )
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set and publication hooks"]
async fn ordinary_publication_has_a_positive_control_and_both_hold_refusals() {
    for hold in [0, 1, 2] {
        let (client, name) = database("ledger_ordinary_publication").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (parent, done) = publication(&store).await;
        let fence = maintenance::fence_value(&store, &parent).await;
        if hold != 0 {
            maintenance::hold(&store, &parent, hold == 1).await;
        }
        let before = support::all_rows(&store).await;
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        let result = reopened.commit_agent_work(done.clone()).await;
        if hold == 0 {
            result.unwrap();
            assert!(reopened.get_agent(&done.agent.id).await.unwrap().unwrap() == done.agent);
            let tree = &done.worktree.as_ref().unwrap().worktree;
            assert!(reopened.get_worktree(&tree.id).await.unwrap().unwrap() == *tree);
            assert_eq!(
                reopened
                    .agent_log(&done.agent.id, 0, 10)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|v| v.kind)
                    .collect::<Vec<_>>(),
                done.agent_logs
            );
            assert_eq!(
                reopened.sandbox_events(&parent.id, 0, 10).await.unwrap()[0].kind,
                done.sandbox_event.unwrap()
            );
            assert_eq!(maintenance::fence_value(&store, &parent).await, fence + 1);
        } else {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(support::all_rows(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable replica set with failpoints and publication hooks"]
async fn failed_publication_commit_leaves_no_parent_child_log_or_event_change() {
    let (client, name) = database("ledger_publication_commit_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, done) = publication(&store).await;
    let before = support::all_rows(&store).await;
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1},
            "data":{"failCommands":["commitTransaction"],"errorCode":13}
        })
        .await
        .unwrap();
    let outcome = store.commit_agent_work(done.clone()).await;
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":"off"
        })
        .await
        .unwrap();
    assert!(matches!(outcome, Err(SandboxStoreError::Unavailable(_))));
    assert_eq!(support::all_rows(&store).await, before);
    support::observe_transaction_cleanup(&store).await;
    assert_eq!(support::all_rows(&store).await, before);
    store.commit_agent_work(done.clone()).await.unwrap();
    assert!(store.get_agent(&done.agent.id).await.unwrap().unwrap() == done.agent);
    client.database(&name).drop().await.unwrap();
}
