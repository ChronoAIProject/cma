use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::agent_capacity_recovery::ports::ClaimOutcome;

pub(super) fn after(at: &str, seconds: u64) -> String {
    layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(
                layer2_core::seconds_from_rfc3339(at).unwrap() + seconds,
            ),
    )
}

pub(super) async fn claim(
    store: &MongoSandboxStore,
    caller: &Caller,
    request: &RecoveryRequest,
    at: &str,
) -> RecoveryClaim {
    let ClaimOutcome::Claimed(claim) = service(store, false)
        .claim(caller, request, "quota-worker", at, &after(at, 60))
        .await
        .unwrap()
    else {
        panic!("expected a fresh recovery claim")
    };
    *claim
}

pub(super) fn confirmation(claim: &RecoveryClaim) -> RecoveryConfirmation {
    RecoveryConfirmation {
        operation_key: claim.work.request.key().unwrap(),
        target: claim.work.target.clone(),
        authority_fence: claim.work.lease.as_ref().unwrap().fence,
        native_state: ConfirmedNativeState::Idle,
    }
}

pub(super) async fn complete(
    store: &MongoSandboxStore,
    caller: &Caller,
    claim: &RecoveryClaim,
    at: &str,
) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
    service(store, false)
        .complete(caller, claim, &confirmation(claim), at)
        .await
}

pub(super) async fn second_recovery(
    store: &MongoSandboxStore,
    request: &RecoveryRequest,
) -> (Caller, RecoveryRequest, RecoveryWork) {
    let agent = ordinary(store, request, "second-recovery").await;
    let worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let claimed = store
        .claim_agent_work(&agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let ready = fixture::ready_claimed(store, worktree, claimed, "second-recovery").await;
    let failed = fixture::prepare_capacity(store, ready).await;
    admit_fixture(store, failed).await
}

pub(super) async fn release(store: &MongoSandboxStore, mut agent: Agent) {
    agent
        .transition(AgentStatus::Failed, layer2_core::now_rfc3339(), None)
        .unwrap();
    agent.failure = Some(TypedFailure {
        class: "synthetic_start_failure".into(),
        detail: "release test reservation".into(),
    });
    agent.work.step = AgentWorkStep::None;
    agent.work.lease = None;
    replace_agent_fixture(store, &agent).await;
}

// Compare every document involved in recovery publication, including parent
// fences, from one snapshot. Refusal/abort must not leave a partial write.
pub(super) async fn durable(store: &MongoSandboxStore) -> Vec<Vec<Document>> {
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let carrier = records::collection(store);
    let mut groups = Vec::new();
    for collection in [
        &store.agents,
        &store.sandboxes,
        &store.workspaces,
        &store.worktrees,
        &store.agent_log,
        &store.segment_logs.heads,
        &store.segment_logs.projections,
        &store.segment_logs.publications,
        &store.segment_logs.seals,
        &store.segment_logs.evidence,
        &carrier,
    ] {
        let mut cursor = collection
            .find(doc! {})
            .sort(doc! {"_id":1})
            .session(&mut session)
            .await
            .unwrap();
        groups.push(cursor.stream(&mut session).try_collect().await.unwrap());
    }
    session.commit_transaction().await.unwrap();
    groups
}
