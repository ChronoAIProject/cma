use super::*;
use crate::sandbox::store::{
    CommandRevivalStore, SandboxEventStore, SandboxLifecycleCommandResult, SandboxTransaction,
    SandboxTransactionStore,
};

async fn pending() -> (
    Arc<InMemorySandboxStore>,
    ResourceAccessService,
    Arc<Authority>,
    Agent,
    AdmittedAgentCommand,
) {
    let (store, access, authority, mut agent) = execution_fixture(MAX_MEMBERSHIP_AGE).await;
    grant(&store, ShareMode::OrgRoles).await;
    let claimed = claim(&store, &agent, "admin").await;
    store
        .release_command_claim(
            &claimed.work.lease.as_ref().unwrap().claim,
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
    let command = store
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.status = SandboxStatus::Paused;
    parent.runtime = None;
    parent.lifecycle.checkpoints.latest =
        Some(crate::sandbox::lifecycle_test_support::checkpoint(1, NOW));
    parent.revision += 1;
    parent.validate().unwrap();
    store.save_sandbox(revision, parent).await.unwrap();
    let revision = agent.revision;
    agent.status = AgentStatus::Paused;
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    (store, access, authority, agent, command)
}

async fn proof(
    access: &ResourceAccessService,
    command: &AdmittedAgentCommand,
) -> Arc<CommandRevivalAuthorization> {
    access
        .authorize_command_revival(&context("admin"), "selected-credential", command)
        .await
        .unwrap()
}

async fn transaction(
    store: &InMemorySandboxStore,
    proof: Arc<CommandRevivalAuthorization>,
) -> SandboxTransaction {
    let mut parent = proof.current_snapshot().await.unwrap().lineage.sandbox;
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap().unwrap(),
        parent
    );
    let expected_revision = parent.revision;
    let event_at = crate::now_rfc3339();
    let from = parent.status;
    parent
        .begin_revive(SandboxReviveCause::AgentMessage, event_at.clone())
        .unwrap();
    SandboxTransaction::SaveSandboxAndAppend {
        command_revival: Some(proof),
        released_runtime: None,
        expected_revision,
        sandbox: Box::new(parent),
        event_at,
        event: SandboxEventKind::StatusChanged {
            from,
            to: SandboxStatus::Reviving,
        },
    }
}

#[tokio::test]
async fn shared_revival_preserves_command_and_conversation_and_uses_owner_connections() {
    let (store, access, _, agent, command) = pending().await;
    let before = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let checked = Arc::new(Mutex::new(Vec::new()));
    let service = SandboxService::new(
        store.clone(),
        Arc::new(crate::profiles::InMemoryProfileStore::default()),
        SandboxPolicy::default(),
        Arc::new(OwnerConnections(checked.clone())),
    );
    let authorization = proof(&access, &command).await;
    let SandboxLifecycleCommandResult::Accepted(first) = service
        .revive_for_command(authorization.clone())
        .await
        .unwrap()
    else {
        panic!("revival should be accepted")
    };
    assert_eq!(
        first.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert_eq!(first.sandbox.status, SandboxStatus::Reviving);
    assert_eq!(first.sandbox.owner, before.owner);
    assert_eq!(first.sandbox.profile, before.profile);
    assert_eq!(first.sandbox.credential, before.credential);
    assert_eq!(*checked.lock().unwrap(), vec![before.owner]);
    let SandboxLifecycleCommandResult::Accepted(replay) =
        store.schedule_command_revival(authorization).await.unwrap()
    else {
        panic!("replay should observe scheduled work")
    };
    assert_eq!(
        replay.disposition,
        SandboxLifecycleCommandDisposition::AlreadyScheduled
    );
    assert_eq!(replay.sandbox, first.sandbox);
    let audits = store
        .command_authorization_audits(
            &command.receipt.envelope.command_id,
            &crate::sandbox::store::PageRequest::new(20, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audits.items.len(), 1);
    assert_eq!(
        audits.items[0].effect,
        CommandAuthorizationEffect::Revival {
            sandbox_revision: first.sandbox.revision
        }
    );
    assert_eq!(audits.items[0].resources.len(), 2);
    assert_eq!(
        audits.items[0].resources[0].capability,
        ResourceCapability::Interact
    );
    assert_eq!(
        audits.items[0].resources[1].capability,
        ResourceCapability::ManageSandbox
    );
    assert_eq!(
        store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(command)
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(
        store
            .sandbox_events(&agent.sandbox_id, 0, 50)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn revival_transaction_refuses_changed_policy_command_and_transition() {
    for change in [
        "policy", "command", "envelope", "services", "event", "target",
    ] {
        let (store, access, _, agent, command) = pending().await;
        let mut selected = command.clone();
        if change == "envelope" {
            selected.receipt.envelope.correlation_id = "another-command".into();
        }
        let authorization = proof(&access, &selected).await;
        let mut mutation = transaction(&store, authorization).await;
        match change {
            "policy" => {
                let snapshot = access
                    .authorize_personal(
                        &context("owner").caller,
                        &ResourceTarget::Sandbox {
                            sandbox_id: agent.sandbox_id.clone(),
                        },
                        ResourceCapability::ManageSharing,
                    )
                    .await
                    .unwrap()
                    .snapshot;
                let mut policy = snapshot.policy.unwrap();
                policy.revision += 1;
                store.set_resource_policy_for_test(policy);
            }
            "command" => store
                .reject_unclaimed_command(
                    &command.receipt.envelope.command_id,
                    command.revision,
                    CommandFailureCode::AuthorityRevoked,
                    crate::now_rfc3339(),
                )
                .await
                .unwrap(),
            "services" => {
                if let SandboxTransaction::SaveSandboxAndAppend { sandbox, .. } = &mut mutation {
                    sandbox.name = "unauthorized-edit".into();
                }
            }
            "event" => {
                if let SandboxTransaction::SaveSandboxAndAppend { event, .. } = &mut mutation {
                    *event = SandboxEventKind::WorkStepCompleted {
                        step: SandboxWorkStep::RunLifecycleOperation,
                    };
                }
            }
            "target" => {
                let mut stopped = agent.clone();
                stopped.requested_stop_reason = Some(AgentStopReason::Owner);
                stopped.revision += 1;
                store.save_agent(agent.revision, stopped).await.unwrap();
            }
            _ => {}
        }
        let before_parent = store.get_sandbox(&agent.sandbox_id).await.unwrap();
        let before_command = store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap();
        assert!(store.commit(mutation).await.is_err(), "{change}");
        assert_eq!(
            store.get_sandbox(&agent.sandbox_id).await.unwrap(),
            before_parent,
            "{change}"
        );
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            before_command,
            "{change}"
        );
        assert!(store
            .sandbox_events(&agent.sandbox_id, 0, 50)
            .await
            .unwrap()
            .is_empty());
    }
}

struct ChangedRole(AtomicUsize);
#[async_trait]
impl OrganizationAuthority for ChangedRole {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut current = context(caller.subject.as_str());
        let role = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            OrganizationRole::Admin
        } else {
            OrganizationRole::Viewer
        };
        let now = SystemTime::now();
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            org.clone(),
            role,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(current)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}

#[tokio::test]
async fn agent_interaction_does_not_replace_fresh_parent_management_authority() {
    let (store, _, _, agent, command) = pending().await;
    let authority = Arc::new(ChangedRole(AtomicUsize::new(0)));
    let access = ResourceAccessService::new(store.clone(), authority.clone());
    assert!(access
        .authorize_command_revival(&context("admin"), "selected-credential", &command)
        .await
        .is_err());
    assert_eq!(authority.0.load(Ordering::SeqCst), 2);
    assert_eq!(
        store
            .get_sandbox(&agent.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Paused
    );
    assert!(store
        .sandbox_events(&agent.sandbox_id, 0, 50)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn held_revival_proof_expires_even_when_wall_clock_has_not_advanced() {
    let (store, access, _, agent, command) = pending().await;
    let authorization = proof(&access, &command).await;
    let mutation = transaction(&store, authorization).await;
    tokio::time::pause();
    tokio::time::advance(MAX_MEMBERSHIP_AGE + Duration::from_secs(1)).await;
    assert!(store.commit(mutation).await.is_err());
    assert_eq!(
        store
            .get_sandbox(&agent.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Paused
    );
    assert!(store
        .sandbox_events(&agent.sandbox_id, 0, 50)
        .await
        .unwrap()
        .is_empty());
    tokio::time::resume();
}

struct ChangedConnections(Arc<InMemorySandboxStore>);
#[async_trait]
impl SandboxCredentialPrerequisite for ChangedConnections {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        assert_eq!(owner, &SubjectId::new("owner"));
        let mut parent = self
            .0
            .get_sandbox(&SandboxId("sandbox".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = parent.revision;
        parent.profile.revision += 1;
        parent
            .profile
            .content
            .nyxid_services
            .push(crate::ServiceId::new("changed-fixture-service"));
        parent.revision += 1;
        self.0.save_sandbox(revision, parent).await.unwrap();
        Ok(())
    }
}

#[tokio::test]
async fn changed_parent_configuration_during_connections_cannot_revive_on_stale_check() {
    let (store, access, _, agent, command) = pending().await;
    let service = SandboxService::new(
        store.clone(),
        Arc::new(crate::profiles::InMemoryProfileStore::default()),
        SandboxPolicy::default(),
        Arc::new(ChangedConnections(store.clone())),
    );
    let authorization = proof(&access, &command).await;
    assert!(service.revive_for_command(authorization).await.is_err());
    assert_eq!(
        store
            .get_sandbox(&agent.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Paused
    );
    assert!(store
        .sandbox_events(&agent.sandbox_id, 0, 50)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn command_revival_preserves_existing_pause_cancellation_protocol() {
    let (store, access, _, agent, command) = pending().await;
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.status = SandboxStatus::Active;
    parent.runtime = crate::sandbox::lifecycle_test_support::sandbox().runtime;
    parent.begin_pause(crate::now_rfc3339(), None).unwrap();
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    let authority = parent
        .lifecycle
        .operation
        .as_ref()
        .unwrap()
        .authority()
        .clone();
    let proof = proof(&access, &command).await;
    let SandboxLifecycleCommandResult::Accepted(result) =
        store.schedule_command_revival(proof).await.unwrap()
    else {
        panic!("pause cancellation should be scheduled")
    };
    assert_eq!(result.sandbox.status, SandboxStatus::Pausing);
    assert_eq!(
        result
            .sandbox
            .lifecycle
            .operation
            .as_ref()
            .unwrap()
            .authority(),
        &authority
    );
    assert!(matches!(
        result.sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause {
            phase: SandboxPausePhase::ResumeWriters,
            ..
        })
    ));
    assert_eq!(
        store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(command)
    );
}

#[tokio::test]
async fn queue_deadline_shortens_revival_authority_and_controls_cannot_request_it() {
    let (_, access, _, _, mut command) = pending().await;
    command.queue_deadline = crate::rfc3339_at(SystemTime::now() + Duration::from_secs(2));
    let authorization = proof(&access, &command).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    assert!(authorization.require_current().is_err());
    tokio::time::resume();
    command.receipt.envelope.command = AgentCommand::Interrupt;
    command.receipt.envelope.request_fingerprint =
        command.receipt.envelope.command.fingerprint().unwrap();
    assert!(access
        .authorize_command_revival(&context("admin"), "selected-credential", &command)
        .await
        .is_err());
}
