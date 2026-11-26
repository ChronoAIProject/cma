//! Method-scoped faults around real in-memory stores for public route contracts.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use layer2_core::trigger_domain::*;
use layer2_core::{
    AgentProfileId, LaunchId, SandboxProfileId, ServiceHandoffId, SubjectId, WorkspaceProfileId,
};

pub struct FaultPolicy {
    current: Mutex<Result<LaunchPolicy, LaunchServiceError>>,
}

impl Default for FaultPolicy {
    fn default() -> Self {
        Self {
            current: Mutex::new(Ok(LaunchPolicy {
                enabled: true,
                orchestration_paused: false,
                visitor_launches_per_day: 5,
            })),
        }
    }
}

impl FaultPolicy {
    pub fn set(&self, value: Result<LaunchPolicy, LaunchServiceError>) {
        *self.current.lock().unwrap() = value;
    }
}

#[async_trait]
impl LaunchPolicySource for FaultPolicy {
    async fn current_launch_policy(&self) -> Result<LaunchPolicy, LaunchServiceError> {
        self.current.lock().unwrap().clone()
    }
}

pub struct FaultTriggerStore {
    inner: Arc<InMemoryTriggerStore>,
    failure: Mutex<Option<(&'static str, TriggerStoreError)>>,
}

impl FaultTriggerStore {
    pub fn new(inner: Arc<InMemoryTriggerStore>) -> Self {
        Self {
            inner,
            failure: Mutex::new(None),
        }
    }

    pub fn fail(&self, method: &'static str, error: TriggerStoreError) {
        *self.failure.lock().unwrap() = Some((method, error));
    }

    fn check(&self, method: &str) -> Result<(), TriggerStoreError> {
        match self.failure.lock().unwrap().as_ref() {
            Some((target, error)) if *target == "*" || *target == method => Err(error.clone()),
            _ => Ok(()),
        }
    }
}

pub struct FaultLaunchStore {
    calls: AtomicUsize,
    inner: Arc<InMemoryLaunchStore>,
    failure: Mutex<Option<(&'static str, LaunchStoreError)>>,
}

impl FaultLaunchStore {
    pub fn new(inner: Arc<InMemoryLaunchStore>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            inner,
            failure: Mutex::new(None),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn fail(&self, method: &'static str, error: LaunchStoreError) {
        *self.failure.lock().unwrap() = Some((method, error));
    }

    fn check(&self, method: &str) -> Result<(), LaunchStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.failure.lock().unwrap().as_ref() {
            Some((target, error)) if *target == "*" || *target == method => Err(error.clone()),
            _ => Ok(()),
        }
    }
}

macro_rules! forward {
    ($wrapper:ident, $contract:ident, $error:ident, {
        $(fn $method:ident($($arg:ident: $ty:ty),* $(,)?) -> $out:ty;)*
    }) => {
        #[async_trait]
        impl $contract for $wrapper {
            $(async fn $method(&self, $($arg: $ty),*) -> Result<$out, $error> {
                self.check(stringify!($method))?;
                self.inner.$method($($arg),*).await
            })*
        }
    };
}

forward!(FaultTriggerStore, TriggerStore, TriggerStoreError, {
    fn create_trigger(trigger: Trigger) -> ();
    fn trigger(id: &TriggerId, owner: &SubjectId) -> Option<Trigger>;
    fn delete_trigger(id: &TriggerId, owner: &SubjectId, archived_at: String) -> ();
    fn trigger_revision(id: &TriggerId, digest: &str) -> Option<TriggerRevision>;
    fn save_draft(
        id: &TriggerId,
        owner: &SubjectId,
        expected_draft_version: u64,
        draft: TriggerContent,
        updated_at: String,
    ) -> Trigger;
    fn publish_trigger(
        id: &TriggerId,
        owner: &SubjectId,
        expected_draft_version: u64,
        published_at: String,
    ) -> PublicTrigger;
    fn trigger_by_public_id(id: &PublicTriggerId) -> Option<PublicTrigger>;
    fn preview_by_public_id(
        id: &PublicTriggerId,
        publisher_display_name: String,
    ) -> Option<TriggerPreview>;
    fn list_own_triggers(owner: &SubjectId, page: &TriggerPageRequest) -> TriggerPage<Trigger>;
    fn retire_trigger(id: &TriggerId, owner: &SubjectId, retired_at: String) -> Trigger;
    fn create_placement(
        trigger_id: &TriggerId,
        owner: &SubjectId,
        source_url: String,
        label: String,
    ) -> Placement;
    fn placement(
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
    ) -> Option<Placement>;
    fn list_placements(
        trigger_id: &TriggerId,
        owner: &SubjectId,
        include_retired: bool,
    ) -> Vec<Placement>;
    fn update_placement(
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
        update: PlacementUpdate,
    ) -> Placement;
    fn retire_placement(
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
        retired_at: String,
    ) -> Placement;
    fn record_launch(id: &TriggerId) -> TriggerStats;
    fn record_agent_ready(id: &TriggerId) -> TriggerStats;
    fn guard_sandbox_profile_retirement(id: &SandboxProfileId) -> ();
    fn guard_workspace_profile_retirement(id: &WorkspaceProfileId) -> ();
    fn guard_agent_profile_retirement(id: &AgentProfileId) -> ();
});

forward!(FaultLaunchStore, LaunchStore, LaunchStoreError, {
    fn visitor_launches_admitted(owner: &SubjectId, utc_day: u64) -> u32;
    fn admit_launch(admission: LaunchAdmission) -> LaunchAdmissionResult;
    fn launch_by_command(
        owner: &SubjectId,
        public_id: &PublicTriggerId,
        command_key_hash: &str,
    ) -> Option<Launch>;
    fn launch(id: &LaunchId, owner: &SubjectId) -> Option<Launch>;
    fn list_launches(owner: &SubjectId, page: &LaunchPageRequest) -> LaunchPage;
    fn placement_stats(trigger_id: &TriggerId) -> Vec<LaunchPlacementStats>;
    fn commit_launch(commit: LaunchCommit) -> Launch;
    fn launch_events(id: &LaunchId, owner: &SubjectId, after: u64, limit: u16) -> Vec<LaunchEvent>;
    fn due_launches(now: &str, limit: u16) -> Vec<LaunchId>;
    fn claim_launch(
        id: &LaunchId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Option<Launch>;
});

forward!(
    FaultLaunchStore,
    ServiceConnectionStore,
    LaunchStoreError,
    {
        fn handoff_by_intent(
            owner: &SubjectId,
            launch_id: &LaunchId,
            intent_id: &str,
        ) -> Option<ServiceConnectionHandoff>;
        fn create_handoff(handoff: ServiceConnectionHandoff) -> ServiceConnectionHandoff;
        fn handoff(id: &ServiceHandoffId, owner: &SubjectId) -> Option<ServiceConnectionHandoff>;
        fn save_handoff(
            expected_revision: u64,
            handoff: ServiceConnectionHandoff,
        ) -> ServiceConnectionHandoff;
    }
);
