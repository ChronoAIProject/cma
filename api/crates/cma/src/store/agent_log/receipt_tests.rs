use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent};
use layer2_core::sandbox::store::AgentStore;
use layer2_core::sandbox::AgentWorkStep;

const AT: &str = "2026-09-05T00:00:00Z";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn single_append_receipt_refuses_absent_mismatched_and_uncommitted_rows() {
    for enabled in [false, true] {
        let client =
            mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
                .await
                .unwrap();
        let name = format!("cma_receipt_negative_{}", uuid::Uuid::new_v4().simple());
        let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mut agent = ready_agent(&active_sandbox("sbx_receipt"), "agt_receipt");
        agent.conversation.last_seq = 0;
        agent.conversation.conversation_id = None;
        agent.conversation.runtime_generation = None;
        store.create_agent(agent.clone()).await.unwrap();
        let created = store
            .append_agent_log(
                &agent.id,
                AT.into(),
                AgentLogKind::Created {
                    workspace_id: agent.workspace_id.clone(),
                    worktree_id: agent.creation_worktree_id().clone(),
                },
            )
            .await
            .unwrap();
        if enabled {
            store = store
                .prepare_bounded_admission(std::slice::from_ref(&agent.id))
                .await
                .unwrap();
        }
        let mut observed = client.start_session().snapshot(true).await.unwrap();
        store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .read_concern(ReadConcern::snapshot())
            .session(&mut observed)
            .await
            .unwrap();
        let at = observed.snapshot_time().unwrap();
        let expected = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: 2,
            appended_at: AT.into(),
            kind: AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None,
            },
        };
        let mut absent_agent = expected.clone();
        absent_agent.agent_id = AgentId("agt_absent_receipt".into());
        for absent in [&absent_agent, &expected] {
            assert!(reconcile_append_after_unknown(&store, absent, at)
                .await
                .unwrap()
                .is_none());
        }
        let mut mismatch = created.clone();
        mismatch.appended_at = "2026-09-05T00:00:01Z".into();
        assert!(matches!(
            reconcile_append_after_unknown(&store, &mismatch, at).await,
            Err(SandboxStoreError::SequenceConflict)
        ));
        // The real append transaction has written the row and summary, but
        // neither an older pinned snapshot nor an aborted write is a receipt.
        let mut writing = client.start_session().await.unwrap();
        start_transaction(&mut writing).await.unwrap();
        let pending = append_in_transaction(
            &store,
            &mut writing,
            &agent.id,
            AT.into(),
            expected.kind.clone(),
        )
        .await
        .unwrap();
        assert_eq!(pending, expected);
        assert!(reconcile_append_after_unknown(&store, &expected, at)
            .await
            .unwrap()
            .is_none());
        writing.abort_transaction().await.unwrap();
        let mut after_abort = client.start_session().snapshot(true).await.unwrap();
        store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .read_concern(ReadConcern::snapshot())
            .session(&mut after_abort)
            .await
            .unwrap();
        assert!(reconcile_append_after_unknown(
            &store,
            &expected,
            after_abort.snapshot_time().unwrap(),
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(
            store
                .agent_log
                .count_documents(doc! { "agent_id": agent.id.as_str() })
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .get_agent(&agent.id)
                .await
                .unwrap()
                .unwrap()
                .conversation
                .last_seq,
            1
        );
        assert_eq!(
            reconcile_append_after_unknown(&store, &created, at)
                .await
                .unwrap(),
            Some(created)
        );
        client.database(&name).drop().await.unwrap();
    }
}
