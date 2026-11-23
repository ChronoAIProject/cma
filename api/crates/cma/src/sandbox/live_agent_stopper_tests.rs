use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use layer2_core::sandbox::store::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentStopRequest, Page, PageRequest, SandboxCheckpointMutation,
    SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxStoreError,
    MAX_SANDBOX_LIFECYCLE_AGENTS,
};
use layer2_core::sandbox::{Agent, AgentStatus, AgentStopReason, AgentWorkStep};
use layer2_core::{AgentId, SandboxId, SubjectId};

use super::super::live_agents::LiveAgentRegistryError;
use super::*;

#[path = "live_agent_stopper_fixture.rs"]
mod fixture;
use fixture::{attach, live_agent, live_target, sandbox, terminal_agent};

const NOW: &str = "2026-09-06T00:00:00Z";

struct ScriptedStore {
    pages: Mutex<VecDeque<Result<Page<Agent>, SandboxStoreError>>>,
    page_requests: Mutex<Vec<(SandboxId, PageRequest)>>,
    agents: Mutex<BTreeMap<AgentId, Agent>>,
    stop_requests: Mutex<Vec<AgentStopRequest>>,
    control_failures: Mutex<VecDeque<AgentControlStoreError>>,
}

impl ScriptedStore {
    fn new(pages: Vec<Result<Page<Agent>, SandboxStoreError>>, agents: Vec<Agent>) -> Arc<Self> {
        Arc::new(Self {
            pages: Mutex::new(pages.into()),
            page_requests: Mutex::new(Vec::new()),
            agents: Mutex::new(
                agents
                    .into_iter()
                    .map(|agent| (agent.id.clone(), agent))
                    .collect(),
            ),
            stop_requests: Mutex::new(Vec::new()),
            control_failures: Mutex::new(VecDeque::new()),
        })
    }

    fn fail_control_with(&self, error: AgentControlStoreError) {
        self.control_failures.lock().unwrap().push_back(error);
    }

    fn page_requests(&self) -> Vec<(SandboxId, PageRequest)> {
        self.page_requests.lock().unwrap().clone()
    }

    fn stop_requests(&self) -> Vec<AgentStopRequest> {
        self.stop_requests.lock().unwrap().clone()
    }

    fn agent(&self, id: &AgentId) -> Agent {
        self.agents.lock().unwrap().get(id).unwrap().clone()
    }
}

#[async_trait::async_trait]
impl SandboxLifecycleExecutionStore for ScriptedStore {
    async fn sandbox_agents(
        &self,
        sandbox_id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        self.page_requests
            .lock()
            .unwrap()
            .push((sandbox_id.clone(), page.clone()));
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .expect("sandbox-agent page is scripted")
    }

    async fn publish_sandbox_checkpoint(
        &self,
        _mutation: SandboxCheckpointMutation,
    ) -> Result<(), SandboxStoreError> {
        unreachable!("the stopper never publishes checkpoints")
    }

    async fn commit_sandbox_lifecycle(
        &self,
        _mutation: SandboxLifecycleMutation,
    ) -> Result<(), SandboxStoreError> {
        unreachable!("the stopper never commits lifecycle phases")
    }
}

#[async_trait::async_trait]
impl AgentControlStore for ScriptedStore {
    async fn schedule_agent_stop(
        &self,
        request: AgentStopRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        self.stop_requests.lock().unwrap().push(request.clone());
        if let Some(error) = self.control_failures.lock().unwrap().pop_front() {
            return Err(error);
        }

        let mut agents = self.agents.lock().unwrap();
        let current = agents
            .get(&request.target.agent_id)
            .cloned()
            .ok_or(AgentControlStoreError::NotFound)?;
        if AgentControlTarget::from(&current) != request.target {
            return Err(AgentControlStoreError::Conflict);
        }
        if !current.status.is_live() {
            return Err(AgentControlStoreError::StatusConflict {
                status: current.status,
            });
        }
        if let Some(reason) = current.requested_stop_reason.as_ref() {
            if reason != &request.reason {
                return Err(AgentControlStoreError::StopReasonConflict);
            }
            if !is_stop_step(current.work.step) {
                return Err(AgentControlStoreError::WorkConflict {
                    step: current.work.step,
                });
            }
            return Ok(AgentControlResult::Existing(current));
        }

        let mut scheduled = current;
        scheduled
            .request_stop(request.reason, request.requested_at)
            .map_err(|_| AgentControlStoreError::Conflict)?;
        agents.insert(scheduled.id.clone(), scheduled.clone());
        Ok(AgentControlResult::Scheduled(scheduled))
    }

    async fn schedule_agent_delete(
        &self,
        _request: AgentDeleteRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        unreachable!("the stopper never schedules agent deletion")
    }
}

fn is_stop_step(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
    )
}

fn ok_page(items: Vec<Agent>, next: Option<&str>) -> Result<Page<Agent>, SandboxStoreError> {
    Ok(Page {
        items,
        next: next.map(str::to_owned),
    })
}

fn stopper(store: Arc<ScriptedStore>, registry: Arc<LiveAgentRegistry>) -> DurableLiveAgentStopper {
    DurableLiveAgentStopper::new(store, registry)
}

#[tokio::test]
async fn sandbox_without_agents_is_immediately_complete() {
    let sandbox = sandbox();
    let store = ScriptedStore::new(vec![ok_page(Vec::new(), None)], Vec::new());
    let stopper = stopper(store.clone(), Arc::new(LiveAgentRegistry::default()));

    assert_eq!(
        stopper
            .stop_live_agents(&sandbox, None, "retire:no-agents")
            .await,
        Ok(())
    );
    assert_eq!(
        store.page_requests(),
        [(sandbox.id, PageRequest::new(MAX_PAGE_SIZE, None).unwrap())]
    );
    assert!(store.stop_requests().is_empty());
}

#[tokio::test]
async fn live_agent_is_scheduled_fenced_and_keeps_retirement_pending() {
    let sandbox = sandbox();
    let mut agent = live_agent(&sandbox, 1);
    agent.work.next_fence = 9;
    let target = live_target(&agent);
    let registry = Arc::new(LiveAgentRegistry::default());
    let _updates = attach(&registry, &agent, 8);
    assert!(registry.target_is_attached(&target));
    let store = ScriptedStore::new(
        vec![ok_page(vec![agent.clone()], None)],
        vec![agent.clone()],
    );
    let stopper = stopper(store.clone(), registry.clone());

    assert_eq!(
        stopper
            .stop_live_agents(&sandbox, None, "retire:live")
            .await,
        Err(SandboxDependencyError::AgentsStopping)
    );
    assert_eq!(
        store.stop_requests(),
        [AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: AgentStopReason::SandboxRetired,
            requested_at: sandbox.updated_at,
        }]
    );
    let scheduled = store.agent(&agent.id);
    assert_eq!(
        scheduled.requested_stop_reason,
        Some(AgentStopReason::SandboxRetired)
    );
    assert_eq!(scheduled.work.step, AgentWorkStep::FenceInteraction);
    assert_eq!(scheduled.work.next_fence, 9);
    assert!(!registry.target_is_attached(&target));
    assert_eq!(
        registry.attachment(&target, scheduled.work.next_fence),
        Err(LiveAgentRegistryError::StaleGeneration)
    );
}

#[tokio::test]
async fn in_progress_stops_replay_without_overwriting_a_legitimate_reason() {
    let sandbox = sandbox();
    let mut retirement = live_agent(&sandbox, 1);
    retirement.requested_stop_reason = Some(AgentStopReason::SandboxRetired);
    retirement
        .work
        .schedule(AgentWorkStep::FenceInteraction, NOW.into());
    retirement.work.next_fence = 12;
    let mut owner = live_agent(&sandbox, 2);
    owner.requested_stop_reason = Some(AgentStopReason::Owner);
    owner.work.schedule(AgentWorkStep::CloseProcess, NOW.into());
    owner.work.next_fence = 22;
    let registry = Arc::new(LiveAgentRegistry::default());
    let retirement_target = live_target(&retirement);
    let owner_target = live_target(&owner);
    let _updates = [
        attach(&registry, &retirement, 11),
        attach(&registry, &owner, 21),
    ];
    let store = ScriptedStore::new(
        vec![ok_page(vec![retirement.clone(), owner.clone()], None)],
        vec![retirement.clone(), owner.clone()],
    );
    let stopper = stopper(store.clone(), registry.clone());

    assert_eq!(
        stopper
            .stop_live_agents(&sandbox, None, "retire:replay")
            .await,
        Err(SandboxDependencyError::AgentsStopping)
    );
    let requests = store.stop_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].target.agent_id, retirement.id);
    assert_eq!(requests[0].reason, AgentStopReason::SandboxRetired);
    assert_eq!(
        store.agent(&owner.id).requested_stop_reason,
        Some(AgentStopReason::Owner)
    );
    assert!(!registry.target_is_attached(&retirement_target));
    assert!(!registry.target_is_attached(&owner_target));
}

#[tokio::test]
async fn pagination_reaches_every_page_before_scheduling_stops() {
    let sandbox = sandbox();
    let first = live_agent(&sandbox, 1);
    let second = live_agent(&sandbox, 2);
    let store = ScriptedStore::new(
        vec![
            ok_page(vec![first.clone()], Some("page-two")),
            ok_page(vec![second.clone()], None),
        ],
        vec![first, second],
    );
    let stopper = stopper(store.clone(), Arc::new(LiveAgentRegistry::default()));

    assert_eq!(
        stopper
            .stop_live_agents(&sandbox, None, "retire:paged")
            .await,
        Err(SandboxDependencyError::AgentsStopping)
    );
    assert_eq!(
        store.page_requests(),
        [
            (
                sandbox.id.clone(),
                PageRequest::new(MAX_PAGE_SIZE, None).unwrap(),
            ),
            (
                sandbox.id,
                PageRequest::new(MAX_PAGE_SIZE, Some("page-two".into())).unwrap(),
            ),
        ]
    );
    assert_eq!(store.stop_requests().len(), 2);
}

#[tokio::test]
async fn pagination_rejects_cursor_stalls_and_agent_overflow_before_mutation() {
    let sandbox = sandbox();
    let first = live_agent(&sandbox, 1);
    let second = live_agent(&sandbox, 2);
    let stalled = ScriptedStore::new(
        vec![
            ok_page(vec![first.clone()], Some("stalled")),
            ok_page(vec![second.clone()], Some("stalled")),
        ],
        vec![first, second],
    );
    let stalled_stopper = stopper(stalled.clone(), Arc::new(LiveAgentRegistry::default()));
    assert_eq!(
        stalled_stopper
            .stop_live_agents(&sandbox, None, "retire:stalled")
            .await,
        Err(SandboxDependencyError::Validation(
            AGENT_STATE_INVALID.into()
        ))
    );
    assert!(stalled.stop_requests().is_empty());

    let agents = (0..=MAX_SANDBOX_LIFECYCLE_AGENTS)
        .map(|index| terminal_agent(live_agent(&sandbox, index), AgentStatus::Stopped))
        .collect::<Vec<_>>();
    let overflow = ScriptedStore::new(
        vec![
            ok_page(agents[..usize::from(MAX_PAGE_SIZE)].to_vec(), Some("more")),
            ok_page(agents[usize::from(MAX_PAGE_SIZE)..].to_vec(), None),
        ],
        Vec::new(),
    );
    let overflow_stopper = stopper(overflow.clone(), Arc::new(LiveAgentRegistry::default()));
    assert_eq!(
        overflow_stopper
            .stop_live_agents(&sandbox, None, "retire:overflow")
            .await,
        Err(SandboxDependencyError::Validation(
            AGENT_STATE_INVALID.into()
        ))
    );
    assert!(overflow.stop_requests().is_empty());
}

#[tokio::test]
async fn cross_sandbox_or_owner_rows_are_security_failures() {
    let sandbox = sandbox();
    let mut foreign_sandbox = live_agent(&sandbox, 1);
    foreign_sandbox.sandbox_id = SandboxId::generate();
    let mut foreign_owner = live_agent(&sandbox, 2);
    foreign_owner.owner = SubjectId::new("another-owner");

    for agent in [foreign_sandbox, foreign_owner] {
        let store = ScriptedStore::new(vec![ok_page(vec![agent.clone()], None)], vec![agent]);
        let stopper = stopper(store.clone(), Arc::new(LiveAgentRegistry::default()));
        assert_eq!(
            stopper
                .stop_live_agents(&sandbox, None, "retire:foreign")
                .await,
            Err(SandboxDependencyError::Security(
                AGENT_BOUNDARY_INVALID.into()
            ))
        );
        assert!(store.stop_requests().is_empty());
    }
}

#[tokio::test]
async fn store_failures_and_races_are_sanitized_and_classified() {
    const BACKEND_DETAIL: &str = "storage topology detail";

    let sandbox = sandbox();
    let unavailable = ScriptedStore::new(
        vec![Err(SandboxStoreError::Unavailable(BACKEND_DETAIL.into()))],
        Vec::new(),
    );
    let error = stopper(unavailable, Arc::new(LiveAgentRegistry::default()))
        .stop_live_agents(&sandbox, None, "retire:unavailable")
        .await
        .unwrap_err();
    assert_eq!(
        error,
        SandboxDependencyError::Transient(AGENT_STORE_UNAVAILABLE.into())
    );
    assert!(!error.to_string().contains(BACKEND_DETAIL));

    let agent = live_agent(&sandbox, 1);
    let raced = ScriptedStore::new(
        vec![ok_page(vec![agent.clone()], None)],
        vec![agent.clone()],
    );
    raced.fail_control_with(AgentControlStoreError::Conflict);
    assert_eq!(
        stopper(raced, Arc::new(LiveAgentRegistry::default()))
            .stop_live_agents(&sandbox, None, "retire:raced")
            .await,
        Err(SandboxDependencyError::Transient(
            AGENT_STATE_CHANGED.into()
        ))
    );

    let corrupt = ScriptedStore::new(vec![ok_page(vec![agent.clone()], None)], vec![agent]);
    corrupt.fail_control_with(AgentControlStoreError::Corrupt(BACKEND_DETAIL.into()));
    let error = stopper(corrupt, Arc::new(LiveAgentRegistry::default()))
        .stop_live_agents(&sandbox, None, "retire:corrupt")
        .await
        .unwrap_err();
    assert_eq!(
        error,
        SandboxDependencyError::Validation(AGENT_STATE_INVALID.into())
    );
    assert!(!error.to_string().contains(BACKEND_DETAIL));
}

#[tokio::test]
async fn all_terminal_agents_complete_without_control_mutations() {
    let sandbox = sandbox();
    let agents = [
        AgentStatus::Stopped,
        AgentStatus::Failed,
        AgentStatus::Deleted,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, status)| terminal_agent(live_agent(&sandbox, index), status))
    .collect();
    let store = ScriptedStore::new(vec![ok_page(agents, None)], Vec::new());
    let stopper = stopper(store.clone(), Arc::new(LiveAgentRegistry::default()));

    assert_eq!(
        stopper
            .stop_live_agents(&sandbox, None, "retire:terminal")
            .await,
        Ok(())
    );
    assert!(store.stop_requests().is_empty());
}
