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
    let (client, name) = database("command_revival_prepare").await;
    let mut fixture = Fixture::new(&client, &name).await;
    let revision = fixture.agent.revision;
    fixture.agent.status = AgentStatus::Paused;
    fixture.agent.revision += 1;
    fixture
        .store
        .save_agent(revision, fixture.agent.clone())
        .await
        .unwrap();
    let execution = Arc::new(crate::commands::ResourceCommandExecution {
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

async fn failpoint_client() -> (Client, Client, String, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_revival_{suffix}");
    let app_name = format!("cma-revival-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    (
        admin,
        Client::with_options(options).unwrap(),
        name,
        app_name,
    )
}

async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn revival_expiry_during_transaction_io_rolls_back_command_marker_parent_and_event() {
    let (admin, client, name, app_name) = failpoint_client().await;
    let fixture = Fixture::new(&client, &name).await;
    let before = fixture.documents().await;
    let proof = fixture.proof(Duration::from_millis(500)).await;
    let mutation = fixture.mutation(proof);
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1_i32},
        "data":{"failCommands":["insert"],"appName":&app_name,"blockConnection":true,"blockTimeMS":1000_i32},
    }).await.unwrap();
    let result = fixture.store.commit(mutation).await;
    disable(&admin).await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(
        fixture
            .store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(fixture.documents().await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn concurrent_command_refusal_prevents_revival_from_a_transaction_snapshot() {
    let (admin, client, name, app_name) = failpoint_client().await;
    let fixture = Fixture::new(&client, &name).await;
    let replica = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let proof = fixture.proof(MAX_MEMBERSHIP_AGE).await;
    let mutation = fixture.mutation(proof);
    let configured = admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1_i32},
        "data":{"failCommands":["update"],"appName":&app_name,"blockConnection":true,"blockTimeMS":2000_i32},
    }).await.unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let store = fixture.store.clone();
    let pending = tokio::spawn(async move { store.commit(mutation).await });
    let reached = admin
        .database("admin")
        .run_command(doc! {
            "waitForFailPoint":"failCommand", "timesEntered":count+1, "maxTimeMS":5000_i32,
        })
        .await;
    let refused = replica
        .reject_unclaimed_command(
            &fixture.command.receipt.envelope.command_id,
            fixture.command.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await;
    let result = pending.await.unwrap();
    disable(&admin).await;
    reached.unwrap();
    refused.unwrap();
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(
        fixture
            .store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        fixture.store.get_sandbox(&fixture.parent.id).await.unwrap(),
        Some(fixture.parent.clone())
    );
    let row = fixture
        .store
        .agent_commands
        .find_one(doc! {"_id":fixture.command.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key("last_command_revival"));
    assert_eq!(
        fixture
            .store
            .get_command(&fixture.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::Failed
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ambiguous_revival_commit_recovers_after_authority_expiry_without_repeating_event() {
    let (admin, client, name, app_name) = failpoint_client().await;
    let fixture = Fixture::new(&client, &name).await;
    let proof = fixture.proof(Duration::from_millis(500)).await;
    let mutation = fixture.mutation(proof.clone());
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":&app_name,
            "blockConnection":true,"blockTimeMS":200_i32,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous revival commit"}},
    }).await.unwrap();
    let result = fixture.store.commit(mutation).await;
    disable(&admin).await;
    result.unwrap();
    assert!(proof.require_current().is_err());
    let audits = fixture
        .store
        .command_authorization_audits(
            &fixture.command.receipt.envelope.command_id,
            &PageRequest::new(20, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(audits.items.len(), 1);
    assert_eq!(audits.items[0].resources.len(), 2);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .command_authorization_audits(
                &fixture.command.receipt.envelope.command_id,
                &PageRequest::new(20, None).unwrap()
            )
            .await
            .unwrap(),
        audits
    );
    let before = fixture.documents().await;
    let proof = fixture.proof(MAX_MEMBERSHIP_AGE).await;
    let SandboxLifecycleCommandResult::Accepted(outcome) =
        fixture.store.schedule_command_revival(proof).await.unwrap()
    else {
        panic!("accepted revival should remain scheduled")
    };
    assert_eq!(
        outcome.disposition,
        SandboxLifecycleCommandDisposition::AlreadyScheduled
    );
    assert_eq!(fixture.documents().await, before);
    client.database(&name).drop().await.unwrap();
}
