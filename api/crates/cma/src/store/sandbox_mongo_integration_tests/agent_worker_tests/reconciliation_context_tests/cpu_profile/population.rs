use super::*;

pub(super) async fn seed(store: &MongoSandboxStore) -> Vec<Agent> {
    let mut live = Vec::new();
    // The observed live mix, rounded from 49 candidates to 50: 33 attached,
    // five paused, ten closing, and two already awaiting connection recovery.
    for n in 0..50 {
        let status = if n < 25 {
            AgentStatus::Ready
        } else if n < 33 {
            AgentStatus::Working
        } else if n < 38 {
            AgentStatus::Paused
        } else {
            AgentStatus::Ready
        };
        let mut agent = idle_connection_fixture(store, &format!("cpu_{n:04}"), status).await;
        agent.conversation.managed_guidance_revision = 2;
        if n >= 38 {
            agent.work.schedule(
                if n < 48 {
                    AgentWorkStep::CloseProcess
                } else {
                    AgentWorkStep::PublishConnection
                },
                NOW.into(),
            );
        }
        agent.revision += 1;
        replace_agent_fixture(store, &agent).await;
        live.push(agent);
    }
    // Retained/failed rows participate in indexed discovery but not live scans.
    for n in 50..1402 {
        let mut agent = live[0].clone();
        agent.id = AgentId(format!("agt_cpu_retained_{n:04}"));
        agent.worktree_id = WorktreeId(format!("wkt_cpu_retained_{n:04}"));
        agent.creation_key = format!("cpu-retained-{n}");
        agent.owner = SubjectId(format!("owner-{}", n % 12));
        agent.conversation.active_turn = None;
        agent.work.schedule(AgentWorkStep::None, NOW.into());
        if n < 84 {
            agent.status = AgentStatus::Failed;
            agent
                .work
                .schedule(AgentWorkStep::RemoveWorktree, NOW.into());
            agent.work.attempts = 100;
        } else if n < 86 {
            agent.status = AgentStatus::Failed;
            agent
                .work
                .schedule(AgentWorkStep::EnsureProcessClosed, NOW.into());
        } else if n < 107 {
            agent.status = AgentStatus::Failed;
        } else if n < 271 {
            agent.status = AgentStatus::Stopped;
        } else {
            agent.status = AgentStatus::Deleted;
        }
        store
            .agents
            .insert_one(crate::store::agents::agent_document(&agent).unwrap())
            .await
            .unwrap();
    }
    // The live fixture contributes 50 parents; add 289 retained parents.
    for n in 50..339 {
        let mut parent = retired_sandbox(&format!("sbx_cpu_retained_{n:04}"));
        parent.owner = SubjectId(format!("owner-{}", n % 12));
        parent.name = format!("cpu-retained-{n}");
        store.create_sandbox(parent).await.unwrap();
    }
    live
}
