use super::*;
use layer2_core::sandbox::store::SandboxStore;

fn authority(digit: char) -> AgentAuthorizationRevision {
    AgentAuthorizationRevision {
        authority_id: format!("owner-api:{}:2000000000", digit.to_string().repeat(64)),
        policy_revision: 1,
        grant_revision: None,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_expired_scope_consent_recovers_pending_rotation_without_rewriting_audit() {
    let (client, name, store, fixture) = super::super::tests::approved_fixture().await;
    store
        .fail_access_request(fixture.lease.as_ref().unwrap(), "fixture_complete")
        .await
        .unwrap();
    let before = store
        .get_sandbox(&fixture.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let services = vec![ServiceId::new("github")];
    let first = store
        .replace_agent_service_access(
            &before.owner,
            &fixture.requester.agent_id,
            services.clone(),
            before.credential.effective_reaches(),
            &[],
            authority('a'),
            "initial-consent",
        )
        .await
        .unwrap();
    let claimed = store
        .claim_access_request("initial-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.id, claimed.id);
    store
        .grant_live_access_request_services(claimed.lease.as_ref().unwrap())
        .await
        .unwrap();
    let mut pending = store.get_sandbox(&before.id).await.unwrap().unwrap();
    assert!(pending.needs_service_access_credential());
    pending.status = SandboxStatus::CredentialRotating;
    pending.work.schedule(
        layer2_core::sandbox::SandboxWorkStep::MintRotatedCredential,
        layer2_core::now_rfc3339(),
    );
    pending.validate().unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id":pending.id.as_str()},
            doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&pending).unwrap()},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .replace_agent_service_access(
                &before.owner,
                &fixture.requester.agent_id,
                services.clone(),
                services.clone(),
                &[],
                authority('b'),
                "renew-consent",
            )
            .await,
        Err(AgentAccessError::Conflict)
    );
    let failed = store
        .fail_access_request(claimed.lease.as_ref().unwrap(), "owner_authority_revoked")
        .await
        .unwrap();
    let renewed = store
        .replace_agent_service_access(
            &before.owner,
            &fixture.requester.agent_id,
            services.clone(),
            services.clone(),
            &[],
            authority('b'),
            "renew-consent",
        )
        .await
        .unwrap();
    assert_eq!(renewed.phase, AgentAccessPhase::Updating);
    assert_eq!(
        renewed.progress.renewed_from.as_deref(),
        Some(failed.id.as_str())
    );
    assert_eq!(
        renewed.progress.grant_generation_floor,
        failed.progress.grant_generation_floor
    );
    assert_eq!(
        store.agent_access_request(&failed.id).await.unwrap(),
        Some(failed.clone())
    );
    assert_eq!(
        store.get_sandbox(&before.id).await.unwrap(),
        Some(pending.clone())
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .credential_scope_authorization(&pending)
            .await
            .unwrap(),
        Some(authority('b'))
    );
    assert_eq!(
        reopened
            .replace_agent_service_access(
                &before.owner,
                &fixture.requester.agent_id,
                services.clone(),
                services.clone(),
                &[],
                authority('b'),
                "renew-consent",
            )
            .await
            .unwrap(),
        renewed
    );
    assert_eq!(
        reopened
            .replace_agent_service_access(
                &before.owner,
                &fixture.requester.agent_id,
                services.clone(),
                services,
                &[],
                authority('c'),
                "competing-renewal",
            )
            .await,
        Err(AgentAccessError::Conflict)
    );

    // Authority selection rejects corrupt history instead of guessing by time.
    assert_eq!(
        latest_consent(vec![renewed.clone(), failed.clone()], &pending).unwrap(),
        Some(renewed.clone())
    );
    assert_eq!(
        latest_consent(vec![renewed.clone()], &pending),
        Err(AgentAccessError::Invalid)
    );
    let mut fork = renewed.clone();
    fork.id = "fork".into();
    assert_eq!(
        latest_consent(vec![failed.clone(), renewed.clone(), fork], &pending),
        Err(AgentAccessError::Invalid)
    );
    let mut cycle = failed.clone();
    cycle.progress.renewed_from = Some(renewed.id.clone());
    assert_eq!(
        latest_consent(vec![cycle, renewed.clone()], &pending),
        Err(AgentAccessError::Invalid)
    );
    let mut foreign = renewed;
    foreign.requester.owner = SubjectId::new("foreign");
    assert_eq!(
        latest_consent(vec![failed, foreign], &pending),
        Err(AgentAccessError::Invalid)
    );
    client.database(&name).drop().await.unwrap();
}
