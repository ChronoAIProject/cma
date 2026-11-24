use super::*;
use crate::store::agent_access_requests::AgentAccessProgress;
use layer2_core::sandbox::store::{SandboxEventStore, SandboxMaintenanceOutcome, SandboxStore};

async fn approved(f: &Fixture) -> AgentAccessRecord {
    let record = f
        .independent
        .request_agent_access(f.requester(), proposal())
        .await
        .unwrap();
    let mut approved_services = f.original_services();
    approved_services.push(ServiceId::new("github"));
    approved_services.sort();
    approved_services.dedup();
    f.independent
        .approve_agent_access(
            &record.requester.owner,
            &record.requester.agent_id,
            &record.id,
            approved_services,
            &[],
            AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "personal:retry-owner".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            "maintenance-approve",
        )
        .await
        .unwrap();
    let claimed = f
        .independent
        .claim_access_request("maintenance-worker")
        .await
        .unwrap()
        .unwrap();
    f.independent
        .advance_access_request(
            claimed.lease.as_ref().unwrap(),
            AgentAccessPhase::Connecting,
            AgentAccessPhase::WaitingQuiet,
            AgentAccessProgress::default(),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consented_maintenance_retries_lineage_abort_with_exact_approval_and_lease() {
    let f = Fixture::new(true).await;
    let approved = approved(&f).await;
    let lease = approved.lease.as_ref().unwrap();
    let parent = &approved.requester.sandbox_id;
    let events_before = f.independent.sandbox_events(parent, 0, 100).await.unwrap();
    f.abort("aggregate", Some("agents"), 1);
    let result = f.store.schedule_access_request_pause(lease).await;
    let (fired, commands) = f.clear().await;
    let record = f
        .independent
        .read_access_request(&approved.id)
        .await
        .unwrap()
        .unwrap();
    let parent_after = f.independent.get_sandbox(parent).await.unwrap();
    let events_after = f.independent.sandbox_events(parent, 0, 100).await.unwrap();
    let replay = f.independent.schedule_access_request_pause(lease).await;
    let parent_replay = f.independent.get_sandbox(parent).await.unwrap();
    let events_replay = f.independent.sandbox_events(parent, 0, 100).await.unwrap();
    f.cleanup().await;
    assert!(fired);
    let outcome = result.expect("known-aborted consented maintenance lineage must retry");
    assert_starts(&commands, 2);
    assert_eq!(record.requester, approved.requester);
    assert_eq!(record.decision, approved.decision);
    assert_eq!(record.approved_at, approved.approved_at);
    assert_eq!(record.lease, approved.lease);
    assert_eq!(parent_after, parent_replay);
    assert_eq!(events_after, events_replay);
    match outcome {
        SandboxMaintenanceOutcome::Scheduled => {
            assert_eq!(record.phase, AgentAccessPhase::Pausing);
            assert_eq!(replay.unwrap(), SandboxMaintenanceOutcome::AlreadyScheduled);
            assert_eq!(events_after.len(), events_before.len() + 1);
        }
        SandboxMaintenanceOutcome::NotEligible => {
            assert_eq!(record, approved);
            assert_eq!(replay.unwrap(), SandboxMaintenanceOutcome::NotEligible);
            assert_eq!(events_after, events_before);
        }
        other => panic!("unexpected fresh maintenance result: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consented_maintenance_retry_cannot_borrow_a_replaced_conversation() {
    let f = Fixture::new(true).await;
    let approved = approved(&f).await;
    f.independent
        .agents
        .update_one(
            doc! {"_id":approved.requester.agent_id.as_str()},
            doc! {"$set":{"record.conversation.conversation_id":"replacement-native"}},
        )
        .await
        .unwrap();
    let before = f
        .independent
        .get_sandbox(&approved.requester.sandbox_id)
        .await
        .unwrap();
    f.abort("aggregate", Some("agents"), 1);
    let result = f
        .store
        .schedule_access_request_pause(approved.lease.as_ref().unwrap())
        .await;
    let (fired, commands) = f.clear().await;
    let record = f
        .independent
        .read_access_request(&approved.id)
        .await
        .unwrap();
    let after = f
        .independent
        .get_sandbox(&approved.requester.sandbox_id)
        .await
        .unwrap();
    f.cleanup().await;
    assert!(fired);
    assert_eq!(result, Err(AgentAccessError::NotFound));
    assert_starts(&commands, 2);
    assert_eq!(record, Some(approved));
    assert_eq!(after, before);
}
