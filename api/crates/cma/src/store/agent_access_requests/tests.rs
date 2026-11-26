use super::*;
use layer2_core::sandbox::store::{AgentProvenanceStore, SandboxStore};
use layer2_core::sandbox::ServiceConnectionSelection;
use std::sync::Arc;

pub(crate) async fn approved_fixture() -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentAccessRecord,
) {
    approved_fixture_with_connection(None).await
}

pub(super) async fn approved_fixture_with_connection(
    connection: Option<ServiceConnectionSelection>,
) -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentAccessRecord,
) {
    let (client, database, store, state) =
        crate::store::agent_access_tools::tests::persisted().await;
    let creation = state.creation.unwrap();
    crate::store::agent_access_tools::tests::publish(&store, &creation.agent_id).await;
    let state = store
        .agent_provenance_snapshot(&creation.agent_id)
        .await
        .unwrap();
    let requester = AgentAccessRequester {
        agent_id: creation.agent_id.clone(),
        owner: state.resource.lineage.sandbox.owner.clone(),
        sandbox_id: creation.sandbox_id.clone(),
        conversation: "original-native".into(),
        runtime: state.resource.lineage.sandbox.runtime.clone().unwrap(),
        creation,
    };
    let mut record = store
        .request_agent_access(
            requester,
            AgentAccessProposal {
                retry_of: None,
                request_key: "access-request-fixture".into(),
                services: vec!["github".into()],
                reason: "Read owner repository".into(),
            },
        )
        .await
        .unwrap();
    record = store
        .approve_agent_access_with_connection(
            &record.requester.owner,
            &record.requester.agent_id,
            &record.id,
            vec![ServiceId::new("github")],
            connection,
            &[],
            AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "personal:fixture".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            "approve-fixture",
        )
        .await
        .unwrap();
    let claimed = store
        .claim_access_request("access-fixture-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, record.id);
    record = store
        .advance_access_request(
            claimed.lease.as_ref().unwrap(),
            AgentAccessPhase::Connecting,
            AgentAccessPhase::WaitingQuiet,
            AgentAccessProgress::default(),
        )
        .await
        .unwrap();
    (client, database, store, record)
}

pub(crate) async fn approved_scoped_fixture() -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentAccessRecord,
) {
    let (client, database, store, record) = approved_fixture().await;
    let mut sandbox = store
        .get_sandbox(&record.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    // Owner replacement compares a nonempty current scope. The generic tool
    // proposal fixture starts empty and is not a valid replacement request.
    sandbox.credential.reaches = vec![ServiceId::new("original-service")];
    sandbox.validate().unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id":sandbox.id.as_str()},
            doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap()},
        )
        .await
        .unwrap();
    (client, database, store, record)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_requests_replay_scope_approval_and_owner_boundaries() {
    let (client, database, store, first) = approved_fixture().await;
    let requester = first.requester.clone();
    let mut proposal = first.proposal.clone();
    proposal.request_key = "proposal-two".into();
    proposal.services = vec!["github".into(), "calendar".into()];
    let record = store
        .request_agent_access(requester.clone(), proposal.clone())
        .await
        .unwrap();
    let mut reordered = proposal.clone();
    reordered.services.reverse();
    assert_eq!(
        store
            .request_agent_access(requester.clone(), reordered)
            .await
            .unwrap()
            .id,
        record.id
    );
    let mut changed = proposal.clone();
    changed.reason = "Different purpose".into();
    assert_eq!(
        store.request_agent_access(requester.clone(), changed).await,
        Err(AgentAccessError::Conflict)
    );
    let authority = AgentAuthorizationRevision {
        replay_principal: None,
        authority_id: "personal:owner-login".into(),
        policy_revision: 1,
        grant_revision: None,
    };
    assert_eq!(
        store
            .approve_agent_access(
                &requester.owner,
                &requester.agent_id,
                &record.id,
                vec![ServiceId::new("github")],
                &[],
                authority.clone(),
                "approve-two"
            )
            .await,
        Err(AgentAccessError::Invalid)
    );
    assert_eq!(
        store
            .approve_agent_access(
                &requester.owner,
                &requester.agent_id,
                &record.id,
                vec![ServiceId::new("calendar"), ServiceId::new("github")],
                &[ServiceId::new("baseline")],
                authority.clone(),
                "approve-two"
            )
            .await,
        Err(AgentAccessError::Invalid)
    );
    let scopes = vec![
        ServiceId::new("baseline"),
        ServiceId::new("calendar"),
        ServiceId::new("github"),
    ];
    let approved = store
        .approve_agent_access(
            &requester.owner,
            &requester.agent_id,
            &record.id,
            scopes.clone(),
            &[ServiceId::new("baseline")],
            authority.clone(),
            "approve-two",
        )
        .await
        .unwrap();
    let replay = store
        .approve_agent_access(
            &requester.owner,
            &requester.agent_id,
            &record.id,
            scopes.clone(),
            &[ServiceId::new("baseline")],
            authority.clone(),
            "approve-two",
        )
        .await
        .unwrap();
    assert_eq!(approved, replay);
    assert_eq!(
        store
            .access_request_connection_services(&requester.owner, &record.id)
            .await
            .unwrap(),
        scopes
    );
    assert_eq!(
        store
            .access_request_connection_services(&SubjectId::new("foreign"), &record.id)
            .await,
        Err(AgentAccessError::NotFound)
    );
    assert_eq!(
        store
            .list_agent_access_requests(&SubjectId::new("foreign"), &requester.agent_id)
            .await,
        Err(AgentAccessError::NotFound)
    );
    assert_eq!(
        store
            .deny_agent_access(
                &requester.owner,
                &requester.agent_id,
                &first.id,
                "deny-applying"
            )
            .await,
        Err(AgentAccessError::Conflict)
    );
    let denied = store
        .deny_agent_access(
            &requester.owner,
            &requester.agent_id,
            &record.id,
            "deny-two",
        )
        .await
        .unwrap();
    assert_eq!(denied.phase, AgentAccessPhase::Denied);
    assert_eq!(
        store
            .deny_agent_access(
                &requester.owner,
                &requester.agent_id,
                &record.id,
                "deny-two"
            )
            .await
            .unwrap(),
        denied
    );
    assert_eq!(
        store
            .access_request_connection_services(&requester.owner, &record.id)
            .await,
        Err(AgentAccessError::Conflict)
    );
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened.read_access_request(&record.id).await.unwrap(),
        Some(denied)
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_requests_bound_capacity_and_fence_worker_takeovers() {
    let (client, database, store, first) = approved_fixture().await;
    let requester = first.requester.clone();
    let lease = first.lease.clone().unwrap();
    let renewed = store.renew_access_lease(&lease).await.unwrap();
    assert_eq!(renewed.token, lease.token);
    store
        .release_access_request(&lease, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(store.renew_access_lease(&lease).await.is_err());
    let reclaimed = store
        .claim_access_request("replacement-worker")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(reclaimed.lease.as_ref().unwrap().token, lease.token);
    assert!(store
        .fail_access_request(&lease, "stale_worker")
        .await
        .is_err());
    for index in 0..3 {
        let mut proposal = first.proposal.clone();
        proposal.request_key = format!("extra-{index}");
        store
            .request_agent_access(requester.clone(), proposal)
            .await
            .unwrap();
    }
    let mut overflow = first.proposal.clone();
    overflow.request_key = "over-capacity".into();
    assert_eq!(
        store
            .request_agent_access(requester.clone(), overflow)
            .await,
        Err(AgentAccessError::Conflict)
    );
    let failed = store
        .fail_access_request(reclaimed.lease.as_ref().unwrap(), "owner_authority_revoked")
        .await
        .unwrap();
    assert_eq!(failed.phase, AgentAccessPhase::Failed);
    assert!(store
        .renew_access_lease(reclaimed.lease.as_ref().unwrap())
        .await
        .is_err());
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_requests_never_move_two_sandbox_requests_into_applying() {
    let (client, database, store, first) = approved_fixture().await;
    let mut proposal = first.proposal.clone();
    proposal.request_key = "second-applying".into();
    let mut record = store
        .request_agent_access(first.requester.clone(), proposal)
        .await
        .unwrap();
    record = store
        .approve_agent_access(
            &record.requester.owner,
            &record.requester.agent_id,
            &record.id,
            vec![ServiceId::new("github")],
            &[],
            AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "personal:fixture".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            "approve-second",
        )
        .await
        .unwrap();
    let claimed = store
        .claim_access_request("another-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, record.id);
    assert_eq!(
        store
            .advance_access_request(
                claimed.lease.as_ref().unwrap(),
                AgentAccessPhase::Connecting,
                AgentAccessPhase::WaitingQuiet,
                AgentAccessProgress::default()
            )
            .await,
        Err(AgentAccessError::Conflict)
    );
    store
        .fail_access_request(first.lease.as_ref().unwrap(), "owner_authority_revoked")
        .await
        .unwrap();
    assert_eq!(
        store
            .advance_access_request(
                claimed.lease.as_ref().unwrap(),
                AgentAccessPhase::Connecting,
                AgentAccessPhase::WaitingQuiet,
                AgentAccessProgress::default()
            )
            .await
            .unwrap()
            .phase,
        AgentAccessPhase::WaitingQuiet
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_requests_concurrent_admission_holds_capacity_and_runtime_identity() {
    let (client, name, store, first) = approved_fixture().await;
    let attempts = (0..8).map(|index| {
        let store = store.clone();
        let identity = first.requester.clone();
        let mut proposal = first.proposal.clone();
        proposal.request_key = format!("concurrent-{index}");
        async move { store.request_agent_access(identity, proposal).await }
    });
    let results = futures_util::future::join_all(attempts).await;
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 3);
    assert_eq!(
        store
            .list_agent_access_requests(&first.requester.owner, &first.requester.agent_id)
            .await
            .unwrap()
            .len(),
        4
    );
    for index in 0..5 {
        let mut requester = first.requester.clone();
        match index {
            0 => requester.conversation = "another-native".into(),
            1 => requester.creation.creation_fingerprint = "d".repeat(64),
            2 => requester.owner = SubjectId::new("foreign"),
            3 => requester.runtime.boot_id = Some("e".repeat(32)),
            _ => requester.runtime.provision_generation = Some(2),
        }
        assert_eq!(
            store
                .request_agent_access(requester, first.proposal.clone())
                .await,
            Err(AgentAccessError::NotFound)
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_requests_concurrent_applying_is_serialized_by_parent_fence() {
    let (client, name, store, first) = approved_fixture().await;
    store
        .fail_access_request(first.lease.as_ref().unwrap(), "test_cleanup")
        .await
        .unwrap();
    let mut claims = vec![];
    for index in 0..2 {
        let mut proposal = first.proposal.clone();
        proposal.request_key = format!("apply-{index}");
        let record = store
            .request_agent_access(first.requester.clone(), proposal)
            .await
            .unwrap();
        store
            .approve_agent_access(
                &record.requester.owner,
                &record.requester.agent_id,
                &record.id,
                vec![ServiceId::new("github")],
                &[],
                AgentAuthorizationRevision {
                    replay_principal: None,
                    authority_id: "personal:fixture".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
                &format!("approve-{index}"),
            )
            .await
            .unwrap();
        claims.push(
            store
                .claim_access_request(&format!("worker-{index}"))
                .await
                .unwrap()
                .unwrap(),
        );
    }
    let attempts = claims.into_iter().map(|record| {
        let store = store.clone();
        async move {
            store
                .advance_access_request(
                    record.lease.as_ref().unwrap(),
                    AgentAccessPhase::Connecting,
                    AgentAccessPhase::WaitingQuiet,
                    record.progress.clone(),
                )
                .await
        }
    });
    let results = futures_util::future::join_all(attempts).await;
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let applying = store
        .list_agent_access_requests(&first.requester.owner, &first.requester.agent_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|record| record.phase.applying())
        .collect::<Vec<_>>();
    assert_eq!(applying.len(), 1);
    assert_eq!(
        applying[0].view().status,
        layer2_core::sandbox::agent_access::AgentAccessStatus::Applying
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_requests_expire_pending_and_reject_unproved_phase_changes() {
    let (client, name, store, first) = approved_fixture().await;
    let lease = first.lease.as_ref().unwrap();
    assert_eq!(
        store
            .advance_access_request(
                lease,
                AgentAccessPhase::WaitingQuiet,
                AgentAccessPhase::Pausing,
                AgentAccessProgress::default()
            )
            .await,
        Err(AgentAccessError::Invalid)
    );
    store.schedule_access_request_pause(lease).await.unwrap();
    let pausing = store.read_access_request(&first.id).await.unwrap().unwrap();
    assert_eq!(
        store
            .advance_access_request(
                lease,
                AgentAccessPhase::Pausing,
                AgentAccessPhase::Granting,
                pausing.progress
            )
            .await,
        Err(AgentAccessError::Conflict)
    );
    // A new proposal remains admissible only from its current runtime; fixture
    // admission below precedes expiry and does not borrow the paused runtime.
    let mut expired = first.clone();
    expired.proposal.request_key = "expired-pending".into();
    expired.id = records::request_id(&expired.requester, &expired.proposal.request_key).unwrap();
    expired.body_sha256 = records::digest(&expired.proposal).unwrap();
    expired.created_at = layer2_core::rfc3339_at(
        std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600),
    );
    expired.updated_at = expired.created_at.clone();
    expired.phase = AgentAccessPhase::Pending;
    expired.lease = None;
    expired.decision = None;
    expired.approved_at = None;
    expired.progress = AgentAccessProgress::default();
    store
        .agent_access_requests
        .insert_one(document(&expired).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store
            .approve_agent_access(
                &expired.requester.owner,
                &expired.requester.agent_id,
                &expired.id,
                vec![ServiceId::new("github")],
                &[],
                AgentAuthorizationRevision {
                    replay_principal: None,
                    authority_id: "personal:fixture".into(),
                    policy_revision: 1,
                    grant_revision: None
                },
                "approve-expired"
            )
            .await,
        Err(AgentAccessError::Conflict)
    );
    let claim = store
        .claim_access_request("expiry-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.id, expired.id);
    assert_eq!(claim.phase, AgentAccessPhase::Pending);
    store
        .fail_access_request(claim.lease.as_ref().unwrap(), "request_expired")
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}
