use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use axum::Router;
use layer2_core::{
    agent_ownership::{
        execution_owner, test_support::MemoryOwnershipStore, Authority, Member,
        OwnershipCoordinator, PROTOCOL_VERSION,
    },
    resource_access::CallerContext,
    sandbox::{
        agent_io::*, store::InMemorySandboxStore, AgentReadService, Sandbox,
        SandboxResourceReadError, SandboxResourceReader, SandboxResourceReading,
    },
    AgentId, SubjectId,
};

pub(super) use super::super::fixtures::{call, context, intent, Fixture, PATH, SOURCE};

pub(super) const PASS: u8 = 0;
pub(super) const UNAVAILABLE: u8 = 1;
pub(super) const WRONG_VARIANT: u8 = 2;
pub(super) const NOT_ADMITTED: u8 = 3;
pub(super) const UNSUPPORTED: u8 = 4;
pub(super) const LOST_REPLY: u8 = 5;

struct Custody;

#[async_trait]
impl AgentCommandAdmissionAuthority for Custody {
    async fn command_admission_authority(
        &self,
        caller: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if credential != "fixture-credential"
            || caller.caller.subject != SubjectId::new("owner-a")
            || owner
                != &(AgentOwner::Person {
                    subject: SubjectId::new("owner-a"),
                })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: "fixture-custody".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

struct NoResources;

#[async_trait]
impl SandboxResourceReader for NoResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("command admission does not inspect native resources")
    }
}

/// In-process routing fixture with a coordinator-issued lease and real service
/// mutations. CMA's relay tests separately prove network/authentication custody.
pub(super) struct Route {
    pub(super) calls: Mutex<Vec<OwnerCommandMutation>>,
    pub(super) mode: AtomicU8,
    pub(super) authority: Arc<Authority>,
    owner: Arc<AgentCommandService>,
    _coordinator: Arc<OwnershipCoordinator>,
}

#[async_trait]
impl AgentCommandOwnerRouting for Route {
    async fn route(
        &self,
        caller: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        mutation: OwnerCommandMutation,
    ) -> Result<OwnerCommandReply, CommandAdmissionError> {
        assert_eq!(caller.caller.subject, SubjectId::new("owner-a"));
        assert_eq!(credential, "fixture-credential");
        assert!(matches!(source, CommandSubmissionSource::Api));
        self.calls.lock().unwrap().push(mutation.clone());
        match self.mode.load(Ordering::SeqCst) {
            UNAVAILABLE => return Err(CommandLedgerError::Unavailable.into()),
            WRONG_VARIANT => {
                return Ok(OwnerCommandReply::Admit(Err(
                    CommandLedgerError::Unavailable.into(),
                )))
            }
            NOT_ADMITTED => {
                return Ok(match mutation {
                    OwnerCommandMutation::TargetedControl(_) => {
                        OwnerCommandReply::TargetedControl(Err(TargetedControlError::NotAdmitted))
                    }
                    OwnerCommandMutation::QueuedInput(_) => {
                        OwnerCommandReply::QueuedInput(Err(QueuedCancellationError::NotAdmitted))
                    }
                    OwnerCommandMutation::QueuedCancellation(_) => {
                        OwnerCommandReply::QueuedCancellation(Err(
                            QueuedCancellationError::NotAdmitted,
                        ))
                    }
                    _ => panic!("unexpected refusal fixture operation"),
                })
            }
            UNSUPPORTED => {
                return Ok(OwnerCommandReply::TargetedControl(Err(
                    TargetedControlError::CapabilityUnsupported,
                )))
            }
            PASS | LOST_REPLY => {}
            _ => panic!("unknown routing fixture mode"),
        }
        let result = self
            .authority
            .run(async {
                assert_eq!(execution_owner().as_ref(), Some(&self.authority.token));
                self.owner
                    .execute_owner_mutation(caller, credential, source, mutation)
                    .await
            })
            .await
            .map_err(|_| CommandAdmissionError::from(CommandLedgerError::Unavailable))?;
        if self.mode.load(Ordering::SeqCst) == LOST_REPLY {
            result?;
            return Err(CommandLedgerError::Unavailable.into());
        }
        result
    }
}

pub(super) struct Routed {
    pub(super) original: Fixture,
    pub(super) router: Router,
    pub(super) commands: Arc<AgentCommandService>,
    pub(super) route: Arc<Route>,
}

impl Routed {
    pub(super) fn store(&self) -> &Arc<InMemorySandboxStore> {
        &self.original.store
    }
}

pub(super) async fn fixture(bound: bool) -> Routed {
    let original = super::super::fixtures::fixture(bound).await;
    let owner = AgentCommandService::new(
        original.store.clone(),
        original.access.clone(),
        Arc::new(Custody),
    );
    let ownership = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::register(
        ownership.client(),
        Member {
            instance_id: "control-owner".into(),
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
    let authority = coordinator
        .acquire(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    assert!(authority.token.epoch > 0);
    let route = Arc::new(Route {
        calls: Mutex::new(Vec::new()),
        mode: AtomicU8::new(PASS),
        authority,
        owner: owner.clone(),
        _coordinator: coordinator,
    });
    let commands = owner.with_owner_routing(route.clone());
    let reads = AgentReadService::new(original.store.clone(), Arc::new(NoResources))
        .with_resource_access(original.access.clone());
    let router = super::super::super::router(super::super::super::ResponsesV2State::new(
        crate::agent_page::AgentPageOrigin::test(),
        commands.clone(),
        reads,
    ));
    Routed {
        original,
        router,
        commands,
        route,
    }
}
