use layer2_core::resource_access::*;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::store::{
    AgentStore, InMemorySandboxStore, SandboxStore, WorkspaceStore, WorktreeStore,
};
use layer2_core::sandbox::AgentStatus;
use layer2_core::{Caller, SandboxId};

use super::resource_fixture::*;
use super::*;

#[path = "resource_admission_tests.rs"]
mod admission;
#[path = "api_bearer_command_tests.rs"]
mod api_bearer;
#[path = "nyxid_proxy_command_tests.rs"]
mod nyxid_proxy;
#[path = "sandbox_controls_tests.rs"]
mod sandbox_controls;

struct ResourceFixture {
    identity: Fixture,
    store: Arc<InMemorySandboxStore>,
    access: ResourceAccessService,
    envelope: AgentCommandEnvelope,
}

impl ResourceFixture {
    async fn reserve(&self) -> AdmittedAgentCommand {
        let snapshot = self
            .store
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: self.envelope.agent_id.clone(),
            })
            .await
            .unwrap();
        let now = SystemTime::now();
        let result = self
            .store
            .reserve_command(CommandReservation {
                binding: None,
                envelope: self.envelope.clone(),
                key_sha256: command_key_digest("revival").unwrap(),
                response_id: Some(layer2_core::ids::AgentResponseId::generate()),
                admitted_at: layer2_core::rfc3339_at(now),
                queue_deadline: layer2_core::rfc3339_at(now + Duration::from_secs(300)),
                queue_capacity: 16,
                expected_agent_revision: snapshot.lineage.agent.as_ref().unwrap().revision,
                parent: layer2_core::sandbox::store::SandboxMutationGuard::capture(
                    &snapshot.lineage.sandbox,
                ),
                policy: snapshot.fence(),
                authority_deadline: None,
                admission_audit: None,
            })
            .await
            .unwrap();
        match result {
            CommandReservationResult::Created(command)
            | CommandReservationResult::Existing(command) => *command,
        }
    }

    async fn new(organization_owned: bool) -> Self {
        Self::with_owner(if organization_owned {
            "org"
        } else {
            "resource-owner"
        })
        .await
    }

    async fn with_owner(resource_owner: &str) -> Self {
        let organization_owned = resource_owner == "org";
        let identity = Fixture::new().await;
        identity.seed().await;
        let caller = Caller::new(
            SubjectId::new("owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let authorization = identity
            .auth
            .personal_command_authority(&caller, "must-not-leak-access-token")
            .await
            .unwrap();
        let store = Arc::new(InMemorySandboxStore::default());
        let parent = sandbox("sandbox", resource_owner);
        store.create_sandbox(parent.clone()).await.unwrap();
        store
            .create_workspace(workspace("workspace", "sandbox", resource_owner))
            .await
            .unwrap();
        store
            .create_worktree(worktree(
                "worktree",
                "workspace",
                "sandbox",
                "agent",
                resource_owner,
            ))
            .await
            .unwrap();
        let agent = agent(
            "agent",
            "worktree",
            "workspace",
            "sandbox",
            resource_owner,
            AgentStatus::Starting,
        );
        store.create_agent(agent.clone()).await.unwrap();
        if organization_owned {
            store.set_resource_policy_for_test(ResourcePolicyRecord {
                sandbox_id: parent.id.clone(),
                sandbox_created_at: parent.created_at.clone(),
                owner: ResourceOwner::Organization {
                    subject: parent.owner.clone(),
                },
                revision: 1,
                organization_evidence: Some(OrganizationOwnershipEvidence {
                    issuer: "https://fixture.test".into(),
                    organization: parent.owner.clone(),
                    admitted_by: ResourceActor::Human {
                        subject: caller.subject.clone(),
                    },
                    verified_at: layer2_core::now_rfc3339(),
                }),
            });
        } else if resource_owner != "owner" {
            let target = ResourceTarget::Sandbox {
                sandbox_id: parent.id.clone(),
            };
            let snapshot = store.resource_access_snapshot(&target).await.unwrap();
            store
                .mutate_share(ShareMutation {
                    selector: ResourceSelector::Sandbox {
                        sandbox_id: parent.id,
                    },
                    owner: snapshot.scope(&target).unwrap().owner,
                    actor: ResourceActor::Human {
                        subject: parent.owner,
                    },
                    key_sha256: command_key_digest("fixture-share").unwrap(),
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::OrgRoles,
                    },
                    expected_share_revision: None,
                    policy_fence: snapshot.fence(),
                    proposed_id: "fixture-share".into(),
                    at: layer2_core::now_rfc3339(),
                })
                .await
                .unwrap();
        }
        let owner = store
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: agent.id.clone(),
            })
            .await
            .unwrap()
            .scope(&ResourceTarget::Agent {
                agent_id: agent.id.clone(),
            })
            .unwrap()
            .owner;
        let command = AgentCommand::message("retained shared input");
        let envelope = AgentCommandEnvelope {
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: layer2_core::ids::AgentCommandId::generate(),
            agent_id: agent.id,
            owner,
            actor: ResourceActor::Human {
                subject: caller.subject,
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::UserInterface,
                reference: "fixture".into(),
            },
            correlation_id: "fixture".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization,
            command,
        };
        let access = ResourceAccessService::new(store.clone(), identity.auth.clone());
        Self {
            identity,
            store,
            access,
            envelope,
        }
    }

    async fn authorize(
        &self,
    ) -> Result<CommandExecutionAuthorization, ResourceCommandAuthorizationError> {
        self.identity
            .auth
            .authorize_resource_command(&self.access, &self.envelope)
            .await
    }

    async fn blocked(
        &self,
    ) -> (
        Arc<Gate>,
        tokio::task::JoinHandle<
            Result<CommandExecutionAuthorization, ResourceCommandAuthorizationError>,
        >,
    ) {
        let gate = Arc::new(Gate::default());
        self.identity.control.organization.write().await.gate = Some(gate.clone());
        let auth = self.identity.auth.clone();
        let access = self.access.clone();
        let envelope = self.envelope.clone();
        let pending =
            tokio::spawn(async move { auth.authorize_resource_command(&access, &envelope).await });
        tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
            .await
            .unwrap();
        (gate, pending)
    }
}

#[tokio::test]
async fn shared_human_and_api_execution_bind_actual_custody_without_resource_owner_login() {
    for organization_owned in [false, true] {
        let mut fixture = ResourceFixture::new(organization_owned).await;
        let original = fixture.envelope.clone();
        let target = ResourceTarget::Agent {
            agent_id: original.agent_id.clone(),
        };
        let before = fixture
            .store
            .resource_access_snapshot(&target)
            .await
            .unwrap();
        for actor in [
            ResourceActor::Human {
                subject: SubjectId::new("owner"),
            },
            ResourceActor::Api {
                subject: SubjectId::new("owner"),
                delegation_id: None,
            },
        ] {
            fixture.envelope.actor = actor;
            let proof = fixture.authorize().await.unwrap();
            assert_eq!(proof.caller().subject, SubjectId::new("owner"));
            assert_eq!(proof.current_snapshot().await.unwrap(), before);
            proof.fence().require_envelope(&fixture.envelope).unwrap();
            assert!(!format!("{:?}", proof.fence()).contains("must-not-leak"));
            assert!(matches!(
                fixture
                    .identity
                    .auth
                    .authorize_command(&fixture.envelope)
                    .await,
                Err(CommandAuthorizationError::Revoked)
            ));
        }
        assert_eq!(
            fixture
                .identity
                .control
                .organization_requests
                .load(Ordering::SeqCst),
            2
        );
        assert_eq!(
            fixture
                .store
                .resource_access_snapshot(&target)
                .await
                .unwrap(),
            before
        );
        assert_eq!(fixture.envelope.owner, original.owner);
    }
}

#[tokio::test]
async fn every_shared_execution_observes_live_org_role_and_provider_capabilities() {
    for organization_owned in [false, true] {
        let fixture = ResourceFixture::new(organization_owned).await;
        fixture.authorize().await.unwrap();
        *fixture.identity.control.organization.write().await =
            Reply::json(json!({"id":"org", "your_role":"viewer"}));
        assert!(matches!(
            fixture.authorize().await,
            Err(ResourceCommandAuthorizationError::Resource(
                ResourceAccessError::Access(_)
            ))
        ));
        fixture.identity.control.organization.write().await.status =
            StatusCode::SERVICE_UNAVAILABLE;
        assert!(matches!(
            fixture.authorize().await,
            Err(ResourceCommandAuthorizationError::Resource(
                ResourceAccessError::Membership(MembershipError::Unavailable)
            ))
        ));
        *fixture.identity.control.organization.write().await =
            Reply::json(json!({"id":"org", "your_role":"admin"}));
        fixture.authorize().await.unwrap();
        let before = fixture
            .identity
            .control
            .organization_requests
            .load(Ordering::SeqCst);
        *fixture.identity.control.introspection.write().await =
            Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
        assert!(matches!(
            fixture.authorize().await,
            Err(ResourceCommandAuthorizationError::Custody(
                CommandAuthorizationError::Revoked
            ))
        ));
        assert_eq!(
            fixture
                .identity
                .control
                .organization_requests
                .load(Ordering::SeqCst),
            before
        );
    }
}

#[tokio::test]
async fn refresh_preserves_incarnation_and_uses_exact_rotated_token_for_org_lookup() {
    let fixture = ResourceFixture::new(false).await;
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
    let proof = fixture.authorize().await.unwrap();
    proof.fence().require_envelope(&fixture.envelope).unwrap();
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
        current.tokens.as_ref().unwrap().access_token.expose(),
        "rotated-fixture-token"
    );
    assert_eq!(
        fixture
            .identity
            .auth
            .personal_command_authority(proof.caller(), "rotated-fixture-token")
            .await
            .unwrap(),
        fixture.envelope.authorization
    );
}

#[tokio::test]
async fn new_sign_in_during_refresh_cannot_adopt_retained_shared_command() {
    let fixture = ResourceFixture::new(false).await;
    let mut state = fixture
        .identity
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(30);
    fixture.identity.store.put_subject(state).await.unwrap();
    let gate = Arc::new(Gate::default());
    fixture.identity.control.exchange.write().await.gate = Some(gate.clone());
    let auth = fixture.identity.auth.clone();
    let access = fixture.access.clone();
    let envelope = fixture.envelope.clone();
    let pending =
        tokio::spawn(async move { auth.authorize_resource_command(&access, &envelope).await });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    fixture.identity.seed().await;
    gate.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(ResourceCommandAuthorizationError::Custody(
            CommandAuthorizationError::Revoked
        ))
    ));
    assert_eq!(
        fixture
            .identity
            .control
            .organization_requests
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn custody_changes_during_org_lookup_never_return_execution_proof() {
    for change in ["sign_in", "generation", "access", "removed", "expired"] {
        let fixture = ResourceFixture::new(false).await;
        let (gate, pending) = fixture.blocked().await;
        let mut state = fixture
            .identity
            .store
            .subject(&SubjectId::new("owner"))
            .await
            .unwrap()
            .unwrap();
        match change {
            "sign_in" => {
                fixture.identity.seed().await;
            }
            _ => {
                match change {
                    "generation" => state.token_generation += 1,
                    "access" => {
                        state.tokens.as_mut().unwrap().access_token =
                            SecretValue::new("replaced-fixture-token".into())
                    }
                    "removed" => state.tokens = None,
                    "expired" => state.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH,
                    _ => unreachable!(),
                }
                state.revision += 1;
                fixture.identity.store.put_subject(state).await.unwrap();
            }
        }
        gate.release.notify_one();
        assert!(
            matches!(
                pending.await.unwrap(),
                Err(ResourceCommandAuthorizationError::Custody(
                    CommandAuthorizationError::Unavailable
                ))
            ),
            "{change}"
        );
    }
}

#[tokio::test]
async fn authority_replacement_before_shared_execution_is_revoked_before_provider_io() {
    let fixture = ResourceFixture::new(false).await;
    fixture.identity.seed().await;
    let before = fixture.identity.control.requests.load(Ordering::SeqCst);
    assert!(matches!(
        fixture.authorize().await,
        Err(ResourceCommandAuthorizationError::Custody(
            CommandAuthorizationError::Revoked
        ))
    ));
    assert_eq!(
        fixture.identity.control.requests.load(Ordering::SeqCst),
        before
    );
}

#[tokio::test]
async fn shared_credential_path_refuses_delegation_channels_and_tampered_custody() {
    let mut fixture = ResourceFixture::new(false).await;
    let original = fixture.envelope.clone();
    for change in 0..7 {
        fixture.envelope = original.clone();
        match change {
            0 => {
                fixture.envelope.actor = ResourceActor::Api {
                    subject: SubjectId::new("owner"),
                    delegation_id: Some("delegated".into()),
                }
            }
            1 => fixture.envelope.source.kind = AgentSourceKind::Channel,
            2 => fixture.envelope.source.kind = AgentSourceKind::Trigger,
            3 => fixture.envelope.authorization.policy_revision += 1,
            4 => fixture.envelope.authorization.grant_revision = Some(1),
            5 => {
                fixture.envelope.actor = ResourceActor::Human {
                    subject: SubjectId::new("resource-owner"),
                }
            }
            _ => fixture.envelope.command = AgentCommand::message("tampered"),
        }
        let before = fixture.identity.control.requests.load(Ordering::SeqCst);
        assert!(matches!(
            fixture.authorize().await,
            Err(ResourceCommandAuthorizationError::Custody(
                CommandAuthorizationError::Revoked
            ))
        ));
        assert_eq!(
            fixture.identity.control.requests.load(Ordering::SeqCst),
            before
        );
    }
}

#[tokio::test]
async fn policy_change_during_org_lookup_invalidates_shared_execution() {
    let fixture = ResourceFixture::new(false).await;
    let (gate, pending) = fixture.blocked().await;
    let target = ResourceTarget::Sandbox {
        sandbox_id: SandboxId("sandbox".into()),
    };
    let snapshot = fixture
        .store
        .resource_access_snapshot(&target)
        .await
        .unwrap();
    let mut policy = snapshot.policy.unwrap();
    policy.revision += 1;
    fixture.store.set_resource_policy_for_test(policy);
    gate.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(ResourceCommandAuthorizationError::Resource(
            ResourceAccessError::Store(ResourceAccessStoreError::Conflict)
        ))
    ));
}

#[tokio::test]
async fn personal_resource_path_preserves_retained_authority_without_membership_lookup() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let caller = fixture
        .identity
        .auth
        .authorize_command(&fixture.envelope)
        .await
        .unwrap();
    let proof = fixture.authorize().await.unwrap();
    assert_eq!(proof.caller(), &caller);
    proof.fence().require_envelope(&fixture.envelope).unwrap();
    assert_eq!(
        fixture
            .identity
            .control
            .organization_requests
            .load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn shared_resource_owner_substitution_cannot_reuse_valid_human_custody() {
    let mut fixture = ResourceFixture::new(false).await;
    for owner in [
        ResourceOwner::Person {
            subject: SubjectId::new("owner"),
        },
        ResourceOwner::Organization {
            subject: SubjectId::new("resource-owner"),
        },
    ] {
        fixture.envelope.owner = owner;
        assert!(matches!(
            fixture.authorize().await,
            Err(ResourceCommandAuthorizationError::Resource(
                ResourceAccessError::Access(_)
            ))
        ));
    }
}

#[tokio::test]
async fn restricted_retained_token_is_refused_before_refresh_or_provider_io() {
    use base64::Engine as _;
    let fixture = ResourceFixture::new(false).await;
    for claims in [
        json!({"relay":true}),
        json!({"sa":true}),
        json!({"delegated":true}),
        json!({"act":{"sub":"owner"}}),
    ] {
        let mut state = fixture
            .identity
            .store
            .subject(&SubjectId::new("owner"))
            .await
            .unwrap()
            .unwrap();
        state.tokens.as_mut().unwrap().access_token = SecretValue::new(format!(
            "e30.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap())
        ));
        state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(30);
        fixture.identity.store.put_subject(state).await.unwrap();
        let before = fixture.identity.control.requests.load(Ordering::SeqCst);
        assert!(matches!(
            fixture.authorize().await,
            Err(ResourceCommandAuthorizationError::Custody(
                CommandAuthorizationError::Revoked
            ))
        ));
        assert_eq!(
            fixture.identity.control.requests.load(Ordering::SeqCst),
            before
        );
    }
}

#[tokio::test]
async fn shared_credential_verification_has_one_total_deadline() {
    let fixture = ResourceFixture::new(false).await;
    let (_gate, pending) = fixture.blocked().await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(16)).await;
    assert!(matches!(
        pending.await.unwrap(),
        Err(
            ResourceCommandAuthorizationError::Custody(CommandAuthorizationError::Unavailable)
                | ResourceCommandAuthorizationError::Resource(ResourceAccessError::Membership(
                    MembershipError::Unavailable
                ))
        )
    ));
    tokio::time::resume();
}

#[tokio::test]
async fn retained_shared_custody_authorizes_revival_with_fresh_parent_evidence() {
    for org_owned in [false, true] {
        let fixture = ResourceFixture::new(org_owned).await;
        let command = fixture.reserve().await;
        let proof = fixture
            .identity
            .auth
            .authorize_command_revival(&fixture.access, &command)
            .await
            .unwrap();
        proof.require_command(&command).unwrap();
        assert_eq!(
            proof
                .current_snapshot()
                .await
                .unwrap()
                .scope(&proof.target())
                .unwrap()
                .owner,
            command.receipt.envelope.owner
        );
        assert_eq!(
            fixture
                .identity
                .control
                .organization_requests
                .load(Ordering::SeqCst),
            2
        );
        assert_eq!(
            fixture
                .store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command)
        );
    }
}

#[tokio::test]
async fn replacement_during_parent_authorization_invalidates_revival_custody() {
    let fixture = ResourceFixture::new(false).await;
    let command = fixture.reserve().await;
    let original = command.clone();
    let gate = Arc::new(Gate::default());
    fixture.identity.control.organization.write().await.gate = Some(gate.clone());
    let auth = fixture.identity.auth.clone();
    let access = fixture.access.clone();
    let pending =
        tokio::spawn(async move { auth.authorize_command_revival(&access, &command).await });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    gate.release.notify_one(); // Agent interaction lookup
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    fixture.identity.seed().await; // Sandbox management lookup still in flight
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
            .get_command(&original.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(original)
    );
}

#[tokio::test]
async fn lost_org_role_between_agent_and_parent_checks_refuses_revival() {
    let fixture = ResourceFixture::new(false).await;
    let command = fixture.reserve().await;
    let gate = Arc::new(Gate::default());
    fixture.identity.control.organization.write().await.gate = Some(gate.clone());
    let auth = fixture.identity.auth.clone();
    let access = fixture.access.clone();
    let pending =
        tokio::spawn(async move { auth.authorize_command_revival(&access, &command).await });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    *fixture.identity.control.organization.write().await =
        Reply::json(json!({"id":"org", "your_role":"viewer"}));
    gate.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(ResourceCommandAuthorizationError::Resource(
            ResourceAccessError::Access(_)
        ))
    ));
    assert_eq!(
        fixture
            .identity
            .control
            .organization_requests
            .load(Ordering::SeqCst),
        2
    );
}
