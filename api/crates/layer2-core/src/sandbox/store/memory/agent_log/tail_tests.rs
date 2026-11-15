use super::super::agent_admission_tests::{admission, created, ready_store, NOW};
use super::*;

async fn fixture() -> (InMemorySandboxStore, Agent) {
    let store = ready_store(1).await;
    let mut agent = created(store.admit_agent(admission("tail-proof")).await.unwrap()).agent;
    for _ in 0..4 {
        store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::ModeChanged {
                    mode: "code".into(),
                },
            )
            .await
            .unwrap();
    }
    agent.conversation.last_seq = 5;
    store.lock().agents.insert(agent.id.clone(), agent.clone());
    (store, agent)
}

async fn page(
    store: &InMemorySandboxStore,
    id: &AgentId,
    after: u64,
    through: u64,
    limit: u16,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let mut rows = Vec::new();
    store
        .visit_agent_log_page_with_tail(id, after, through, limit, &mut |entry| {
            rows.push(entry);
            Ok(())
        })
        .await?;
    Ok(rows)
}

#[tokio::test]
async fn tail_proof_retains_predecessor_and_frozen_range_after_committed_append() {
    let (store, agent) = fixture().await;
    let entries = store.agent_log(&agent.id, 0, 200).await.unwrap();
    crate::sandbox::store::agent_log_contract::assert_tail_page_contract(&store, &entries).await;
    assert_eq!(
        page(&store, &AgentId("absent".into()), 0, 1, 1).await,
        Err(SandboxStoreError::NotFound)
    );
    assert_eq!(
        store
            .visit_agent_log_page_with_tail(&agent.id, 0, 5, 1, &mut |_| {
                // Visitors run outside the store lock, including failure paths.
                let _guard = store.lock();
                Err(SandboxStoreError::InvalidPage)
            })
            .await,
        Err(SandboxStoreError::InvalidPage)
    );
}

#[tokio::test]
async fn tail_proof_refuses_corruption_outside_the_requested_page() {
    for fault in 0..7 {
        let (store, agent) = fixture().await;
        {
            let mut state = store.lock();
            let log = state.agent_logs.get_mut(&agent.id).unwrap();
            match fault {
                0 => {
                    log.pop();
                }
                1 => {
                    let mut row = log[4].clone();
                    row.seq = 6;
                    log.push(row);
                }
                2 => log[4].seq = 7,
                3 => log[4].agent_id = AgentId("foreign".into()),
                4 => {
                    log[0].kind = AgentLogKind::Created {
                        workspace_id: crate::WorkspaceId("foreign".into()),
                        worktree_id: agent.creation_worktree_id().clone(),
                    }
                }
                5 => log[4].kind = log[0].kind.clone(),
                _ => log[0].kind = log[4].kind.clone(),
            }
        }
        assert_eq!(
            page(&store, &agent.id, 1, 2, 1).await,
            Err(SandboxStoreError::SequenceConflict),
            "fault {fault}"
        );
    }
}
