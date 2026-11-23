use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::database;
use crate::store::sandbox_mongo_integration_tests::ready_conversation_fixture;
use layer2_core::sandbox::{
    store::{AgentLogStore, AgentStore, AgentWorkMutation, AgentWorkerStore},
    AgentLogKind, AgentWorkStep,
};

const NOW: &str = "2026-11-05T00:00:00Z";
const LEASE_END: &str = "2026-11-05T00:18:00Z";

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn same_manager_resumes_retained_durable_work_under_a_strictly_newer_epoch() {
    for pacing in [false, true] {
        let (client, name, off) = database().await;
        let mut agent = ready_conversation_fixture(&off, "contained-durable-work").await;
        let original_conversation = agent.conversation.clone();
        let original_history = off.agent_log(&agent.id, 0, 10).await.unwrap();
        assert!(!original_history.is_empty());
        let revision = agent.revision;
        agent
            .work
            .schedule(AgentWorkStep::PublishConnection, NOW.into());
        agent.revision += 1;
        off.save_agent(revision, agent.clone()).await.unwrap();
        let mut store = off.enable_agent_ownership("member-a".into()).await.unwrap();
        let ownership = store.agent_ownership.as_ref().unwrap().clone();
        let a = manager(ownership.clone(), "member-a", pacing, 2).await;
        let b = manager(ownership.clone(), "member-b", pacing, 2).await;
        let membership = ownership.membership().await.unwrap();
        let preferred = layer2_core::agent_ownership::candidates(&agent.id, &membership, None)
            .unwrap()[0]
            .instance_id
            .clone();
        let coordinator = if a.instance_id == preferred { &a } else { &b };
        let peer = if a.instance_id == preferred { &b } else { &a };
        store.agent_manager_instance = Some(preferred);
        let old = coordinator.acquire(&agent.id).await.unwrap().unwrap();
        let original = old
            .run(store.claim_agent_work(&agent.id, "old-worker", NOW.into(), LEASE_END.into()))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(store.due_agent_work(NOW, 10).await.unwrap().is_empty());
        let receiver = Receiver::install(&old);
        assert_eq!(coordinator.retirement_background(&old).await.unwrap(), None);
        assert!(peer.acquire(&agent.id).await.unwrap().is_none());
        ownership
            .owners
            .update_one(
                doc! {"_id": agent.id.as_str()},
                doc! {"$set": {"expires_at": mongodb::bson::DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
        let next = tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                coordinator.maintain().await.unwrap();
                peer.maintain().await.unwrap();
                if let Some(next) = coordinator.acquire(&agent.id).await.unwrap() {
                    break next;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        assert!(next.token.epoch > old.token.epoch);
        assert_eq!(receiver.calls.load(Ordering::Acquire), 1);
        assert_eq!(
            store.due_agent_work(NOW, 10).await.unwrap(),
            vec![agent.id.clone()]
        );
        let claimed = next
            .run(store.claim_agent_work(
                &agent.id,
                "recovered-worker",
                NOW.into(),
                LEASE_END.into(),
            ))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let fence = claimed.work.lease.as_ref().unwrap().fence;
        assert_eq!(fence, original.work.lease.unwrap().fence + 1);
        assert!(old
            .run(store.claim_agent_work(&agent.id, "old-worker", NOW.into(), LEASE_END.into()))
            .await
            .is_err());
        let mut completed = claimed.clone();
        completed
            .work
            .advance("recovered-worker", fence, AgentWorkStep::None, NOW.into())
            .unwrap();
        completed.revision += 1;
        completed.updated_at = NOW.into();
        completed.conversation.last_seq += 1;
        next.run(store.commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "recovered-worker".into(),
            fence,
            agent: completed.clone(),
            worktree: None,
            appended_at: NOW.into(),
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::PublishConnection,
            }],
        }))
        .await
        .unwrap()
        .unwrap();
        let observed = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(observed, completed);
        let mut expected_conversation = original_conversation;
        expected_conversation.last_seq += 1;
        assert_eq!(observed.conversation, expected_conversation);
        let history = store.agent_log(&agent.id, 0, 10).await.unwrap();
        assert_eq!(&history[..original_history.len()], &original_history);
        assert_eq!(
            history.last().unwrap().kind,
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::PublishConnection,
            }
        );
        assert_eq!(observed.work.step, AgentWorkStep::None);
        assert!(store.due_agent_work(NOW, 10).await.unwrap().is_empty());
        for manager in [a, b] {
            let until = Instant::now() + Duration::from_secs(3);
            manager.prepare_drain(until).await.unwrap();
            for authority in manager.owned().await {
                manager
                    .drain_authority_until(&authority, until)
                    .await
                    .unwrap();
            }
            manager.abort_operations_until(until).await.unwrap();
        }
        client.database(&name).drop().await.unwrap();
    }
}
