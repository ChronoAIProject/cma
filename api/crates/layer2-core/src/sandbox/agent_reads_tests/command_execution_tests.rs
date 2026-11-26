use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::store::SandboxMutationGuard;
use layer3_adapter::{ConversationInput, ConversationInputSender};

#[path = "command_revival_tests.rs"]
mod revival;

#[path = "command_audit_tests.rs"]
mod audit;

#[path = "command_lifecycle_dispatch_tests.rs"]
mod lifecycle_dispatch;

#[path = "composer_admission_tests.rs"]
mod composer_admission;

#[path = "command_funding_tests.rs"]
pub(super) mod funding;

pub(super) async fn grant(store: &InMemorySandboxStore, mode: ShareMode) -> ResourceShare {
    let snapshot = store
        .resource_access_snapshot(&ResourceTarget::Sandbox {
            sandbox_id: SandboxId("sandbox".into()),
        })
        .await
        .unwrap();
    store
        .mutate_share(ShareMutation {
            selector: ResourceSelector::Sandbox {
                sandbox_id: SandboxId("sandbox".into()),
            },
            owner: snapshot
                .scope(&ResourceTarget::Sandbox {
                    sandbox_id: SandboxId("sandbox".into()),
                })
                .unwrap()
                .owner,
            actor: context("owner").actor,
            key_sha256: command_key_digest("execution-share").unwrap(),
            change: ShareChange::Create {
                target_org_id: SubjectId::new("org"),
                mode,
            },
            expected_share_revision: None,
            policy_fence: snapshot.fence(),
            proposed_id: "execution-share".into(),
            at: crate::now_rfc3339(),
        })
        .await
        .unwrap()
        .share
}

async fn execution_fixture(
    lifetime: Duration,
) -> (
    Arc<InMemorySandboxStore>,
    ResourceAccessService,
    Arc<Authority>,
    Agent,
) {
    let mut authority = Authority::new();
    authority.lifetime = lifetime;
    *authority.membership.lock().unwrap() = Ok(OrganizationRole::Admin);
    let (store, _, authority, mut agent) = setup(authority).await;
    let revision = agent.revision;
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(NOW.into());
    agent.conversation.conversation_id = Some("retained-conversation".into());
    agent.conversation.runtime_generation = Some(3);
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let access = ResourceAccessService::new(store.clone(), authority.clone());
    (store, access, authority, agent)
}

async fn claim(store: &InMemorySandboxStore, agent: &Agent, actor: &str) -> AdmittedAgentCommand {
    let target = ResourceTarget::Agent {
        agent_id: agent.id.clone(),
    };
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let now = SystemTime::now();
    let command = AgentCommand::message("shared execution");
    store
        .reserve_command(CommandReservation {
            binding: None,
            envelope: AgentCommandEnvelope {
                continuation: None,
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId::generate(),
                agent_id: agent.id.clone(),
                owner: snapshot.scope(&target).unwrap().owner,
                actor: context(actor).actor,
                source: AgentCommandSource {
                    kind: AgentSourceKind::Api,
                    reference: "test".into(),
                },
                correlation_id: "test".into(),
                request_fingerprint: command.fingerprint().unwrap(),
                authorization: AgentAuthorizationRevision {
                    replay_principal: None,
                    authority_id: "actual-admin-custody".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
                command,
            },
            key_sha256: command_key_digest("execution").unwrap(),
            response_id: Some(AgentResponseId::generate()),
            admitted_at: crate::rfc3339_at(now),
            queue_deadline: crate::rfc3339_at(now + Duration::from_secs(300)),
            queue_capacity: 16,
            expected_agent_revision: agent.revision,
            parent: SandboxMutationGuard::capture(&snapshot.lineage.sandbox),
            policy: snapshot.fence(),
            authority_deadline: None,
            admission_audit: None,
        })
        .await
        .unwrap();
    store
        .claim_next_command(CommandClaimRequest {
            agent_id: agent.id.clone(),
            holder: "worker".into(),
            now: crate::rfc3339_at(now),
            expires_at: crate::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap()
}

struct Runtime {
    store: Arc<InMemorySandboxStore>,
    sender: ConversationInputSender,
    revoke: Mutex<Option<ResourceShare>>,
    at: usize,
    resolutions: AtomicUsize,
}

#[async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        if self.resolutions.fetch_add(1, Ordering::SeqCst) + 1 == self.at {
            let grant = self.revoke.lock().unwrap().take();
            if let Some(grant) = grant {
                share(&self.store, Some(grant)).await;
            }
        }
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: self.sender.clone(),
        })
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

fn runtime(
    store: Arc<InMemorySandboxStore>,
    revoke: Option<ResourceShare>,
    at: usize,
) -> (Arc<Runtime>, Arc<AtomicUsize>) {
    let sends = Arc::new(AtomicUsize::new(0));
    let sent = sends.clone();
    let sender = ConversationInputSender::spawn(4, move |_| {
        sent.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    (
        Arc::new(Runtime {
            store,
            sender,
            revoke: Mutex::new(revoke),
            at,
            resolutions: AtomicUsize::new(0),
        }),
        sends,
    )
}

#[tokio::test]
async fn shared_admin_executes_once_with_actual_actor_and_retained_owner() {
    let (store, access, _, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    grant(&store, ShareMode::OrgRoles).await;
    let command = claim(&store, &agent, "admin").await;
    let authorization = access
        .authorize_command_execution(
            &context("admin"),
            "selected-credential",
            &command.receipt.envelope,
        )
        .await
        .unwrap();
    assert_eq!(authorization.caller().subject, SubjectId::new("admin"));
    let (runtime, sends) = runtime(store.clone(), None, 0);
    let operations = AgentOperationService::new(store.clone(), runtime);
    operations
        .execute_resource_claim(&authorization, &command)
        .await
        .unwrap();
    let retained = store
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.receipt, command.receipt);
    assert!(retained.work.dispatch.is_some());
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(current.owner, agent.owner);
    assert_eq!(current.profile, agent.profile);
    assert_eq!(
        current.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert!(operations
        .execute_resource_claim(&authorization, &command)
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn view_grant_members_and_actor_substitution_cannot_obtain_execution_proof() {
    for mode in [ShareMode::View, ShareMode::OrgRoles] {
        let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        grant(&store, mode).await;
        let command = claim(&store, &agent, "admin").await;
        for role in [
            OrganizationRole::Viewer,
            OrganizationRole::Member,
            OrganizationRole::Admin,
        ] {
            *authority.membership.lock().unwrap() = Ok(role);
            let result = access
                .authorize_command_execution(
                    &context("admin"),
                    "selected-credential",
                    &command.receipt.envelope,
                )
                .await;
            assert_eq!(
                result.is_ok(),
                mode == ShareMode::OrgRoles && role == OrganizationRole::Admin
            );
        }
        assert!(access
            .authorize_command_execution(
                &context("owner"),
                "selected-credential",
                &command.receipt.envelope
            )
            .await
            .is_err());
        assert!(access
            .authorize_personal_command_execution(
                &context("admin").caller,
                &command.receipt.envelope
            )
            .await
            .is_err());
        let mut channel = command.receipt.envelope.clone();
        channel.source.kind = AgentSourceKind::Channel;
        assert!(access
            .authorize_command_execution(&context("admin"), "selected-credential", &channel)
            .await
            .is_err());
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    }
}

#[tokio::test]
async fn revocation_during_runtime_lookup_fences_binding_and_final_writer() {
    for at in [1, 2] {
        let (store, access, _, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
        let grant = grant(&store, ShareMode::OrgRoles).await;
        let command = claim(&store, &agent, "admin").await;
        let authorization = access
            .authorize_command_execution(
                &context("admin"),
                "selected-credential",
                &command.receipt.envelope,
            )
            .await
            .unwrap();
        let (runtime, sends) = runtime(store.clone(), Some(grant), at);
        let operations = AgentOperationService::new(store.clone(), runtime.clone());
        let result = operations
            .execute_resource_claim(&authorization, &command)
            .await;
        assert!(result.is_err());
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        let retained = store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.receipt, command.receipt);
        assert_eq!(retained.work.dispatch.is_some(), at == 2);
        if at == 2 {
            let current = store.get_agent(&agent.id).await.unwrap().unwrap();
            assert_eq!(
                current.conversation.operation.unwrap().state,
                AgentOperationDelivery::PreWriteFailed,
                "{result:?}"
            );
            assert_eq!(current.status, AgentStatus::Ready);
        } else {
            assert_eq!(
                store.get_agent(&agent.id).await.unwrap(),
                Some(agent.clone())
            );
        }
        assert!(access
            .authorize_personal(
                &context("owner").caller,
                &ResourceTarget::Agent { agent_id: agent.id },
                ResourceCapability::Interact
            )
            .await
            .is_ok());
        runtime
            .sender
            .send(ConversationInput::Cancel)
            .await
            .unwrap();
        assert_eq!(sends.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn held_proof_expires_without_renewal_and_cannot_adopt_changed_command() {
    let (store, access, _, agent) = execution_fixture(Duration::from_millis(100)).await;
    grant(&store, ShareMode::OrgRoles).await;
    let command = claim(&store, &agent, "admin").await;
    let authorization = access
        .authorize_command_execution(
            &context("admin"),
            "selected-credential",
            &command.receipt.envelope,
        )
        .await
        .unwrap();
    let mut changed = command.receipt.envelope.clone();
    changed.command = AgentCommand::message("different effect");
    changed.request_fingerprint = changed.command.fingerprint().unwrap();
    assert!(authorization.fence().require_envelope(&changed).is_err());
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(authorization.current_snapshot().await.is_err());
    let (runtime, sends) = runtime(store.clone(), None, 0);
    assert!(AgentOperationService::new(store.clone(), runtime)
        .execute_resource_claim(&authorization, &command)
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

struct OwnerConnections(Arc<Mutex<Vec<SubjectId>>>);
#[async_trait]
impl SandboxCredentialPrerequisite for OwnerConnections {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        self.0.lock().unwrap().push(owner.clone());
        Ok(())
    }
}

#[tokio::test]
async fn organization_owned_execution_keeps_runtime_credentials_with_resource_owner() {
    let (store, access, authority, agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    store.set_resource_policy_for_test(ResourcePolicyRecord {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: parent.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: parent.owner.clone(),
            admitted_by: context("admin").actor,
            verified_at: crate::now_rfc3339(),
        }),
    });
    let command = claim(&store, &agent, "admin").await;
    *authority.membership.lock().unwrap() = Ok(OrganizationRole::Member);
    assert!(access
        .authorize_command_execution(
            &context("admin"),
            "selected-credential",
            &command.receipt.envelope
        )
        .await
        .is_err());
    *authority.membership.lock().unwrap() = Ok(OrganizationRole::Admin);
    let proof = access
        .authorize_command_execution(
            &context("admin"),
            "selected-credential",
            &command.receipt.envelope,
        )
        .await
        .unwrap();
    let checked_owners = Arc::new(Mutex::new(Vec::new()));
    let (runtime, sends) = runtime(store.clone(), None, 0);
    AgentOperationService::new(store.clone(), runtime)
        .with_connections(Arc::new(OwnerConnections(checked_owners.clone())))
        .execute_resource_claim(&proof, &command)
        .await
        .unwrap();
    assert_eq!(*checked_owners.lock().unwrap(), vec![parent.owner.clone()]);
    let retained = store
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.receipt.envelope.owner,
        ResourceOwner::Organization {
            subject: parent.owner
        }
    );
    assert_eq!(retained.receipt.envelope.actor, context("admin").actor);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
}
