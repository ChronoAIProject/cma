use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_hot_scope_binds_connection_to_original_approval_and_generation() {
    use layer2_core::sandbox::ServiceConnectionSelection;
    let service = ServiceId::new("github");
    let connection = ServiceConnectionSelection {
        service: service.clone(),
        connection_id: "repository-connection".into(),
    };
    let replacement = ServiceConnectionSelection {
        connection_id: "other-connection".into(),
        ..connection.clone()
    };
    let (client, name, store, record) =
        super::super::super::tests::approved_fixture_with_connection(Some(connection.clone()))
            .await;
    let lease = record.lease.as_ref().unwrap();
    let authority = match record.decision.as_ref().unwrap() {
        AgentAccessDecision::Approved { authorization, .. } => authorization.clone(),
        _ => panic!("expected approved fixture"),
    };
    let granted = store
        .grant_live_access_request_services(lease)
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let after = sandbox(&reopened, &record).await;
    let scope = after.credential.service_scope.as_ref().unwrap();
    assert_eq!(scope.connection.as_ref(), Some(&connection));
    assert_eq!(
        Some(scope.credential_generation_floor),
        granted.progress.grant_generation_floor
    );
    assert_eq!(scope.authorized_by, record.requester.owner);
    assert_eq!(scope.services, vec![service.clone()]);
    assert_eq!(
        reopened
            .approve_agent_access_with_connection(
                &record.requester.owner,
                &record.requester.agent_id,
                &record.id,
                vec![service.clone()],
                Some(connection.clone()),
                &[],
                authority.clone(),
                "approve-fixture",
            )
            .await,
        Ok(granted.clone())
    );
    assert_eq!(
        reopened
            .approve_agent_access_with_connection(
                &record.requester.owner,
                &record.requester.agent_id,
                &record.id,
                vec![service.clone()],
                Some(replacement.clone()),
                &[],
                authority.clone(),
                "approve-fixture",
            )
            .await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(
        reopened.grant_live_access_request_services(lease).await,
        Ok(granted)
    );
    assert_eq!(sandbox(&reopened, &record).await, after);

    // Model settled credential delivery in the fixture. Exact provider mint and
    // runtime delivery are unavailable in this reader-only slice.
    let settled = reopened
        .fail_access_request(lease, "fixture_delivery_finished")
        .await
        .unwrap();
    let first_revision = scope.revision;
    let mut after = after;
    after.credential.key_generation += 1;
    let current = after.credential.current.as_mut().unwrap();
    current.generation = after.credential.key_generation;
    current.provider_key_id = "fixture-repository-key".into();
    after.work = SandboxWorkState::new(SandboxWorkStep::None, after.updated_at.clone());
    put(&reopened, &after).await;

    let mut proposal = record.proposal.clone();
    proposal.request_key = "switch-connection-proposal".into();
    let change = reopened
        .request_agent_access(record.requester.clone(), proposal)
        .await
        .unwrap();
    let binding = ConnectionApprovalBinding {
        sandbox_id: after.id.clone(),
        scope_revision: first_revision,
        credential_generation: after.credential.key_generation,
    };
    let stale_binding = ConnectionApprovalBinding {
        credential_generation: binding.credential_generation - 1,
        ..binding.clone()
    };
    assert_eq!(
        reopened
            .approve_agent_access_bound(
                &change.requester.owner,
                &change.requester.agent_id,
                &change.id,
                vec![service.clone()],
                Some(replacement.clone()),
                Some(stale_binding.clone()),
                &[],
                authority.clone(),
                "switch-connection-approval",
            )
            .await,
        Err(AgentAccessError::StaleGithubApproval)
    );
    // A definite stale consent refusal leaves the exact pending request intact.
    assert_eq!(
        reopened
            .github_approval_replay(
                &change.requester.owner,
                &change.requester.agent_id,
                &change.id,
                std::slice::from_ref(&service),
                &replacement,
                &stale_binding,
                "switch-connection-approval",
            )
            .await,
        Ok(None)
    );
    assert_eq!(sandbox(&reopened, &record).await, after);
    let approved = reopened
        .approve_agent_access_bound(
            &change.requester.owner,
            &change.requester.agent_id,
            &change.id,
            vec![service.clone()],
            Some(replacement.clone()),
            Some(binding.clone()),
            &[],
            authority.clone(),
            "switch-connection-approval",
        )
        .await
        .unwrap();
    assert_eq!(approved.expected_scope_revision, Some(first_revision));
    assert_eq!(approved.approved_connection_binding(), Some(&binding));
    assert_eq!(
        reopened
            .approve_agent_access_bound(
                &change.requester.owner,
                &change.requester.agent_id,
                &change.id,
                vec![service.clone()],
                Some(replacement.clone()),
                Some(stale_binding),
                &[],
                authority.clone(),
                "switch-connection-approval",
            )
            .await,
        Err(AgentAccessError::Conflict)
    );
    let claimed = reopened
        .claim_access_request("switch-connection-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, change.id);
    let next_lease = claimed.lease.as_ref().unwrap();
    let switching = reopened
        .grant_live_access_request_services(next_lease)
        .await
        .unwrap();
    let switched = sandbox(&reopened, &record).await;
    let scope = switched.credential.service_scope.as_ref().unwrap();
    assert_eq!(scope.connection.as_ref(), Some(&replacement));
    assert_eq!(scope.revision, first_revision + 1);
    assert_eq!(
        scope.credential_generation_floor,
        after.credential.key_generation + 1
    );
    assert_eq!(switched.runtime, after.runtime);
    assert_eq!(
        reopened
            .grant_live_access_request_services(next_lease)
            .await,
        Ok(switching)
    );
    assert_eq!(
        reopened
            .approve_agent_access_with_connection(
                &record.requester.owner,
                &record.requester.agent_id,
                &record.id,
                vec![service],
                Some(connection),
                &[],
                authority,
                "approve-fixture",
            )
            .await,
        Ok(settled)
    );
    assert_eq!(sandbox(&reopened, &record).await, switched);
    client.database(&name).drop().await.unwrap();
}
