use super::resource_access::{context, ready_conversation};
use super::*;
use crate::resource_access::*;
use crate::sandbox::agent_io::*;
use crate::sandbox::store::SandboxMutationGuard;

pub(super) struct LocalAuthority(pub(super) Arc<BindingAccessService>);
#[async_trait::async_trait]
impl OrganizationAuthority for LocalAuthority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("binding authority is local")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("binding authority is local")
    }
}
#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for LocalAuthority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        _: &str,
        _: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Ok(self
            .0
            .authenticate(&context.credential.credential_id)
            .await
            .unwrap()
            .unwrap()
            .command_authority())
    }
}

pub(super) async fn request(
    f: &Fixture,
    bindings: &Arc<BindingAccessService>,
    access: &ResourceAccessService,
    key: &str,
) -> CommandReservation {
    let principal = bindings
        .authenticate(&f.key.sha256())
        .await
        .unwrap()
        .unwrap();
    let proof = bindings
        .resolve(&principal, "scoped-conversation")
        .await
        .unwrap();
    let target = ResourceTarget::Agent {
        agent_id: proof.conversation.agent_id.clone(),
    };
    let held = access
        .authorize(
            &context(&principal),
            None,
            &target,
            ResourceCapability::Interact,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    let command = AgentCommand::message("binding input");
    let now = std::time::SystemTime::now();
    CommandReservation {
        binding: held.binding_authority().cloned(),
        envelope: AgentCommandEnvelope {
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: crate::ids::AgentCommandId::generate(),
            agent_id: proof.conversation.agent_id,
            owner: held.snapshot.scope(&target).unwrap().owner,
            actor: context(&principal).actor,
            source: AgentCommandSource {
                kind: AgentSourceKind::Api,
                reference: "binding-test".into(),
            },
            correlation_id: "binding-test".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: principal.command_authority(),
            command,
        },
        key_sha256: command_key_digest(key).unwrap(),
        response_id: Some(crate::ids::AgentResponseId::generate()),
        admitted_at: crate::rfc3339_at(now),
        queue_deadline: crate::rfc3339_at(now + std::time::Duration::from_secs(300)),
        queue_capacity: 4,
        expected_agent_revision: held.snapshot.lineage.agent.as_ref().unwrap().revision,
        parent: SandboxMutationGuard::capture(&held.snapshot.lineage.sandbox),
        policy: held.snapshot.fence(),
        authority_deadline: None,
        admission_audit: Some(
            held.authorization_audit(
                &context(&principal).actor,
                &target,
                ResourceCapability::Interact,
            )
            .unwrap(),
        ),
    }
}

#[tokio::test]
async fn binding_commands_fence_reservation_replay_execution_and_revival() {
    binding_commands("human").await;
}

#[tokio::test]
async fn organization_binding_commands_admit_replay_execute_and_revive_with_local_authority() {
    binding_commands("organization").await;
}

async fn binding_commands(owner_kind: &str) {
    let mut f = fixture();
    let mut profile = profile("Scoped command binding");
    profile["metadata"]["owner"]["kind"] = owner_kind.into();
    let binding = f.ready_with_profile(profile, None).await;
    let conversation = ready_conversation(&f, &binding).await;
    let bindings = BindingAccessService::new(f.store.clone());
    let authority = Arc::new(LocalAuthority(bindings.clone()));
    let access = Arc::new(
        ResourceAccessService::new(f.store.clone(), authority.clone())
            .with_bindings(bindings.clone()),
    );
    let old = request(&f, &bindings, &access, "turn-one").await;
    let mut missing_proof = old.clone();
    missing_proof.binding = None;
    assert_eq!(
        f.store.reserve_command(missing_proof).await,
        Err(CommandLedgerError::Invalid)
    );
    let wrong_kind = match old.envelope.owner.clone() {
        AgentOwner::Person { subject } => AgentOwner::Organization { subject },
        AgentOwner::Organization { subject } => AgentOwner::Person { subject },
    };
    let wrong_subject = match &old.envelope.owner {
        AgentOwner::Person { .. } => AgentOwner::Person {
            subject: SubjectId::new("foreign-owner"),
        },
        AgentOwner::Organization { .. } => AgentOwner::Organization {
            subject: SubjectId::new("foreign-owner"),
        },
    };
    for owner in [wrong_kind, wrong_subject] {
        let mut forged = old.clone();
        forged.envelope.owner = owner;
        assert_eq!(
            forged
                .binding
                .as_ref()
                .unwrap()
                .require_command(&forged.envelope),
            Err(CommandLedgerError::Invalid)
        );
        assert!(matches!(
            f.store.reserve_command(forged.clone()).await,
            Err(CommandLedgerError::Invalid | CommandLedgerError::NotFound)
        ));
        let principal = bindings
            .authenticate(&f.key.sha256())
            .await
            .unwrap()
            .unwrap();
        assert!(access
            .authorize_command_execution(&context(&principal), "", &forged.envelope)
            .await
            .is_err());
    }
    let CommandReservationResult::Created(record) =
        f.store.reserve_command(old.clone()).await.unwrap()
    else {
        panic!()
    };
    let principal = bindings
        .authenticate(&f.key.sha256())
        .await
        .unwrap()
        .unwrap();
    let execution = access
        .authorize_command_execution(&context(&principal), "", &record.receipt.envelope)
        .await
        .unwrap();
    execution.current_snapshot().await.unwrap();
    let revival = access
        .authorize_command_revival(&context(&principal), "", &record)
        .await
        .unwrap();
    assert!(revival.binding_authority().is_some());
    assert!(access
        .authorize(
            &context(&principal),
            None,
            &ResourceTarget::Sandbox {
                sandbox_id: binding.sandbox_id.clone()
            },
            ResourceCapability::ManageSandbox,
            AccessPurpose::Read
        )
        .await
        .is_err());

    f.key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let rotated = binding
        .rotate_credential(f.key.sha256(), crate::now_rfc3339())
        .unwrap();
    f.store
        .save_binding(Some(binding.revision), rotated.clone())
        .await
        .unwrap();
    assert_eq!(
        f.store.reserve_command(old.clone()).await,
        Err(CommandLedgerError::Conflict)
    );
    assert!(execution.current_snapshot().await.is_err());
    assert!(revival.current_snapshot().await.is_err());
    let next = request(&f, &bindings, &access, "turn-one").await;
    assert_eq!(old.scope(), next.scope());
    assert_eq!(
        f.store.reserve_command(next).await.unwrap(),
        CommandReservationResult::Existing(record.clone())
    );
    let principal = bindings
        .retained_principal(&binding.binding_id, binding.generation)
        .await
        .unwrap();
    access
        .authorize_command_execution(&context(&principal), "", &record.receipt.envelope)
        .await
        .unwrap()
        .current_snapshot()
        .await
        .unwrap();

    // The public admission service captures the same proof; stale store calls
    // cannot turn a scoped principal into an unfenced owner command.
    let commands = AgentCommandService::new(f.store.clone(), access.clone(), authority);
    let admitted = commands
        .admit(
            &context(&principal),
            "",
            CommandSubmissionSource::Api,
            CommandSubmission {
                agent_id: conversation.agent_id.clone(),
                command: AgentCommand::message("another turn"),
                idempotency_key: "turn-two".into(),
                expected_agent_revision: None,
                requested_model: None,
            },
        )
        .await
        .unwrap();
    let pending = request(&f, &bindings, &access, "turn-three").await;
    f.service
        .retire_conversation(
            &f.caller,
            &binding.binding_id,
            &conversation.conversation_id,
        )
        .await
        .unwrap();
    assert_eq!(
        f.store.reserve_command(pending.clone()).await,
        Err(CommandLedgerError::Conflict)
    );
    assert!(f
        .store
        .get_command(&pending.envelope.command_id)
        .await
        .unwrap()
        .is_none());
    assert!(access
        .authorize_command_execution(&context(&principal), "", &record.receipt.envelope)
        .await
        .is_err());
    assert!(f
        .store
        .get_command(&admitted.receipt.command_id)
        .await
        .unwrap()
        .is_some());
}
