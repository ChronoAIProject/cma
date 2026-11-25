use super::*;
use layer2_core::sandbox::store::{CommandRevivalStore, SandboxLifecycleCommandResult};
use layer2_core::sandbox::{
    SandboxCheckpointRef, SandboxLifecycleCommandDisposition, SandboxReviveCause,
};
use layer3_adapter::{CheckpointReceipt, WriterState};

struct Fixture {
    store: Arc<MongoSandboxStore>,
    agent: Agent,
    parent: Sandbox,
    command: AdmittedAgentCommand,
    execution: Arc<crate::commands::ResourceCommandExecution>,
    sends: Arc<AtomicUsize>,
}

impl Fixture {
    async fn new(client: &Client, name: &str) -> Self {
        let (store, agent, mut parent, execution, _, sends) = composed(client, name).await;
        grant(&store, &parent).await;
        let claimed = admit_shared(&store, &agent, &parent).await;
        store
            .release_command_claim(
                &claimed.work.lease.as_ref().unwrap().claim,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let command = store
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        let revision = parent.revision;
        parent.status = SandboxStatus::Paused;
        parent.runtime = None;
        parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
        parent.lifecycle.checkpoints.latest = Some(
            SandboxCheckpointRef::seal(
                &CheckpointReceipt {
                    epoch: 1,
                    object_reference: "checkpoints/revival".into(),
                    sha256: "a".repeat(64),
                    writers: WriterState::Resumed,
                },
                &[],
                NOW,
            )
            .unwrap(),
        );
        parent.revision += 1;
        parent.validate().unwrap();
        store.save_sandbox(revision, parent.clone()).await.unwrap();
        Self {
            store,
            agent,
            parent,
            command,
            execution,
            sends,
        }
    }

    async fn proof(&self, lifetime: Duration) -> Arc<CommandRevivalAuthorization> {
        ResourceAccessService::new(self.store.clone(), Arc::new(Membership(lifetime)))
            .authorize_command_revival(&context(), "fixture-credential", &self.command)
            .await
            .unwrap()
    }

    fn mutation(&self, proof: Arc<CommandRevivalAuthorization>) -> SandboxTransaction {
        let mut next = self.parent.clone();
        let event_at = layer2_core::now_rfc3339();
        next.begin_revive(SandboxReviveCause::AgentMessage, event_at.clone())
            .unwrap();
        SandboxTransaction::SaveSandboxAndAppend {
            command_revival: Some(proof),
            released_runtime: None,
            expected_revision: self.parent.revision,
            sandbox: Box::new(next),
            event_at,
            event: SandboxEventKind::StatusChanged {
                from: SandboxStatus::Paused,
                to: SandboxStatus::Reviving,
            },
        }
    }

    async fn documents(&self) -> (Option<Document>, Option<Document>, Vec<SandboxEvent>) {
        (
            self.store
                .sandboxes
                .find_one(doc! {"_id":self.parent.id.as_str()})
                .await
                .unwrap(),
            self.store
                .agent_commands
                .find_one(doc! {"_id":self.command.receipt.envelope.command_id.as_str()})
                .await
                .unwrap(),
            self.store
                .sandbox_events(&self.parent.id, 0, 100)
                .await
                .unwrap(),
        )
    }
}

struct ResourceAuthority;
#[async_trait::async_trait]
impl AgentCommandAuthorizer for ResourceAuthority {
    async fn authorize_command(
        &self,
        _: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError> {
        Err(CommandAuthorizationError::Revoked) // personal compatibility is unavailable to this shared actor
    }
    async fn authorize_resource_command(
        &self,
        access: &ResourceAccessService,
        envelope: &AgentCommandEnvelope,
    ) -> Result<CommandExecutionAuthorization, ResourceCommandAuthorizationError> {
        Ok(access
            .authorize_command_execution(&context(), "fixture-credential", envelope)
            .await?)
    }
    async fn authorize_command_revival(
        &self,
        access: &ResourceAccessService,
        command: &AdmittedAgentCommand,
    ) -> Result<Arc<CommandRevivalAuthorization>, ResourceCommandAuthorizationError> {
        Ok(access
            .authorize_command_revival(&context(), "fixture-credential", command)
            .await?)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn production_shared_preparation_revives_once_without_sending_or_rewriting_command() {
    run_shared_revival(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_stop_keeps_admitted_command_revival_exactly_once() {
    run_shared_revival(true).await;
}

async fn run_shared_revival(stopped: bool) {
    let (client, name) = database("command_revival_prepare").await;
    let mut fixture = Fixture::new(&client, &name).await;
    if stopped {
        use layer2_core::admission::AdmissionControlStore;
        crate::store::admission_control::MongoAdmissionControlStore::open(&client, &name)
            .await
            .unwrap()
            .change(
                0,
                &SubjectId::new("operator"),
                "contain new work",
                true,
                100,
            )
            .await
            .unwrap();
        let service = SandboxService::new(
            fixture.store.clone(),
            Arc::new(InMemoryProfileStore::default()),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        );
        service.attach_platform_spend(fixture.store.clone());
        let proof = fixture.proof(MAX_MEMBERSHIP_AGE).await;
        service.revive_for_command(proof).await.unwrap();
        let first = fixture
            .store
            .get_sandbox(&fixture.parent.id)
            .await
            .unwrap()
            .unwrap();
        let proof = fixture.proof(MAX_MEMBERSHIP_AGE).await;
        service.revive_for_command(proof).await.unwrap();
        assert_eq!(
            fixture.store.get_sandbox(&fixture.parent.id).await.unwrap(),
            Some(first)
        );
    }
    let revision = fixture.agent.revision;
    fixture.agent.status = AgentStatus::Paused;
    fixture.agent.revision += 1;
    fixture
        .store
        .save_agent(revision, fixture.agent.clone())
        .await
        .unwrap();
    let execution = Arc::new(crate::commands::ResourceCommandExecution {
        source_guard: None,
        authority: Arc::new(ResourceAuthority),
        access: Arc::new(ResourceAccessService::new(
            fixture.store.clone(),
            Arc::new(Membership(MAX_MEMBERSHIP_AGE)),
        )),
        store: fixture.store.clone(),
        operations: fixture.execution.operations.clone(),
        context: fixture.execution.context.clone(),
    });
    assert_eq!(
        execution.check_queued(&fixture.command).await,
        CommandReadiness::Ready
    );
    let worker = AgentCommandWorker::new(
        fixture.store.clone(),
        execution.clone(),
        "revival-worker".into(),
    )
    .unwrap();
    worker
        .run_one(&fixture.command.receipt.envelope.command_id)
        .await
        .unwrap();
    let first = fixture
        .store
        .get_sandbox(&fixture.parent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.status, SandboxStatus::Reviving);
    let audits = fixture
        .store
        .command_authorization_audits(
            &fixture.command.receipt.envelope.command_id,
            &PageRequest::new(20, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audits.items.len(), 1);
    assert_eq!(
        audits.items[0].resources[0].capability,
        ResourceCapability::Interact
    );
    assert_eq!(
        audits.items[0].resources[1].capability,
        ResourceCapability::ManageSandbox
    );
    worker
        .run_one(&fixture.command.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(
        fixture.store.get_sandbox(&fixture.parent.id).await.unwrap(),
        Some(first)
    );
    assert_eq!(
        fixture
            .store
            .get_command(&fixture.command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(fixture.command.clone())
    );
    assert_eq!(
        fixture.store.get_agent(&fixture.agent.id).await.unwrap(),
        Some(fixture.agent)
    );
    assert_eq!(fixture.sends.load(Ordering::SeqCst), 0);
    let row = fixture
        .store
        .agent_commands
        .find_one(doc! {"_id":fixture.command.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(row.get_str("last_command_revival").is_ok());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn revival_transaction_refuses_claim_rejection_policy_and_unrelated_candidate_changes() {
    for change in ["claim", "reject", "policy", "candidate", "envelope"] {
        let (client, name) = database("command_revival_fences").await;
        let mut fixture = Fixture::new(&client, &name).await;
        if change == "envelope" {
            fixture.command.receipt.envelope.correlation_id = "another-correlation".into();
        }
        let proof = fixture.proof(MAX_MEMBERSHIP_AGE).await;
        let mut mutation = fixture.mutation(proof);
        match change {
            "claim" => {
                fixture
                    .store
                    .claim_next_command(claim_request(
                        &fixture.agent,
                        "new-worker",
                        &layer2_core::now_rfc3339(),
                    ))
                    .await
                    .unwrap()
                    .unwrap();
            }
            "reject" => fixture
                .store
                .reject_unclaimed_command(
                    &fixture.command.receipt.envelope.command_id,
                    fixture.command.revision,
                    CommandFailureCode::AuthorityRevoked,
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap(),
            "policy" => {
                fixture
                    .store
                    .resource_policies
                    .update_one(
                        doc! {"_id":fixture.parent.id.as_str()},
                        doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
                    )
                    .await
                    .unwrap();
            }
            "candidate" => {
                if let SandboxTransaction::SaveSandboxAndAppend { sandbox, .. } = &mut mutation {
                    sandbox.name = "unrelated-edit".into();
                }
            }
            _ => {}
        }
        let before = fixture.documents().await;
        assert!(fixture.store.commit(mutation).await.is_err(), "{change}");
        assert_eq!(fixture.documents().await, before, "{change}");
        client.database(&name).drop().await.unwrap();
    }
}

#[path = "command_revival_tests/commit_outcomes.rs"]
mod commit_outcomes;

pub(in crate::store::sandbox_mongo_integration_tests) async fn revive_managed_sandbox(
    source: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
) -> (Sandbox, Sandbox) {
    use crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home;

    let (client, name) = database("command_revival_release").await;
    let fixture = Fixture::new(&client, &name).await;
    let mut original = fixture.parent.clone();
    let home = home(&original.id);
    let runtime_version = home
        .native_pool
        .as_ref()
        .unwrap()
        .request
        .pool
        .spec
        .image
        .uri
        .clone();
    original.lifecycle.home = Some(Box::new(home));
    original.lifecycle.suspended_runtime = Some(Box::new(RuntimeBinding {
        runtime_id: "agent-sandbox:member:sandbox-uid".into(),
        runtime_version,
        provision_generation: Some(1),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    }));
    original.revision += 1;
    original.validate().unwrap();
    assert!(original.supports_retained_runtime_update());
    fixture
        .store
        .save_sandbox(fixture.parent.revision, original.clone())
        .await
        .unwrap();
    let proof = fixture.proof(MAX_MEMBERSHIP_AGE).await;
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_released_runtime(source);
    let result = store.schedule_command_revival(proof).await;
    let persisted = store.get_sandbox(&original.id).await.unwrap().unwrap();
    client.database(&name).drop().await.unwrap();
    let SandboxLifecycleCommandResult::Accepted(outcome) = result.unwrap() else {
        panic!("managed command revival must be admitted");
    };
    assert_eq!(
        outcome.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert_eq!(persisted, outcome.sandbox);
    assert_eq!(persisted.status, SandboxStatus::Reviving);
    (persisted, original)
}
