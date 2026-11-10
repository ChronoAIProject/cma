use super::*;

pub(super) const CLASS: &str = "agent_runtime_unavailable";
pub(super) const FIELD: &str = "worker_failure_pacing_v1";

pub(super) fn stamp(seconds: u64) -> String {
    layer2_core::rfc3339_at(
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(layer2_core::seconds_from_rfc3339(NOW).unwrap() + seconds),
    )
}

pub(super) fn scans() -> u64 {
    crate::store::agent_log_telemetry::snapshot(
        crate::store::agent_log_telemetry::ScanCaller::WorkerPublication(
            AgentWorkStep::PublishConnection,
        ),
        true,
    )[0]
}

pub(super) async fn observation(store: &MongoSandboxStore, agent: &Agent) -> AgentRetryObservation {
    AgentRetryObservation {
        agent_revision: agent.revision + 1,
        command_admission_generation: store
            .agent_command_admission_generation(&agent.id)
            .await
            .unwrap()
            .unwrap(),
        sandbox_revision: store
            .get_sandbox(&agent.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        workspace_revision: store
            .get_workspace(&agent.workspace_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        worktree_revision: store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
    }
}

pub(super) async fn fail(
    store: &MongoSandboxStore,
    id: &AgentId,
    seconds: u64,
    observed: Option<AgentRetryObservation>,
    class: &str,
) -> AgentWorkMutation {
    let claimed = store
        .claim_agent_work(id, "worker", stamp(seconds), stamp(seconds + 600))
        .await
        .unwrap()
        .unwrap();
    let mutation = retry_mutation(&claimed, seconds, class);
    match observed {
        Some(mut observed) => {
            observed.agent_revision = claimed.revision;
            store
                .commit_agent_work_retry(mutation.clone(), observed)
                .await
                .unwrap();
        }
        None => store.commit_agent_work(mutation.clone()).await.unwrap(),
    }
    mutation
}

pub(super) fn retry_mutation(claimed: &Agent, seconds: u64, class: &str) -> AgentWorkMutation {
    let mut next = claimed.clone();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    next.work
        .retry("worker", fence, stamp(seconds + 30))
        .unwrap();
    next.revision += 1;
    next.updated_at = stamp(seconds);
    next.conversation.last_seq += 1;
    AgentWorkMutation {
        expected_agent_revision: claimed.revision,
        holder: "worker".into(),
        fence,
        agent: next,
        appended_at: stamp(seconds),
        worktree: None,
        sandbox_event: None,
        agent_logs: vec![AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::PublishConnection,
            failure: TypedFailure {
                class: class.into(),
                detail: "read-only runtime inspection unavailable".into(),
            },
        }],
    }
}

pub(super) async fn until(store: &MongoSandboxStore, id: &AgentId) -> String {
    store
        .agents
        .find_one(doc! {"_id": id.as_str()})
        .await
        .unwrap()
        .unwrap()
        .get_document(FIELD)
        .unwrap()
        .get_str("until")
        .unwrap()
        .to_owned()
}
