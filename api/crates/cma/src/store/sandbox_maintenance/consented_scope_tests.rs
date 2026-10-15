//! Stale queued scope approvals must be rejected before shared Sandbox downtime.
use super::*;
use crate::store::agent_access_requests::{AgentAccessProgress, AgentAccessRecord};
use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_access_tool::AgentAccessProposal;
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStore};
use layer2_core::sandbox::SandboxWorkState;
use layer2_core::ServiceId;

async fn approve_calendar(
    store: &MongoSandboxStore,
    first: &AgentAccessRecord,
    key: &str,
    services: Vec<ServiceId>,
) -> AgentAccessRecord {
    let request = store
        .request_agent_access(
            first.requester.clone(),
            AgentAccessProposal {
                request_key: key.into(),
                services: vec!["calendar".into()],
                reason: "Read the owner's calendar".into(),
            },
        )
        .await
        .unwrap();
    store
        .approve_agent_access(
            &request.requester.owner,
            &request.requester.agent_id,
            &request.id,
            services,
            &[],
            AgentAuthorizationRevision {
                authority_id: "personal:fixture".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            key,
        )
        .await
        .unwrap()
}

// The store admits real pause/grant/revival transactions. Runtime effects and
// command delivery are fixture completions; this test does not contact a runtime.
async fn complete_first(store: &MongoSandboxStore, first: &AgentAccessRecord) {
    let lease = first.lease.as_ref().unwrap();
    store.schedule_access_request_pause(lease).await.unwrap();
    let mut sandbox = store
        .get_sandbox(&first.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    sandbox.status = SandboxStatus::Paused;
    sandbox.runtime = None;
    sandbox.lifecycle.operation = None;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    sandbox.revision += 1;
    revival_tests::save_parent(store, &sandbox).await;
    let record = store.read_access_request(&first.id).await.unwrap().unwrap();
    store
        .advance_access_request(
            lease,
            AgentAccessPhase::Pausing,
            AgentAccessPhase::Granting,
            record.progress,
        )
        .await
        .unwrap();
    store.grant_access_request_services(lease).await.unwrap();
    store.schedule_access_request_revival(lease).await.unwrap();
    let mut sandbox = store
        .get_sandbox(&first.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    sandbox.status = SandboxStatus::Active;
    sandbox.runtime = Some(first.requester.runtime.clone());
    sandbox.lifecycle.operation = None;
    sandbox.lifecycle.home_recovery = None;
    sandbox.lifecycle.paused_notice = None;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    sandbox.credential.key_generation += 1;
    let current = sandbox.credential.current.as_mut().unwrap();
    current.generation = sandbox.credential.key_generation;
    current.provider_key_id = "first-grant-delivered".into();
    sandbox.credential.delivered = true;
    sandbox.revision += 1;
    revival_tests::save_parent(store, &sandbox).await;
    let record = store.read_access_request(&first.id).await.unwrap().unwrap();
    let mut progress = record.progress;
    progress.delivered_credential = sandbox.credential.current.clone();
    progress.delivered_runtime = sandbox.runtime.clone();
    store
        .advance_access_request(
            lease,
            AgentAccessPhase::Reviving,
            AgentAccessPhase::Notifying,
            progress.clone(),
        )
        .await
        .unwrap();
    progress.notification_command_id = Some(AgentCommandId("first-notification-delivered".into()));
    store
        .advance_access_request(
            lease,
            AgentAccessPhase::Notifying,
            AgentAccessPhase::Notifying,
            progress.clone(),
        )
        .await
        .unwrap();
    store
        .advance_access_request(
            lease,
            AgentAccessPhase::Notifying,
            AgentAccessPhase::Succeeded,
            progress,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn stale_additive_approval_cannot_pause_after_a_different_request_expands_scope() {
    let (client, name, store, first) = access::tests::approved_fixture().await;
    let second = approve_calendar(
        &store,
        &first,
        "second-calendar",
        vec![ServiceId::new("calendar")],
    )
    .await;
    assert_eq!(
        first.approved_services().unwrap(),
        &[ServiceId::new("github")]
    );
    assert_eq!(
        second.approved_services().unwrap(),
        &[ServiceId::new("calendar")]
    );
    complete_first(&store, &first).await;
    let claimed = store
        .claim_access_request("second-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, second.id);
    let lease = claimed.lease.as_ref().unwrap();
    let waiting = store
        .advance_access_request(
            lease,
            AgentAccessPhase::Connecting,
            AgentAccessPhase::WaitingQuiet,
            AgentAccessProgress::default(),
        )
        .await
        .unwrap();
    let before = store
        .get_sandbox(&first.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.status, SandboxStatus::Active);
    assert_eq!(
        before.credential.effective_reaches(),
        vec![ServiceId::new("github")]
    );
    let events = store.sandbox_events(&before.id, 0, 100).await.unwrap();
    assert_eq!(
        store.schedule_access_request_pause(lease).await,
        Err(AgentAccessError::Invalid)
    );
    // The entire transaction rolls back, including any credential or pause event.
    assert_eq!(
        store.get_sandbox(&before.id).await.unwrap(),
        Some(before.clone())
    );
    assert_eq!(
        store.sandbox_events(&before.id, 0, 100).await.unwrap(),
        events
    );
    assert_eq!(
        store.read_access_request(&second.id).await.unwrap(),
        Some(waiting)
    );
    // Invalid is terminal in the access worker. It cannot leave the Sandbox paused
    // or silently widen this approval to include the first request's service.
    let failed = store
        .fail_access_request(lease, "access_update_cannot_continue")
        .await
        .unwrap();
    assert_eq!(failed.phase, AgentAccessPhase::Failed);
    assert_eq!(failed.decision, second.decision);
    assert_eq!(failed.requester, second.requester);
    assert!(failed.progress.pause_operation_id.is_none());
    assert_eq!(
        store.get_sandbox(&before.id).await.unwrap(),
        Some(before.clone())
    );

    // A genuinely new owner approval that includes the current scope can proceed.
    let fresh = approve_calendar(
        &store,
        &first,
        "calendar-with-current-scope",
        vec![ServiceId::new("calendar"), ServiceId::new("github")],
    )
    .await;
    let claimed = store
        .claim_access_request("fresh-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, fresh.id);
    let lease = claimed.lease.as_ref().unwrap();
    store
        .advance_access_request(
            lease,
            AgentAccessPhase::Connecting,
            AgentAccessPhase::WaitingQuiet,
            AgentAccessProgress::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.schedule_access_request_pause(lease).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    client.database(&name).drop().await.unwrap();
}
