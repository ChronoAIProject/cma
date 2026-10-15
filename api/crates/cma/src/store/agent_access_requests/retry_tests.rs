use super::*;

fn retry(record: &AgentAccessRecord, key: &str) -> AgentAccessProposal {
    AgentAccessProposal {
        request_key: key.into(),
        retry_of: Some(record.id.clone()),
        services: vec!["api-github".into()],
        reason: "Retry the owner-selected repository access".into(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_failed_retry_is_a_new_pending_attempt_without_inherited_consent() {
    let (client, database, store, old) = tests::approved_fixture().await;
    let failed = store
        .fail_access_request(old.lease.as_ref().unwrap(), "access_update_cannot_continue")
        .await
        .unwrap();
    let proposal = retry(&failed, "explicit-retry-one");
    let next = store
        .request_agent_access(failed.requester.clone(), proposal.clone())
        .await
        .unwrap();
    assert_ne!(next.id, failed.id);
    assert_eq!(next.phase, AgentAccessPhase::Pending);
    assert!(
        next.decision.is_none()
            && next.approved_at.is_none()
            && next.lease.is_none()
            && next.error.is_none()
    );
    assert_eq!(next.progress, AgentAccessProgress::default());
    assert_eq!(next.view().retry_of.as_deref(), Some(failed.id.as_str()));
    assert_eq!(
        store
            .request_agent_access(failed.requester.clone(), failed.proposal.clone())
            .await
            .unwrap(),
        failed
    );
    assert_eq!(
        store
            .request_agent_access(failed.requester.clone(), proposal.clone())
            .await
            .unwrap(),
        next
    );
    let mut changed = proposal;
    changed.reason = "Changed purpose on an existing attempt".into();
    assert_eq!(
        store
            .request_agent_access(failed.requester.clone(), changed)
            .await,
        Err(AgentAccessError::Conflict)
    );
    let mut same_key = retry(&failed, &failed.proposal.request_key);
    same_key.services = failed.proposal.services.clone();
    assert_eq!(
        store
            .request_agent_access(failed.requester.clone(), same_key)
            .await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(
        store
            .access_request_connection_services(&next.requester.owner, &next.id)
            .await,
        Err(AgentAccessError::Conflict)
    );
    let approved = store
        .approve_agent_access(
            &next.requester.owner,
            &next.requester.agent_id,
            &next.id,
            vec![ServiceId::new("api-github")],
            &[],
            AgentAuthorizationRevision {
                authority_id: "personal:new-owner-consent".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            "new-approval",
        )
        .await
        .unwrap();
    assert_eq!(approved.phase, AgentAccessPhase::Connecting);
    assert_ne!(approved.decision, failed.decision);
    assert_eq!(
        store.read_access_request(&failed.id).await.unwrap(),
        Some(failed.clone())
    );
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened.read_access_request(&failed.id).await.unwrap(),
        Some(failed)
    );
    assert_eq!(
        reopened.read_access_request(&next.id).await.unwrap(),
        Some(approved)
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_retry_rejects_nonfailed_or_missing_predecessors() {
    let (client, database, store, active) = tests::approved_fixture().await;
    assert_eq!(
        store
            .request_agent_access(active.requester.clone(), retry(&active, "retry-active"))
            .await,
        Err(AgentAccessError::Conflict)
    );
    let mut missing = retry(&active, "retry-missing");
    missing.retry_of = Some(format!("aar_{}", "0".repeat(64)));
    assert_eq!(
        store
            .request_agent_access(active.requester.clone(), missing)
            .await,
        Err(AgentAccessError::NotFound)
    );
    let mut fresh = active.proposal.clone();
    fresh.request_key = "deny-fresh".into();
    let pending = store
        .request_agent_access(active.requester.clone(), fresh)
        .await
        .unwrap();
    assert_eq!(
        store
            .request_agent_access(active.requester.clone(), retry(&pending, "retry-pending"))
            .await,
        Err(AgentAccessError::Conflict)
    );
    let denied = store
        .deny_agent_access(
            &pending.requester.owner,
            &pending.requester.agent_id,
            &pending.id,
            "deny",
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .request_agent_access(active.requester.clone(), retry(&denied, "retry-denied"))
            .await,
        Err(AgentAccessError::Conflict)
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_concurrent_retry_attempts_have_one_successor_and_stable_replay() {
    let (client, database, store, first) = tests::approved_fixture().await;
    let failed = store
        .fail_access_request(
            first.lease.as_ref().unwrap(),
            "access_update_cannot_continue",
        )
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        store.request_agent_access(failed.requester.clone(), retry(&failed, "retry-a")),
        store.request_agent_access(failed.requester.clone(), retry(&failed, "retry-b")),
    );
    let winner = match (a, b) {
        (Ok(record), Err(AgentAccessError::Conflict))
        | (Err(AgentAccessError::Conflict), Ok(record)) => record,
        other => panic!("exactly one successor must commit: {other:?}"),
    };
    assert_eq!(
        store
            .request_agent_access(winner.requester.clone(), winner.proposal.clone())
            .await
            .unwrap(),
        winner
    );
    assert_eq!(
        store.read_access_request(&failed.id).await.unwrap(),
        Some(failed.clone())
    );
    let listed = store
        .list_agent_access_requests(&failed.requester.owner, &failed.requester.agent_id)
        .await
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed
            .iter()
            .filter(|r| r.proposal.retry_of.as_deref() == Some(&failed.id))
            .count(),
        1
    );
    client.database(&database).drop().await.unwrap();
}
