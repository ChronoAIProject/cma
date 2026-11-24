//! Pause/resume use a separate persisted-control authorizer from Agent messages.
use super::*;
use layer2_core::sandbox::control::*;

fn controls(fixture: &ResourceFixture, auth: Arc<AuthState>) -> Arc<SandboxControlService> {
    SandboxControlService::new(
        fixture.store.clone(),
        Arc::new(ResourceAccessService::new(
            fixture.store.clone(),
            auth.clone(),
        )),
        auth,
    )
}

async fn submission(
    fixture: &ResourceFixture,
    command: SandboxControl,
) -> SandboxControlSubmission {
    let sandbox_id = SandboxId("sandbox".into());
    let expected_revision = fixture
        .store
        .get_sandbox(&sandbox_id)
        .await
        .unwrap()
        .unwrap()
        .revision;
    SandboxControlSubmission {
        sandbox_id,
        command,
        expected_revision,
        idempotency_key: "signed-sandbox-control".into(),
    }
}

#[tokio::test]
async fn signed_pause_and_resume_controls_survive_restart_with_proxy_custody_and_stable_receipts() {
    for owner in ["owner", "org"] {
        for command in [SandboxControl::Pause, SandboxControl::Revive] {
            let fixture = ResourceFixture::with_owner(owner).await;
            let ingress = SignedIngress::new(&fixture).await;
            let headers = signed_headers(&signed_claims());
            let request = ingress
                .auth
                .resolve_request(&headers)
                .await
                .unwrap()
                .unwrap();
            let context = ingress.auth.resource_request_context(&request);
            assert!(matches!(
                context.credential_source,
                CredentialSource::IdentityProxy { .. }
            ));
            let credential = request.delegated_credential.as_deref().unwrap();
            let accepted = controls(&fixture, ingress.auth.clone())
                .admit(&context, credential, submission(&fixture, command).await)
                .await
                .unwrap();
            let persisted = fixture
                .store
                .sandbox_control(&accepted.envelope.command_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(persisted, accepted);
            assert_eq!(persisted.envelope.command, command);
            assert!(persisted
                .envelope
                .authorization
                .authority_id
                .starts_with("nyxid-proxy:"));
            assert_eq!(
                persisted.envelope.actor,
                ResourceActor::Api {
                    subject: SubjectId::new("owner"),
                    delegation_id: None
                }
            );
            assert_eq!(persisted.envelope.owner, fixture.envelope.owner);
            assert_eq!(persisted.status, SandboxControlStatus::Queued);

            // A new auth adapter recovers the persisted proxy grant; the control
            // worker does not receive or replay the original identity JWT.
            let restarted = proxy_auth(&fixture);
            let proof = restarted
                .authorize_sandbox_control(&fixture.access, &persisted)
                .await
                .unwrap();
            proof.require_current().unwrap();
            assert_eq!(proof.envelope(), &persisted.envelope);
            assert_eq!(proof.audit().unwrap().actor, persisted.envelope.actor);
            assert_eq!(
                proof.current_snapshot().await.unwrap().lineage.sandbox.id,
                SandboxId("sandbox".into())
            );
            let replay = controls(&fixture, restarted.clone())
                .admit(&context, credential, submission(&fixture, command).await)
                .await
                .unwrap();
            assert_eq!(replay, persisted);
            let serialized = serde_json::to_string(&persisted).unwrap();
            assert!(!serialized.contains(credential));
            assert!(!serialized.contains("must-not-leak-refresh-token"));
            assert!(!serialized.contains(
                headers[crate::auth::nyxid_proxy::jwt::HEADER]
                    .to_str()
                    .unwrap()
            ));
        }
    }
}

#[tokio::test]
async fn persisted_signed_pause_and_resume_refuse_revoked_or_replaced_authority() {
    for command in [SandboxControl::Pause, SandboxControl::Revive] {
        for cause in [
            "permissions",
            "new-sign-in",
            "disabled",
            "foreign-grant",
            "expired-queue",
        ] {
            let fixture = ResourceFixture::with_owner("owner").await;
            let ingress = SignedIngress::new(&fixture).await;
            let request = ingress
                .auth
                .resolve_request(&signed_headers(&signed_claims()))
                .await
                .unwrap()
                .unwrap();
            let context = ingress.auth.resource_request_context(&request);
            let original = controls(&fixture, ingress.auth.clone())
                .admit(
                    &context,
                    request.delegated_credential.as_deref().unwrap(),
                    submission(&fixture, command).await,
                )
                .await
                .unwrap();
            let mut attempted = original.clone();
            match cause {
                "permissions" => {
                    *fixture.identity.control.introspection.write().await =
                        Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
                }
                "new-sign-in" => {
                    fixture.identity.seed().await;
                }
                "foreign-grant" => {
                    attempted.envelope.authorization.authority_id =
                        format!("nyxid-proxy:{}", "00".repeat(32));
                }
                "expired-queue" => {
                    attempted.admitted_at =
                        layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(301));
                    attempted.queue_deadline =
                        layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(1));
                }
                _ => {}
            }
            let restarted = if cause == "disabled" {
                AuthState::with_store(
                    fixture.identity.auth.fixture_mode().clone(),
                    None,
                    fixture.identity.store.clone(),
                )
                .with_agent_key_service(Some(SERVICE.into()))
            } else {
                proxy_auth(&fixture)
            };
            assert!(
                restarted
                    .authorize_sandbox_control(&fixture.access, &attempted)
                    .await
                    .is_err(),
                "{command:?}: {cause}"
            );
            assert_eq!(
                fixture
                    .store
                    .sandbox_control(&original.envelope.command_id)
                    .await
                    .unwrap(),
                Some(original)
            );
        }
    }
}

#[tokio::test]
async fn signed_empty_permission_ceiling_cannot_admit_pause_or_resume_despite_owner_permissions() {
    for command in [SandboxControl::Pause, SandboxControl::Revive] {
        let fixture = ResourceFixture::with_owner("owner").await;
        *fixture.identity.control.introspection.write().await =
            Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:*"]}));
        let ingress = SignedIngress::new(&fixture).await;
        let mut claims = signed_claims();
        claims["permissions"] = json!([]);
        let request = ingress
            .auth
            .resolve_request(&signed_headers(&claims))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.authorization, Authorization::none());
        let context = ingress.auth.resource_request_context(&request);
        assert!(controls(&fixture, ingress.auth.clone())
            .admit(
                &context,
                request.delegated_credential.as_deref().unwrap(),
                submission(&fixture, command).await,
            )
            .await
            .is_err());
    }
}
