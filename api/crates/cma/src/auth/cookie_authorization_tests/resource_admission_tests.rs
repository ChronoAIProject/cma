use super::*;
use layer2_core::sandbox::store::PageRequest;

fn context(fixture: &ResourceFixture, credential: &str) -> CallerContext {
    let subject = SubjectId::new("owner");
    CallerContext {
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
    }
}

fn submission() -> CommandSubmission {
    CommandSubmission {
        agent_id: layer2_core::AgentId("agent".into()),
        command: AgentCommand::message("shared admission input"),
        idempotency_key: "shared-request".into(),
        expected_agent_revision: None,
        requested_model: None,
    }
}

fn service(fixture: &ResourceFixture) -> Arc<AgentCommandService> {
    AgentCommandService::new(
        fixture.store.clone(),
        Arc::new(fixture.access.clone()),
        fixture.identity.auth.clone(),
    )
}

#[tokio::test]
async fn shared_and_org_admission_retain_actor_custody_policy_and_fresh_deferred_authority() {
    for organization_owned in [false, true] {
        let fixture = ResourceFixture::new(organization_owned).await;
        let credential = "must-not-leak-access-token";
        let context = context(&fixture, credential);
        let service = service(&fixture);
        let first = service
            .admit(
                &context,
                credential,
                CommandSubmissionSource::Api,
                submission(),
            )
            .await
            .unwrap();
        let command = fixture
            .store
            .get_command(&first.receipt.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.receipt.envelope.actor, context.actor);
        assert_eq!(command.receipt.envelope.owner, fixture.envelope.owner);
        assert_eq!(
            command.receipt.envelope.authorization,
            fixture.envelope.authorization
        );
        assert!(fixture
            .identity
            .store
            .subject(match &command.receipt.envelope.owner {
                AgentOwner::Person { subject } | AgentOwner::Organization { subject } => subject,
            })
            .await
            .unwrap()
            .is_none());
        fixture
            .identity
            .auth
            .authorize_resource_command(&fixture.access, &command.receipt.envelope)
            .await
            .unwrap();
        let audits = fixture
            .store
            .command_authorization_audits(
                &first.receipt.command_id,
                &PageRequest::new(20, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(audits.items.len(), 1);
        let audit = &audits.items[0];
        assert_eq!(audit.actor, context.actor);
        assert_eq!(audit.owner, fixture.envelope.owner);
        assert_eq!(audit.credential_authority, fixture.envelope.authorization);
        assert_eq!(audit.resources[0].policy_revision, 1);
        assert!(
            matches!(&audit.resources[0].observations[0].basis, ResourceAuthorizationBasis::Organization {
            organization, role: OrganizationRole::Admin, share, ..
        } if organization.as_str() == "org" && share.is_none() == organization_owned)
        );
        let serialized = serde_json::to_string(audit).unwrap();
        for secret in [
            credential,
            "shared admission input",
            "refresh_token",
            "credential_id",
        ] {
            assert!(!serialized.contains(secret));
        }
        let again = service
            .admit(
                &context,
                credential,
                CommandSubmissionSource::Api,
                submission(),
            )
            .await
            .unwrap();
        assert_eq!(again.receipt, first.receipt);
        assert_eq!(
            fixture
                .store
                .command_authorization_audits(
                    &first.receipt.command_id,
                    &PageRequest::new(20, None).unwrap()
                )
                .await
                .unwrap(),
            audits
        );

        *fixture.identity.control.organization.write().await =
            Reply::json(json!({"id":"org", "your_role":"viewer"}));
        assert!(service
            .admit(
                &context,
                credential,
                CommandSubmissionSource::Api,
                submission()
            )
            .await
            .is_err());
        assert!(fixture
            .identity
            .auth
            .authorize_resource_command(&fixture.access, &command.receipt.envelope)
            .await
            .is_err());
        assert_eq!(
            fixture
                .store
                .command_authorization_audits(
                    &first.receipt.command_id,
                    &PageRequest::new(20, None).unwrap()
                )
                .await
                .unwrap(),
            audits
        );
        assert_eq!(
            fixture
                .store
                .get_command(&first.receipt.command_id)
                .await
                .unwrap()
                .unwrap()
                .receipt,
            command.receipt
        );
    }
}

#[tokio::test]
async fn shared_admission_denies_viewers_missing_custody_and_substituted_principals_without_a_receipt(
) {
    for organization_owned in [false, true] {
        for denial in ["viewer", "member", "missing-custody", "actor", "credential"] {
            let fixture = ResourceFixture::new(organization_owned).await;
            let credential = "must-not-leak-access-token";
            let mut context = context(&fixture, credential);
            match denial {
                "viewer" | "member" => {
                    *fixture.identity.control.organization.write().await =
                        Reply::json(json!({"id":"org", "your_role":denial}))
                }
                "missing-custody" => {
                    let mut state = fixture
                        .identity
                        .store
                        .subject(&context.caller.subject)
                        .await
                        .unwrap()
                        .unwrap();
                    state.tokens = None;
                    state.revision += 1;
                    fixture.identity.store.put_subject(state).await.unwrap();
                }
                "actor" => {
                    context.actor = ResourceActor::Human {
                        subject: SubjectId::new("resource-owner"),
                    }
                }
                "credential" => context.credential.credential_id = "unrelated".into(),
                _ => unreachable!(),
            }
            assert!(
                service(&fixture)
                    .admit(
                        &context,
                        credential,
                        CommandSubmissionSource::Api,
                        submission()
                    )
                    .await
                    .is_err(),
                "{denial}"
            );
            assert!(fixture
                .store
                .command_work_page(&PageRequest::new(20, None).unwrap())
                .await
                .unwrap()
                .items
                .is_empty());
        }
    }
}

#[tokio::test]
async fn sign_in_replacement_during_admission_membership_cannot_borrow_the_old_token() {
    let fixture = ResourceFixture::new(false).await;
    let credential = "must-not-leak-access-token";
    let context = context(&fixture, credential);
    let gate = Arc::new(Gate::default());
    fixture.identity.control.organization.write().await.gate = Some(gate.clone());
    let service = service(&fixture);
    let work = tokio::spawn(async move {
        service
            .admit(
                &context,
                credential,
                CommandSubmissionSource::Api,
                submission(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    let mut replacement = fixture.identity.seed().await;
    replacement.tokens.as_mut().unwrap().access_token =
        SecretValue::new("replacement-access".into());
    fixture
        .identity
        .store
        .put_subject(replacement)
        .await
        .unwrap();
    gate.release.notify_one();
    assert!(work.await.unwrap().is_err());
    assert!(fixture
        .store
        .command_work_page(&PageRequest::new(20, None).unwrap())
        .await
        .unwrap()
        .items
        .is_empty());
}
