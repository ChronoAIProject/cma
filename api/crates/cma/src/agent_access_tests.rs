use super::*;
use layer2_core::resource_access::CredentialContext;
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::sandbox::SandboxPolicy;
use layer2_core::{Authorization, SubjectId};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Authority {
    revoked: AtomicBool,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl PersonalCommandAdmissionAuthority for Authority {
    async fn personal_command_authority(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.revoked.load(Ordering::SeqCst)
            || caller.subject.as_str() != "owner"
            || credential != "owner-fixture-credential"
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: "personal:fixture-owner-login".into(),
            policy_revision: 7,
            grant_revision: None,
        })
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_service_requires_current_human_retained_authority_and_stores_no_credential() {
    let (client, name, store, first) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let authority = Arc::new(Authority {
        revoked: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let service = AgentAccessService::new(
        store.clone(),
        authority.clone(),
        Arc::new(SandboxPolicy {
            baseline_services: vec![ServiceId::new("baseline")],
            ..Default::default()
        }),
    );
    let mut proposal = first.proposal.clone();
    proposal.request_key = "human-consent".into();
    let record = store
        .request_agent_access(first.requester.clone(), proposal)
        .await
        .unwrap();
    let context = CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            record.requester.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "provider".into(),
            subject: record.requester.owner.clone(),
            credential_id: "owner-login".into(),
        },
        actor: ResourceActor::Human {
            subject: record.requester.owner.clone(),
        },
        memberships: vec![],
    };
    let services = vec!["baseline".into(), "github".into()];
    for case in 0..6 {
        let mut denied = context.clone();
        match case {
            0 => denied.credential_source = CredentialSource::Independent { expires_at: None },
            1 => denied.credential_source = CredentialSource::Binding,
            2 => {
                denied.actor = ResourceActor::Api {
                    subject: record.requester.owner.clone(),
                    delegation_id: None,
                }
            }
            3 => {
                denied.actor = ResourceActor::Api {
                    subject: record.requester.owner.clone(),
                    delegation_id: Some("grant".into()),
                };
                denied.credential_source = CredentialSource::delegated("grant");
            }
            4 => denied.credential.subject = SubjectId::new("foreign"),
            _ => denied.caller.authorization = Authorization::new([]),
        }
        assert_eq!(
            service
                .approve(
                    &denied,
                    "owner-fixture-credential",
                    &record.requester.agent_id,
                    &record.id,
                    services.clone(),
                    "human-approve"
                )
                .await,
            Err(AgentAccessError::Forbidden)
        );
    }
    assert_eq!(authority.calls.load(Ordering::SeqCst), 0);
    authority.revoked.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .approve(
                &context,
                "owner-fixture-credential",
                &record.requester.agent_id,
                &record.id,
                services.clone(),
                "human-approve"
            )
            .await,
        Err(AgentAccessError::Forbidden)
    );
    authority.revoked.store(false, Ordering::SeqCst);
    assert_eq!(
        service
            .approve(
                &context,
                "owner-fixture-credential",
                &record.requester.agent_id,
                &record.id,
                vec!["github".into()],
                "human-approve"
            )
            .await,
        Err(AgentAccessError::Invalid)
    );
    let approved = service
        .approve(
            &context,
            "owner-fixture-credential",
            &record.requester.agent_id,
            &record.id,
            services,
            "human-approve",
        )
        .await
        .unwrap();
    assert_eq!(
        approved.status,
        layer2_core::sandbox::agent_access::AgentAccessStatus::Connecting
    );
    let saved = store
        .read_access_request(&record.id)
        .await
        .unwrap()
        .unwrap();
    let encoded = serde_json::to_string(&saved).unwrap();
    assert!(!encoded.contains("owner-fixture-credential"));
    assert!(!encoded.contains("human-approve"));
    assert!(encoded.contains("personal:fixture-owner-login"));
    client.database(&name).drop().await.unwrap();
}
