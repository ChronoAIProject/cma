use super::*;
use layer2_core::sandbox::control::*;

async fn control(fixture: &ResourceFixture) -> SandboxControlRecord {
    let credential = "must-not-leak-access-token";
    let subject = SubjectId::new("owner");
    let context = CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        actor: ResourceActor::Api {
            subject: subject.clone(),
            delegation_id: None,
        },
        credential: fixture
            .identity
            .auth
            .provider_credential_context(&subject, credential)
            .unwrap(),
        memberships: vec![],
    };
    let sandbox_id = SandboxId("sandbox".into());
    let expected_revision = fixture
        .store
        .get_sandbox(&sandbox_id)
        .await
        .unwrap()
        .unwrap()
        .revision;
    SandboxControlService::new(
        fixture.store.clone(),
        Arc::new(fixture.access.clone()),
        fixture.identity.auth.clone(),
    )
    .admit(
        &context,
        credential,
        SandboxControlSubmission {
            sandbox_id,
            command: SandboxControl::Pause,
            expected_revision,
            idempotency_key: "control".into(),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn sandbox_controls_retained_authority_uses_actual_actor_for_personal_share_and_org() {
    for organization_owned in [false, true] {
        let fixture = ResourceFixture::new(organization_owned).await;
        let record = control(&fixture).await;
        let proof = fixture
            .identity
            .auth
            .authorize_sandbox_control(&fixture.access, &record)
            .await
            .unwrap();
        assert_eq!(proof.envelope(), &record.envelope);
        assert_eq!(proof.audit().unwrap().actor, record.envelope.actor);
        assert_eq!(
            record.envelope.actor,
            ResourceActor::Api {
                subject: SubjectId::new("owner"),
                delegation_id: None
            }
        );
        assert_eq!(record.envelope.owner, fixture.envelope.owner);
        assert_eq!(
            record.envelope.authorization,
            fixture.envelope.authorization
        );
        let serialized = serde_json::to_string(&record).unwrap();
        for secret in [
            "must-not-leak-access-token",
            "refresh_token",
            "credential_id",
        ] {
            assert!(!serialized.contains(secret));
        }
        let subject = match record.envelope.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        assert!(fixture
            .identity
            .store
            .subject(&subject)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn sandbox_controls_new_sign_in_and_lost_permissions_cannot_inherit_queued_work() {
    for cause in ["new-sign-in", "permissions", "role", "expired"] {
        let fixture = ResourceFixture::new(false).await;
        let mut record = control(&fixture).await;
        match cause {
            "new-sign-in" => {
                fixture.identity.seed().await;
            }
            "permissions" => {
                *fixture.identity.control.introspection.write().await =
                    Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
            }
            "role" => {
                *fixture.identity.control.organization.write().await =
                    Reply::json(json!({"id":"org","your_role":"viewer"}));
            }
            "expired" => {
                record.admitted_at =
                    layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(301));
                record.queue_deadline =
                    layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(1));
            }
            _ => unreachable!(),
        }
        assert!(
            fixture
                .identity
                .auth
                .authorize_sandbox_control(&fixture.access, &record)
                .await
                .is_err(),
            "{cause}"
        );
    }
}

#[tokio::test]
async fn sandbox_controls_sign_in_replacement_during_membership_io_invalidates_proof() {
    let fixture = ResourceFixture::new(false).await;
    let record = control(&fixture).await;
    let original = record.clone();
    let gate = Arc::new(Gate::default());
    fixture.identity.control.organization.write().await.gate = Some(gate.clone());
    let auth = fixture.identity.auth.clone();
    let access = fixture.access.clone();
    let pending =
        tokio::spawn(async move { auth.authorize_sandbox_control(&access, &record).await });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    fixture.identity.seed().await;
    gate.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(ResourceCommandAuthorizationError::Custody(
            CommandAuthorizationError::Unavailable
        ))
    ));
    assert_eq!(
        fixture
            .store
            .sandbox_control(&original.envelope.command_id)
            .await
            .unwrap(),
        Some(original)
    );
}

#[tokio::test]
async fn sandbox_controls_token_refresh_keeps_exact_custody_without_new_command_identity() {
    let fixture = ResourceFixture::new(false).await;
    let record = control(&fixture).await;
    let mut state = fixture
        .identity
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    let incarnation = state.incarnation.clone();
    let generation = state.token_generation;
    state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(30);
    fixture.identity.store.put_subject(state).await.unwrap();
    *fixture.identity.control.organization_token.write().await = "rotated-fixture-token".into();
    let proof = fixture
        .identity
        .auth
        .authorize_sandbox_control(&fixture.access, &record)
        .await
        .unwrap();
    proof.require_current().unwrap();
    let current = fixture
        .identity
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.incarnation, incarnation);
    assert_eq!(current.token_generation, generation + 1);
    assert_eq!(
        fixture
            .store
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap(),
        Some(record)
    );
}
