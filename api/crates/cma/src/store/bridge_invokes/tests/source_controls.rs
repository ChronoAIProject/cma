use super::*;
mod corruption;

fn service(f: &Fixture) -> BridgeSourceControlService {
    BridgeSourceControlService::new(f.service.clone(), f.store.clone())
}
fn request(f: &Fixture, revision: u64, action: SourceControlAction) -> SourceControlRequest {
    SourceControlRequest {
        expected_bridge_revision: f.bridge.revision,
        expected_grant_revision: 1,
        expected_policy_revision: revision,
        action,
    }
}
async fn grant(f: &mut Fixture, credential: Option<&str>) -> BridgeInvokeGrantId {
    let grant = f
        .service
        .issue(&f.context, credential, f.issue("invoke-consent"))
        .await
        .unwrap()
        .result
        .id;
    f.enable("bridge-enable").await;
    grant
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn source_control_consent_is_explicit_keyed_and_never_changes_invoke_or_conversation() {
    let mut f = Fixture::new().await;
    let grant = grant(&mut f, None).await;
    let original_grant = f.store.grant(&grant).await.unwrap().unwrap();
    let original = f
        .resources
        .resource_access_snapshot(&original_grant.target.resource())
        .await
        .unwrap();
    let api = service(&f);
    assert_eq!(api.policy(&f.context, None, &grant).await.unwrap(), None);
    let consent = request(&f, 0, SourceControlAction::AllowSenderCancelAndSteer);
    let (a, b) = tokio::join!(
        api.submit(&f.context, None, &grant, "enable", consent.clone()),
        api.submit(&f.context, None, &grant, "enable", consent.clone())
    );
    let receipt = a.unwrap();
    assert_eq!(b.unwrap(), receipt);
    assert_eq!(receipt.result.updated_by, f.context.actor);
    assert_eq!(receipt.result.scope, SourceControlScope::SourceSender);
    assert_eq!(
        receipt.result.approval_mode,
        SourceControlApprovalMode::DashboardOnly
    );
    assert!(receipt.result.enabled);
    let revoked = api
        .submit(
            &f.context,
            None,
            &grant,
            "revoke",
            request(&f, 1, SourceControlAction::RevokeSenderControls),
        )
        .await
        .unwrap();
    assert!(!revoked.result.enabled);
    let reopened = Arc::new(
        MongoBridgeInvokeStore::open(f.bridges.clone(), f.resources.clone(), &f.database)
            .await
            .unwrap(),
    );
    let resumed = BridgeSourceControlService::new(f.service.clone(), reopened);
    assert_eq!(
        resumed
            .submit(&f.context, None, &grant, "enable", consent.clone())
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        resumed.policy(&f.context, None, &grant).await.unwrap(),
        Some(revoked.result)
    );
    let mut changed = consent;
    changed.expected_bridge_revision += 1;
    assert_eq!(
        resumed
            .submit(&f.context, None, &grant, "enable", changed)
            .await,
        Err(BridgeInvokeError::IdempotencyConflict)
    );
    let renewed = api
        .submit(
            &f.context,
            None,
            &grant,
            "new-enable",
            request(&f, 2, SourceControlAction::AllowSenderCancelAndSteer),
        )
        .await
        .unwrap();
    assert_eq!(renewed.result.revision, 3);
    assert_eq!(
        f.store.grant(&grant).await.unwrap(),
        Some(original_grant.clone())
    );
    assert_eq!(
        f.resources
            .resource_access_snapshot(&original_grant.target.resource())
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let db = f.client.database(&f.database);
    for name in [
        "bridge_source_control_receipts",
        "bridge_source_control_audits",
    ] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            3
        );
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn source_control_audit_failure_rolls_back_consent_and_same_key_can_recover() {
    let mut f = Fixture::new().await;
    let grant = grant(&mut f, None).await;
    let api = service(&f);
    let db = f.client.database(&f.database);
    db.run_command(doc!{"collMod":"bridge_source_control_audits","validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    let consent = request(&f, 0, SourceControlAction::AllowSenderCancelAndSteer);
    assert_eq!(
        api.submit(&f.context, None, &grant, "enable", consent.clone())
            .await,
        Err(BridgeInvokeError::Unavailable)
    );
    for name in [
        "bridge_source_control_policies",
        "bridge_source_control_receipts",
        "bridge_source_control_audits",
    ] {
        assert_eq!(
            db.collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    db.run_command(doc! {"collMod":"bridge_source_control_audits","validator":{}})
        .await
        .unwrap();
    let receipt = api
        .submit(&f.context, None, &grant, "enable", consent.clone())
        .await
        .unwrap();
    assert_eq!(
        api.submit(&f.context, None, &grant, "enable", consent)
            .await
            .unwrap(),
        receipt
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn source_control_current_role_and_grant_revocation_cannot_infer_new_permission() {
    let mut f = Fixture::organization().await;
    let credential = Some("fixture-credential");
    let grant = grant(&mut f, credential).await;
    let api = service(&f);
    let consent = request(&f, 0, SourceControlAction::AllowSenderCancelAndSteer);
    let receipt = api
        .submit(&f.context, credential, &grant, "enable", consent.clone())
        .await
        .unwrap();
    *f.authority.role.lock().unwrap() = Some(OrganizationRole::Member);
    assert!(api.policy(&f.context, credential, &grant).await.is_err());
    assert!(api
        .submit(&f.context, credential, &grant, "enable", consent.clone())
        .await
        .is_err());
    *f.authority.role.lock().unwrap() = Some(OrganizationRole::Admin);
    let issued = f.store.grant(&grant).await.unwrap().unwrap().view();
    f.service
        .revoke(&f.context, credential, f.revoke(&issued, "invoke-revoke"))
        .await
        .unwrap();
    assert_eq!(
        api.submit(&f.context, credential, &grant, "enable", consent)
            .await
            .unwrap(),
        receipt
    );
    let mut new = request(&f, 1, SourceControlAction::AllowSenderCancelAndSteer);
    new.expected_grant_revision = 2;
    assert_eq!(
        api.submit(&f.context, credential, &grant, "other", new)
            .await,
        Err(BridgeInvokeError::Revoked)
    );
    let mut revoke = request(&f, 1, SourceControlAction::RevokeSenderControls);
    revoke.expected_grant_revision = 2;
    assert!(
        !api.submit(&f.context, credential, &grant, "policy-revoke", revoke)
            .await
            .unwrap()
            .result
            .enabled
    );
    f.close().await;
}
