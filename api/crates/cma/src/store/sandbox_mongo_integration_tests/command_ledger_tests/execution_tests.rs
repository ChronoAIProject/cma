use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime};

use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::resource_access::*;
use layer2_core::sandbox::*;
use layer2_core::{
    Authorization, Caller, Capability, PlatformDefaults, RuntimeCapabilities, RuntimeCapability,
};
use layer3_adapter::ConversationInputSender;

use super::work_tests::{claim_request, live_request};
use super::*;

#[path = "resource_execution_tests.rs"]
mod resource_execution;

#[path = "capacity_execution_tests.rs"]
mod capacity_execution;

struct Authority(Mutex<Result<Caller, CommandAuthorizationError>>);
#[async_trait::async_trait]
impl AgentCommandAuthorizer for Authority {
    async fn authorize_command(
        &self,
        _: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError> {
        self.0.lock().unwrap().clone()
    }
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal worker cannot request organization membership")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal worker cannot enumerate organizations")
    }
}

struct Runtime(ConversationInputSender);
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("command dispatch cannot retire a runtime")
    }
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: self.0.clone(),
        })
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn personal_command_binding_checks_explicit_owner_kind_in_the_transaction() {
    let (client, name) = database("command_execution_owner_kind").await;
    let (store, agent, parent, execution, authority, sends) = composed(&client, &name).await;
    store
        .reserve_command(live_request(&agent, &parent, "one"))
        .await
        .unwrap();
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let policy = ResourcePolicyRecord {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: parent.owner.clone(),
        },
        revision: 1,
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "https://identity.test".into(),
            organization: parent.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("admin"),
            },
            verified_at: NOW.into(),
        }),
    };
    store.resource_policies.insert_one(doc! { "_id": parent.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION, "owner_kind": "organization", "owner_subject": parent.owner.as_str(), "revision": 1_i64, "record": crate::store::sandbox_mongo::record_bson(&policy).unwrap() }).await.unwrap();
    assert_eq!(
        execution.check_queued(&claimed).await,
        CommandReadiness::Reject(CommandFailureCode::AgentUnavailable)
    );
    // Bypass the adapter's earlier snapshot to prove the atomic store fence too.
    let caller = authority.0.lock().unwrap().clone().unwrap();
    assert_eq!(
        execution
            .operations
            .execute_claimed_command(&caller, &claimed)
            .await,
        Err(AgentOperationError::Conflict)
    );
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(claimed)
    );
    client.database(&name).drop().await.unwrap();
}

async fn composed(
    client: &Client,
    name: &str,
) -> (
    Arc<MongoSandboxStore>,
    Agent,
    Sandbox,
    Arc<crate::commands::ResourceCommandExecution>,
    Arc<Authority>,
    Arc<AtomicUsize>,
) {
    composed_with_credentials(
        client,
        name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        active_sandbox("sbx_commands"),
    )
    .await
}

async fn composed_with_credentials(
    client: &Client,
    name: &str,
    credentials: Arc<dyn SandboxCredentialPrerequisite>,
    parent: Sandbox,
) -> (
    Arc<MongoSandboxStore>,
    Agent,
    Sandbox,
    Arc<crate::commands::ResourceCommandExecution>,
    Arc<Authority>,
    Arc<AtomicUsize>,
) {
    let store = MongoSandboxStore::open(client, name).await.unwrap();
    let agent = ready_agent(&parent, "agt_commands");
    store.create_sandbox(parent.clone()).await.unwrap();
    insert_agent(&store, &agent, NOW).await;
    let workspace = ready_workspace(&parent, agent.workspace_id.as_str(), "project");
    let (mut worktree, _) = worktree_and_agent(&parent.id);
    worktree.id = agent.worktree_id.clone();
    worktree.workspace_id = workspace.id.clone();
    worktree.agent_id = Some(agent.id.clone());
    store.create_workspace(workspace).await.unwrap();
    store.create_worktree(worktree).await.unwrap();
    let store = Arc::new(store);
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let authority = Arc::new(Authority(Mutex::new(Ok(caller))));
    let sends = Arc::new(AtomicUsize::new(0));
    let sent = sends.clone();
    let runtime = Arc::new(Runtime(ConversationInputSender::spawn(4, move |_| {
        sent.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    })));
    let profiles = Arc::new(InMemoryProfileStore::default());
    let context = AgentContextService::new(
        SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            credentials.clone(),
        ),
        WorkspaceService::new(store.clone(), profiles.clone()),
        AgentService::new(
            store.clone(),
            profiles.clone(),
            runtime.clone(),
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        ),
        profiles,
        Arc::new(PlatformDefaults::default()),
    );
    let execution = Arc::new(crate::commands::ResourceCommandExecution {
        source_guard: None,
        authority: authority.clone(),
        access: Arc::new(ResourceAccessService::new(store.clone(), authority.clone())),
        store: store.clone(),
        operations: AgentOperationService::new(store.clone(), runtime)
            .with_connections(credentials),
        context,
    });
    (store, agent, parent, execution, authority, sends)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn production_execution_adapter_dispatches_once_and_reconciles_after_restart() {
    let (client, name) = database("command_execution").await;
    let (store, agent, parent, execution, authority, sends) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    let worker =
        AgentCommandWorker::new(store.clone(), execution.clone(), "replica-one".into()).unwrap();
    worker.run_one(id).await.unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    *authority.0.lock().unwrap() = Err(CommandAuthorizationError::Revoked);
    AgentCommandWorker::new(store.clone(), execution, "replica-two".into())
        .unwrap()
        .run_one(id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let current = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(current.receipt, admitted.receipt);
    assert_eq!(current.status, AgentResponseStatus::InProgress);
    assert!(current.work.failure.is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn production_authorization_refuses_missing_authority_and_actor_substitution() {
    let (client, name) = database("command_execution_denied").await;
    let (store, agent, parent, execution, authority, sends) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    for (answer, readiness, attempt) in [
        (
            Err(CommandAuthorizationError::Revoked),
            CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked),
            CommandAttempt::Reject(CommandFailureCode::AuthorityRevoked),
        ),
        (
            Err(CommandAuthorizationError::Unavailable),
            CommandReadiness::Retry,
            CommandAttempt::Retry,
        ),
        (
            Ok(Caller::new(
                SubjectId::new("other"),
                Authorization::new([Capability::UseOwnSandboxes]),
            )),
            CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked),
            CommandAttempt::Reject(CommandFailureCode::AuthorityRevoked),
        ),
    ] {
        *authority.0.lock().unwrap() = answer;
        assert_eq!(execution.check_queued(&admitted).await, readiness);
        assert!(execution.prepare_queued(&admitted).await.is_err());
        assert_eq!(execution.execute(&admitted).await, attempt);
    }
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .get_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(admitted)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn execution_uses_current_renewed_expiry_without_adopting_a_replacement_claim() {
    let (client, name) = database("command_execution_renewed").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    store
        .reserve_command(live_request(&agent, &parent, "one"))
        .await
        .unwrap();
    let mut original = store
        .claim_next_command(claim_request(&agent, "worker", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    // A retained attempt copy can expire while the worker renews the same claim.
    original.work.lease.as_mut().unwrap().expires_at =
        layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(1));
    assert_eq!(execution.execute(&original).await, CommandAttempt::Finished);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(execution.execute(&original).await, CommandAttempt::Retry);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn replaced_control_turn_has_typed_refusal_before_runtime_or_lifecycle_work() {
    for original_turn in [None, Some(1)] {
        let (client, name) = database("command_execution_control").await;
        let (store, mut agent, parent, execution, _, sends) = composed(&client, &name).await;
        agent.status = if original_turn.is_some() {
            AgentStatus::Working
        } else {
            AgentStatus::Ready
        };
        agent.capabilities = RuntimeCapabilities::new([
            RuntimeCapability::Conversation,
            RuntimeCapability::Interrupt,
        ]);
        agent.conversation.active_turn = original_turn;
        store
            .agents
            .replace_one(
                doc! { "_id": agent.id.as_str() },
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        let mut reservation = live_request(&agent, &parent, "control");
        reservation.envelope.command = AgentCommand::Interrupt;
        reservation.response_id = None;
        reservation.envelope.request_fingerprint =
            reservation.envelope.command.fingerprint().unwrap();
        let reserved = store.reserve_command(reservation).await;
        if original_turn.is_none() {
            assert!(matches!(reserved, Err(CommandLedgerError::NotAdmissible)));
            assert_eq!(sends.load(Ordering::SeqCst), 0);
            assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
            assert_eq!(
                store.agent_commands.count_documents(doc! {}).await.unwrap(),
                0
            );
            client.database(&name).drop().await.unwrap();
            continue;
        }
        let command = record(reserved.unwrap());
        agent.conversation.active_turn = Some(2);
        agent.status = AgentStatus::Working;
        agent.revision += 1;
        store
            .agents
            .replace_one(
                doc! { "_id": agent.id.as_str() },
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        AgentCommandWorker::new(store.clone(), execution, "worker".into())
            .unwrap()
            .run_one(&command.receipt.envelope.command_id)
            .await
            .unwrap();
        let refused = store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            refused.work.failure.unwrap().code,
            CommandFailureCode::TargetTurnChanged
        );
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
        client.database(&name).drop().await.unwrap();
    }
}

#[path = "binding_execution_tests.rs"]
mod binding_execution;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operation_command_lookup_is_exact_and_rejects_duplicate_bindings() {
    let (client, name) = database("operation_command_lookup").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    AgentCommandWorker::new(store.clone(), execution, "lookup-worker".into())
        .unwrap()
        .run_one(id)
        .await
        .unwrap();
    let current = store.get_command(id).await.unwrap().unwrap();
    let operation = current.work.dispatch.as_ref().unwrap().operation_sequence;
    assert_eq!(
        store
            .get_operation_command(&agent.id, operation)
            .await
            .unwrap(),
        Some(current.clone())
    );
    assert!(store
        .get_operation_command(&AgentId("foreign-agent".into()), operation)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_operation_command(&agent.id, operation + 1)
        .await
        .unwrap()
        .is_none());
    let mut duplicate = current;
    duplicate.receipt.envelope.command_id = AgentCommandId::generate();
    duplicate.work.dispatch.as_mut().unwrap().claim.command_id =
        duplicate.receipt.envelope.command_id.clone();
    duplicate.receipt.response_id = Some(AgentResponseId::generate());
    duplicate.receipt.agent_order += 1;
    duplicate.key_sha256 = command_key_digest("duplicate-operation").unwrap();
    let document = crate::store::agent_commands::command_document(&duplicate).unwrap();
    store.agent_commands.insert_one(document).await.unwrap();
    assert_eq!(
        store.get_operation_command(&agent.id, operation).await,
        Err(CommandLedgerError::Corrupt)
    );
    client.database(&name).drop().await.unwrap();
}

#[path = "budget_execution_tests.rs"]
mod budget_execution;
