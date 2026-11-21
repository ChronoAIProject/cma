use super::*;
use crate::sandbox::store::PageRequest;

#[tokio::test]
async fn persisted_audit_rejects_malformed_replay_principals() {
    let (store, access, authority, _) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let accepted = AgentCommandService::new(store.clone(), Arc::new(access), authority)
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("principal-audit"),
        )
        .await
        .unwrap();
    let audits = store
        .command_authorization_audits(&accepted.receipt.command_id, &PageRequest::default())
        .await
        .unwrap();
    let mut audit = audits.items[0].clone();
    audit.validate().unwrap();
    audit.credential_authority.replay_principal = Some("stable-owner".into());
    audit.validate().unwrap();
    for invalid in [
        "".into(),
        "a".repeat(257),
        "two words".into(),
        "a\nb".into(),
    ] {
        audit.credential_authority.replay_principal = Some(invalid);
        assert_eq!(audit.validate(), Err(CommandLedgerError::Corrupt));
    }
}

#[tokio::test]
async fn admission_and_dispatch_keep_separate_policy_evidence_and_retries_do_not_rewrite_it() {
    let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let service = AgentCommandService::new(store.clone(), Arc::new(access.clone()), authority);
    let accepted = service
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            submission("audit"),
        )
        .await
        .unwrap();
    let original = store
        .get_command(&accepted.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    let before = store
        .command_authorization_audits(
            &accepted.receipt.command_id,
            &PageRequest::new(20, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(before.items.len(), 1);
    assert_eq!(before.items[0].resources[0].policy_revision, 0);
    assert!(matches!(
        before.items[0].resources[0].observations[0].basis,
        ResourceAuthorizationBasis::PersonalOwner
    ));
    let saved = serde_json::to_value(&original).unwrap();
    assert!(saved.get("admission_audit").is_none());
    assert!(saved["receipt"].get("resources").is_none());

    grant(&store, ShareMode::OrgRoles).await;
    let now = SystemTime::now();
    let claimed = store
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id.clone(),
            holder: "audit-worker".into(),
            now: crate::rfc3339_at(now),
            expires_at: crate::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    let authorization = access
        .authorize_command_execution(
            &context("owner"),
            "selected-credential",
            &claimed.receipt.envelope,
        )
        .await
        .unwrap();
    let (runtime, sends) = runtime(store.clone(), None, 0);
    AgentOperationService::new(store.clone(), runtime)
        .execute_resource_claim(&authorization, &claimed)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let after = store
        .command_authorization_audits(
            &accepted.receipt.command_id,
            &PageRequest::new(20, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after.items.len(), 2);
    assert!(after.items.contains(&before.items[0]));
    let dispatch = after
        .items
        .iter()
        .find(|a| matches!(a.effect, CommandAuthorizationEffect::Dispatch { .. }))
        .unwrap();
    assert_eq!(dispatch.resources[0].policy_revision, 1);
    assert_eq!(
        dispatch.credential_authority,
        before.items[0].credential_authority
    );
    assert_eq!(
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("audit")
            )
            .await
            .unwrap()
            .receipt,
        accepted.receipt
    );
    assert_eq!(
        store
            .command_authorization_audits(
                &accepted.receipt.command_id,
                &PageRequest::new(20, None).unwrap()
            )
            .await
            .unwrap(),
        after
    );
    let page = store
        .command_authorization_audits(
            &accepted.receipt.command_id,
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap();
    let next = store
        .command_authorization_audits(
            &accepted.receipt.command_id,
            &PageRequest::new(1, page.next).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items[0], after.items[0]);
    assert_eq!(next.items[0], after.items[1]);
    assert!(next.next.is_none());
}

#[tokio::test]
async fn shared_dispatch_audit_survives_revocation_and_cannot_supply_new_authority() {
    let (store, access, _, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let grant = grant(&store, ShareMode::OrgRoles).await;
    let command = claim(&store, &agent, "admin").await;
    let proof = access
        .authorize_command_execution(
            &context("admin"),
            "selected-credential",
            &command.receipt.envelope,
        )
        .await
        .unwrap();
    let (runtime, _) = runtime(store.clone(), None, 0);
    AgentOperationService::new(store.clone(), runtime)
        .execute_resource_claim(&proof, &command)
        .await
        .unwrap();
    let page = PageRequest::new(20, None).unwrap();
    let audits = store
        .command_authorization_audits(&command.receipt.envelope.command_id, &page)
        .await
        .unwrap();
    assert_eq!(audits.items.len(), 1);
    let audit = &audits.items[0];
    assert_eq!(audit.actor, context("admin").actor);
    assert_eq!(audit.owner, command.receipt.envelope.owner);
    assert!(
        matches!(&audit.resources[0].observations[0].basis, ResourceAuthorizationBasis::Organization { role: OrganizationRole::Admin, share: Some(observed), .. } if observed.id == grant.id && observed.revision == grant.revision)
    );
    let serialized = serde_json::to_string(audit).unwrap();
    let historical: CommandAuthorizationAudit = serde_json::from_str(&serialized).unwrap();
    share(&store, Some(grant)).await;
    historical.validate().unwrap();
    assert!(proof.current_snapshot().await.is_err());
    assert!(access
        .authorize_command_execution(
            &context("admin"),
            "selected-credential",
            &command.receipt.envelope
        )
        .await
        .is_err());
    assert_eq!(
        store
            .command_authorization_audits(&command.receipt.envelope.command_id, &page)
            .await
            .unwrap(),
        audits
    );
    assert!(!serialized.contains("selected-credential"));
    assert!(!serialized.contains("shared execution"));
}

#[tokio::test]
async fn revoked_execution_cannot_append_audit_or_bind_a_command() {
    let (store, access, _, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    grant(&store, ShareMode::OrgRoles).await;
    let command = claim(&store, &agent, "admin").await;
    let proof = access
        .authorize_command_execution(
            &context("admin"),
            "selected-credential",
            &command.receipt.envelope,
        )
        .await
        .unwrap();
    let before = store
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap();
    let (runtime, sends) = runtime(
        store.clone(),
        Some(grant(&store, ShareMode::OrgRoles).await),
        1,
    );
    assert!(AgentOperationService::new(store.clone(), runtime)
        .execute_resource_claim(&proof, &command)
        .await
        .is_err());
    assert_eq!(
        store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap(),
        before
    );
    assert!(store
        .command_authorization_audits(
            &command.receipt.envelope.command_id,
            &PageRequest::new(20, None).unwrap()
        )
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
}
