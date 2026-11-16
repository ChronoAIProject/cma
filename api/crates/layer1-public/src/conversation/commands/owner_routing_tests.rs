//! Exercise the real service and legacy spawn boundary, with an ingress store
//! that refuses writes and an independently owned service over the shared ledger.
use crate::{
    conversation::command_test_support::{self, context, Authority},
    rest::agents_tests as fixtures,
};
use async_trait::async_trait;
use layer2_core::{
    agent_ownership::{
        test_support::MemoryOwnershipStore, Authority as OwnerAuthority, Member,
        OwnershipCoordinator, OwnershipToken, PROTOCOL_VERSION,
    },
    resource_access::{CallerContext, ResourceAccessService},
    sandbox::{
        agent_io::*,
        store::{InMemorySandboxStore, PageRequest},
        AgentStatus,
    },
    AgentId,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

mod legacy;
mod refusals;

const ID: &str = "agt_owner_routing";

struct IngressWrites(AtomicUsize, Arc<InMemorySandboxStore>);
#[async_trait]
impl AgentCommandStore for IngressWrites {
    async fn reserve_command(
        &self,
        _: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(CommandLedgerError::Unavailable)
    }
    async fn get_command(
        &self,
        id: &layer2_core::ids::AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.1.get_command(id).await
    }
    async fn get_response_command(
        &self,
        id: &layer2_core::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.1.get_response_command(id).await
    }
    async fn get_admitted_command(
        &self,
        scope: &str,
        key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.1.get_admitted_command(scope, key).await
    }
}

struct Router {
    owner: Arc<AgentCommandService>,
    authority: Arc<OwnerAuthority>,
    _coordinator: Arc<OwnershipCoordinator>,
    mutations: Mutex<Vec<OwnerCommandMutation>>,
    failure: Option<&'static str>,
    evidence: Mutex<Vec<Option<Vec<u8>>>>,
}
#[async_trait]
impl AgentCommandOwnerRouting for Router {
    async fn route(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        mutation: OwnerCommandMutation,
    ) -> Result<OwnerCommandReply, CommandAdmissionError> {
        self.mutations.lock().unwrap().push(mutation.clone());
        self.evidence
            .lock()
            .unwrap()
            .push(OwnerCommandRoutingEvidence::current().map(|value| value.bytes().to_vec()));
        if self.failure == Some("unavailable") {
            return Err(CommandLedgerError::Unavailable.into());
        }
        let mut result = self
            .authority
            .run(
                self.owner
                    .execute_owner_mutation(context, credential, source, mutation),
            )
            .await
            .map_err(|_| CommandLedgerError::Unavailable)??;
        if self.failure == Some("reply_lost") {
            return Err(CommandLedgerError::Unavailable.into());
        }
        if self.failure == Some("wrong_agent") {
            if let OwnerCommandReply::Admit(Ok(value)) = &mut result {
                value.receipt.agent_id = AgentId("agt_other".into());
            }
        }
        Ok(result)
    }
}

struct Runtime;
#[async_trait]
impl layer2_core::sandbox::AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &layer2_core::sandbox::AgentRuntimeTarget,
    ) -> Result<layer2_core::sandbox::AgentRuntimeConnection, layer2_core::sandbox::AgentRuntimeError>
    {
        use layer2_core::sandbox::*;
        Ok(AgentRuntimeConnection {
            capabilities: layer2_core::RuntimeCapabilities::new([
                layer2_core::RuntimeCapability::Conversation,
                layer2_core::RuntimeCapability::Interrupt,
            ]),
            input: AgentRuntimeInputSender::spawn(8, |_| async { Ok(()) }),
        })
    }
    async fn fence(
        &self,
        _: &AgentId,
        _: u64,
    ) -> Result<(), layer2_core::sandbox::AgentRuntimeError> {
        Ok(())
    }
}

struct Fixture {
    state: crate::AgentConversationState,
    store: Arc<InMemorySandboxStore>,
    ingress: Arc<IngressWrites>,
    service: Arc<AgentCommandService>,
    router: Arc<Router>,
}
fn ownership(agent: &str, epoch: u64) -> Arc<OwnerAuthority> {
    OwnerAuthority::new(
        OwnershipToken {
            agent_id: AgentId(agent.into()),
            instance_id: "am-owner".into(),
            epoch,
            protocol: PROTOCOL_VERSION,
        },
        tokio::time::Instant::now() + Duration::from_secs(30),
    )
}
async fn fixture(failure: Option<&'static str>) -> Fixture {
    let (state, store) = fixtures::conversation_fixture_with_registry(Arc::new(Runtime)).await;
    let mut state = command_test_support::compose(state, store.clone());
    fixtures::insert_agent(
        &store,
        fixtures::seeded_agent(ID, AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let owner = state.commands.clone().unwrap();
    let ingress = Arc::new(IngressWrites(AtomicUsize::new(0), store.clone()));
    let authority = Arc::new(Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let ownership_store = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::register(
        ownership_store.client(),
        Member {
            instance_id: "am-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap();
    let owner_authority = coordinator
        .acquire(&AgentId(ID.into()))
        .await
        .unwrap()
        .unwrap();
    assert!(owner_authority.token.epoch > 0);
    let router = Arc::new(Router {
        owner,
        authority: owner_authority,
        _coordinator: coordinator,
        mutations: Mutex::new(vec![]),
        failure,
        evidence: Mutex::new(vec![]),
    });
    let service = AgentCommandService::new(ingress.clone(), access, authority)
        .with_owner_routing(router.clone());
    state.commands = Some(service.clone());
    Fixture {
        state,
        store,
        ingress,
        service,
        router,
    }
}
fn submission(key: &str) -> CommandSubmission {
    CommandSubmission {
        agent_id: AgentId(ID.into()),
        command: AgentCommand::message("Next turn"),
        idempotency_key: key.into(),
        expected_agent_revision: None,
        requested_model: None,
    }
}
async fn admit(
    f: &Fixture,
    request: CommandSubmission,
) -> Result<CommandStatusView, CommandAdmissionError> {
    f.service
        .admit(
            &context("owner-a"),
            "fixture-credential",
            CommandSubmissionSource::Api,
            request,
        )
        .await
}
async fn records(f: &Fixture) -> Vec<AdmittedAgentCommand> {
    f.store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
}

#[tokio::test]
async fn nonowner_admission_commits_on_owner_and_exact_retry_retains_receipt() {
    let f = fixture(None).await;
    let first = admit(&f, submission("same-key")).await.unwrap();
    let again = admit(&f, submission("same-key")).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
    assert_eq!(records(&f).await.len(), 1);
    assert_eq!(f.router.mutations.lock().unwrap().len(), 2);
    let mut changed = submission("same-key");
    changed.command = AgentCommand::message("Changed body");
    assert!(matches!(
        admit(&f, changed).await,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::IdempotencyConflict
        ))
    ));
    assert_eq!(records(&f).await.len(), 1);
}

#[tokio::test]
async fn routing_retains_all_three_binding_policy_states() {
    let f = fixture(None).await;
    admit(&f, submission("ordinary")).await.unwrap();
    f.service
        .admit_at_binding_revision(
            &context("owner-a"),
            "fixture-credential",
            CommandSubmissionSource::Api,
            submission("absent-policy"),
            None,
        )
        .await
        .unwrap();
    let result = f
        .service
        .admit_at_binding_revision(
            &context("owner-a"),
            "fixture-credential",
            CommandSubmissionSource::Api,
            submission("specific-policy"),
            Some(7),
        )
        .await;
    assert!(matches!(
        result,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    let values: Vec<_> = f
        .router
        .mutations
        .lock()
        .unwrap()
        .iter()
        .map(|value| match value {
            OwnerCommandMutation::Admit { binding_policy, .. } => *binding_policy,
            _ => panic!("wrong mutation"),
        })
        .collect();
    assert_eq!(values, [None, Some(None), Some(Some(7))]);
    assert_eq!(records(&f).await.len(), 2);
    assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn caller_explicit_stale_revision_is_not_refreshed_by_owner_routing() {
    let f = fixture(None).await;
    let mut request = submission("stale-revision");
    request.expected_agent_revision = Some(99);
    assert!(matches!(
        admit(&f, request).await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    assert!(
        matches!(&f.router.mutations.lock().unwrap()[0], OwnerCommandMutation::Admit { submission, .. }
        if submission.expected_agent_revision == Some(99))
    );
    assert!(records(&f).await.is_empty());
    assert_eq!(f.ingress.0.load(Ordering::SeqCst), 0);
}
