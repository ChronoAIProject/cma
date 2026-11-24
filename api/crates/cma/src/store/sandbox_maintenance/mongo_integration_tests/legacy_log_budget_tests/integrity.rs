use super::*;
use crate::store::agent_capacity_recovery::source::sandbox_uses_segments;
use crate::store::agent_log_integrity::{
    empty_summary, prepare_append, AppendCapacity, INTEGRITY_FIELD, QUARANTINE_FIELD,
};
use crate::store::agent_log_segments::mongo_tests::fixtures as segments;
use layer2_core::sandbox::AgentLogCheckpoint;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_budget_keeps_zero_head_and_foreign_tail_errors_on_the_integrity_path() {
    let f = TestStore::open().await;
    for zero_head in [true, false] {
        let sandbox = active_sandbox(&format!("sbx_legacy_integrity_{zero_head}"));
        f.store.create_sandbox(sandbox.clone()).await.unwrap();
        let mut agent = ready_agent(&sandbox, &format!("agt_legacy_integrity_{zero_head}"));
        if zero_head {
            agent.conversation.last_seq = 0;
            agent.conversation.conversation_id = None;
            agent.conversation.runtime_generation = None;
        }
        let entries = small_history(&agent, 1);
        seed_legacy(&f.store, &agent, &entries).await;
        if !zero_head {
            let checkpoint = AgentLogCheckpoint::from_entries(&agent, &entries).unwrap();
            let integrity = prepare_append(
                &agent,
                &empty_summary(),
                &entries,
                agent.revision,
                checkpoint,
                AppendCapacity::Normal,
            )
            .unwrap()
            .integrity;
            f.store
                .agents
                .update_one(
                    doc! {"_id": agent.id.as_str()},
                    doc! {
                        "$set": {INTEGRITY_FIELD: integrity}
                    },
                )
                .await
                .unwrap();
            f.store
                .agent_log
                .update_one(
                    doc! {"agent_id": agent.id.as_str()},
                    doc! {"$set": {
                        "agent_id": "agt_foreign_tail",
                        "oversized": "x".repeat(MAX_LEGACY_BYTES as usize + 1),
                    }},
                )
                .await
                .unwrap();
        }
        f.commands();
        let request = f.request(&sandbox);
        assert!(matches!(
            f.store.preview_inactivity_pause(request.clone()).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        let commands = f.commands();
        let probe = commands
            .iter()
            .find(|cmd| cmd.contains_key("aggregate"))
            .unwrap();
        assert_probe(probe, &[agent.id.as_str()]);
        if !zero_head {
            let tail_filter = commands
                .iter()
                .filter_map(|cmd| cmd.get_document("filter").ok())
                .find(|filter| filter.contains_key("_id"))
                .unwrap();
            assert_eq!(tail_filter.get_str("agent_id").unwrap(), agent.id.as_str());
        }
        let read_agent = || f.store.agents.find_one(doc! {"_id": agent.id.as_str()});
        assert!(!read_agent()
            .await
            .unwrap()
            .unwrap()
            .contains_key(QUARANTINE_FIELD));
        assert!(matches!(
            f.store.schedule_inactivity_pause(request).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        assert!(read_agent()
            .await
            .unwrap()
            .unwrap()
            .contains_key(QUARANTINE_FIELD));
        assert_unchanged(&f.store, &sandbox).await;
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_budget_excludes_scoped_segments_and_proves_format_after_dispatch() {
    let f = TestStore::open().await;
    let sandbox = active_sandbox("sbx_legacy_mixed");
    f.store.create_sandbox(sandbox.clone()).await.unwrap();
    let mut segment_agent = ready_agent(&sandbox, "agt_budget_segmented");
    segment_agent.conversation.last_seq = (MAX_LEGACY_ROWS + 1) as u64;
    let entries = small_history(&segment_agent, segment_agent.conversation.last_seq);
    let (segment_agent, entries) =
        segments::install(&f.store, segment_agent, entries, 1, 192).await;
    segments::certify(&f.store, &segment_agent, &entries).await;

    // Place a valid v1 envelope in front of the dispatch lookup, then commit
    // the prepared v2 envelope before the unscoped transaction starts.
    let filter = doc! {"_id": segment_agent.id.as_str()};
    let adopted = f
        .store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    f.store
        .agents
        .replace_one(filter.clone(), agent_document(&segment_agent).unwrap())
        .await
        .unwrap();
    assert!(!sandbox_uses_segments(&f.store, &sandbox.id).await.unwrap());
    f.store.agents.replace_one(filter, adopted).await.unwrap();

    let request = f.request(&sandbox);
    // A false dispatch hint selected the unscoped entry point. The new
    // transaction must recognize the newly adopted v2 envelope first.
    let mut session = f.store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    f.commands();
    assert!(matches!(
        transaction::apply(&f.store, &MaintenanceRequest::Inactivity(request.clone()), &mut session).await,
        Err(ApplyError::Domain(SandboxStoreError::Unavailable(message)))
            if message.contains("exclusive transaction scope")
    ));
    assert!(f.commands().is_empty());
    session.abort_transaction().await.unwrap();
    assert_eq!(
        f.store
            .preview_inactivity_pause(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert!(!f.commands().iter().any(|cmd| cmd.contains_key("aggregate")));

    let legacy = ready_agent(&sandbox, "agt_budget_legacy");
    seed_legacy(&f.store, &legacy, &small_history(&legacy, 1)).await;
    f.commands();
    assert_eq!(
        f.store.preview_inactivity_pause(request).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let commands = f.commands();
    let probe = commands
        .iter()
        .find(|cmd| cmd.contains_key("aggregate"))
        .unwrap();
    assert_probe(probe, &[legacy.id.as_str()]);
    assert_unchanged(&f.store, &sandbox).await;
    f.cleanup().await;
}
