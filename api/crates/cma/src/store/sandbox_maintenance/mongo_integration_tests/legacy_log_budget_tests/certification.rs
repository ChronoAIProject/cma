use super::*;
use layer2_core::sandbox::{AgentWorkStep, InactivityPauseRefusal};

const PROOF: &str = "inactivity_log_proof_v1";

async fn seed(
    f: &TestStore,
    id: &str,
    rows: u64,
    modify: impl FnOnce(&mut [AgentLogEntry]),
) -> (Sandbox, Agent) {
    let sandbox = active_sandbox(id);
    f.store.create_sandbox(sandbox.clone()).await.unwrap();
    let mut agent = ready_agent(&sandbox, &format!("agt_{id}"));
    agent.conversation.last_seq = rows;
    let mut entries = small_history(&agent, rows);
    modify(&mut entries);
    seed_legacy(&f.store, &agent, &entries).await;
    (sandbox, agent)
}

async fn proof(f: &TestStore, agent: &Agent) -> Option<Document> {
    f.store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap()
        .get_document(PROOF)
        .ok()
        .cloned()
}

async fn refusal(f: &TestStore, sandbox: &Sandbox) -> Option<InactivityPauseRefusal> {
    f.store
        .preview_inactivity_pause_with_reason(f.request(sandbox))
        .await
        .unwrap()
        .refusal
}

async fn automatic(f: &TestStore, sandbox: &Sandbox) -> SandboxMaintenanceOutcome {
    f.store
        .schedule_inactivity_pause(SandboxInactivityPauseRequest {
            expected_policy_revision: None,
            ..f.request(sandbox)
        })
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn dormant_legacy_log_certifies_across_sweeps_and_preview_is_read_only() {
    let f = TestStore::open().await;
    let (sandbox, agent) = seed(&f, "sbx_dormant_legacy", 257, |_| {}).await;
    let (recent, _) = seed(&f, "sbx_recent_publication", 3, |entries| {
        entries[2].kind = AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::PublishConnection,
        };
        entries[2].appended_at = LATER.into();
    })
    .await;
    assert_eq!(
        refusal(&f, &sandbox).await,
        Some(InactivityPauseRefusal::LegacyLogExpectedRows)
    );
    assert_eq!(
        refusal(&f, &recent).await,
        Some(InactivityPauseRefusal::RecentAgentActivity)
    );
    assert!(proof(&f, &agent).await.is_none());
    for through in [128, 256, 257] {
        assert_eq!(
            automatic(&f, &sandbox).await,
            SandboxMaintenanceOutcome::NotEligible
        );
        let saved = proof(&f, &agent).await.unwrap();
        assert_eq!(saved.get_i64("through").unwrap(), through);
        assert_eq!(
            refusal(&f, &sandbox).await,
            (through < 257).then_some(InactivityPauseRefusal::LegacyLogProofPending)
        );
        assert_eq!(proof(&f, &agent).await, Some(saved));
        assert_unchanged(&f.store, &sandbox).await;
    }
    assert_eq!(
        automatic(&f, &sandbox).await,
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(f.store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        f.store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_unchanged(&f.store, &recent).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn dormant_legacy_certification_restarts_after_an_append_and_honours_sessions() {
    let f = TestStore::open().await;
    let (sandbox, agent) = seed(&f, "sbx_dormant_append", 257, |_| {}).await;
    assert_eq!(
        automatic(&f, &sandbox).await,
        SandboxMaintenanceOutcome::NotEligible
    );
    let mut next = agent.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    let mut entry = log(
        &agent,
        next.conversation.last_seq,
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::PublishConnection,
        },
    );
    entry.appended_at = LATER.into();
    let mut session = f.store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    f.store
        .agent_log
        .insert_one(log_document(&entry).unwrap())
        .session(&mut session)
        .await
        .unwrap();
    let mut update = agent_document(&next).unwrap();
    update.remove("_id");
    f.store
        .agents
        .update_one(doc! {"_id": agent.id.as_str()}, doc! {"$set": update})
        .session(&mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();

    assert_eq!(
        refusal(&f, &sandbox).await,
        Some(InactivityPauseRefusal::LegacyLogExpectedRows)
    );
    for through in [128, 256, 258] {
        assert_eq!(
            automatic(&f, &sandbox).await,
            SandboxMaintenanceOutcome::NotEligible
        );
        let saved = proof(&f, &next).await.unwrap();
        assert_eq!(saved.get_i64("head").unwrap(), 258);
        assert_eq!(saved.get_i64("through").unwrap(), through);
    }
    assert_eq!(
        refusal(&f, &sandbox).await,
        Some(InactivityPauseRefusal::RecentAgentActivity)
    );
    f.store
        .open_sandbox_session(&sandbox.id, "desktop", "2026-09-05T01:00:00Z")
        .await
        .unwrap();
    assert_eq!(
        refusal(&f, &sandbox).await,
        Some(InactivityPauseRefusal::OpenSession)
    );
    assert_unchanged(&f.store, &sandbox).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn dormant_legacy_certification_handles_adjacent_large_rows_and_reports_byte_limit() {
    let f = TestStore::open().await;
    let row_bytes = (MAX_LEGACY_BYTES / 2 + 100_000) as usize;
    let (sandbox, agent) = seed(&f, "sbx_dormant_large_rows", 5, |entries| {
        for entry in &mut entries[2..4] {
            entry.kind = AgentLogKind::AgentMessage {
                content: "x".repeat(row_bytes),
            };
        }
    })
    .await;
    assert_eq!(
        automatic(&f, &sandbox).await,
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        proof(&f, &agent).await.unwrap().get_i64("through").unwrap(),
        3
    );
    assert_eq!(
        automatic(&f, &sandbox).await,
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(refusal(&f, &sandbox).await, None);
    assert_unchanged(&f.store, &sandbox).await;

    let (oversize, _) = seed(&f, "sbx_dormant_oversize_row", 4, |entries| {
        entries[2].kind = AgentLogKind::AgentMessage {
            content: "x".repeat(MAX_LEGACY_BYTES as usize + 1),
        };
    })
    .await;
    for _ in 0..2 {
        assert_eq!(
            automatic(&f, &oversize).await,
            SandboxMaintenanceOutcome::NotEligible
        );
    }
    assert_eq!(
        refusal(&f, &oversize).await,
        Some(InactivityPauseRefusal::LegacyLogProofByteLimit)
    );
    assert_unchanged(&f.store, &oversize).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn two_certified_legacy_heads_above_the_payload_budget_can_pause() {
    let f = TestStore::open().await;
    let row_bytes = (MAX_LEGACY_BYTES / 2 + 100_000) as usize;
    let large_tail = |entries: &mut [AgentLogEntry]| {
        entries[2].kind = AgentLogKind::AgentMessage {
            content: "x".repeat(row_bytes),
        };
    };
    let (sandbox, first) = seed(&f, "sbx_dormant_large_tails", 3, large_tail).await;
    let mut second = ready_agent(&sandbox, "agt_dormant_large_tail_second");
    second.conversation.last_seq = 3;
    let mut entries = small_history(&second, 3);
    large_tail(&mut entries);
    seed_legacy(&f.store, &second, &entries).await;
    assert_eq!(
        refusal(&f, &sandbox).await,
        Some(InactivityPauseRefusal::LegacyLogBytes)
    );
    for _ in 0..2 {
        assert_eq!(
            automatic(&f, &sandbox).await,
            SandboxMaintenanceOutcome::NotEligible
        );
    }
    assert_eq!(
        proof(&f, &first).await.unwrap().get_str("state").unwrap(),
        "certified"
    );
    assert_eq!(
        proof(&f, &second).await.unwrap().get_str("state").unwrap(),
        "certified"
    );
    f.commands();
    assert_eq!(refusal(&f, &sandbox).await, None);
    let commands = f.commands();
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.get_document("projection").is_ok())
            .count(),
        2
    );
    assert!(
        !commands
            .iter()
            .any(|command| command.contains_key("find")
                && command.get_document("projection").is_err())
    );
    assert_unchanged(&f.store, &sandbox).await;
    assert_eq!(
        automatic(&f, &sandbox).await,
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(
        f.store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    f.cleanup().await;
}
