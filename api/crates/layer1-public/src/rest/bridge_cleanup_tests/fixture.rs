//! Real HTTP/core authority with inert retained allocations. This fixture has
//! no runtime adapter, custody lease, cleanup worker or physical-removal proof.
use super::super::{agents_tests, bridges_tests::fixture::Ports, resource_access_tests};
use layer2_core::{
    bridge::{allocation::*, cleanup::*, *},
    resource_access::*,
    sandbox::store::{InMemorySandboxStore, SandboxStore, WorkspaceStore},
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

mod retained;
mod workspace;

pub(in crate::rest) fn context(subject: &str) -> CallerContext {
    let mut context = resource_access_tests::context(subject);
    context.actor = ResourceActor::Human {
        subject: context.caller.subject.clone(),
    };
    context
}

pub(in crate::rest) struct Memory {
    pub records: Vec<AllocationRecord>,
    pub failure: AtomicUsize,
    state: Mutex<Option<(String, ChildCleanupStatus)>>,
    workspace_state: Mutex<Option<(String, WorkspaceCleanupStatus)>>,
    pub runtime_supported: AtomicBool,
    pub child_complete: AtomicBool,
}
impl Memory {
    fn available(&self) -> Result<(), CleanupError> {
        match self.failure.load(Ordering::SeqCst) {
            0 => Ok(()),
            1 => Err(CleanupError::Unavailable),
            2 => Err(CleanupError::Corrupt),
            3 => Err(CleanupError::Conflict),
            _ => Err(CleanupError::OriginalActorRequired),
        }
    }
}
#[async_trait::async_trait]
impl BridgeCleanupStore for Memory {
    async fn allocations(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AllocationRecord>, CleanupError> {
        manager.require_current()?;
        self.available()?;
        Ok(self
            .records
            .iter()
            .filter(|record| {
                record.identity.bridge_id == manager.record().id
                    && after.is_none_or(|after| after < record.id.as_str())
            })
            .take(limit as usize + 1)
            .cloned()
            .collect())
    }
    async fn allocation(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<AllocationRecord>, CleanupError> {
        manager.require_current()?;
        self.available()?;
        Ok(self
            .records
            .iter()
            .find(|record| record.id == id && record.identity.bridge_id == manager.record().id)
            .cloned())
    }
    async fn status(&self, access: &CleanupAccess) -> Result<ChildCleanupStatus, CleanupError> {
        access.require_current()?;
        self.available()?;
        Ok(self
            .state
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, status)| status.clone())
            .unwrap_or_else(|| retained::status(access.allocation())))
    }
    async fn recover(
        &self,
        access: &CleanupAccess,
        id: &str,
        digest: &str,
    ) -> Result<Option<ChildCleanupStatus>, CleanupError> {
        access.require_current()?;
        self.available()?;
        let state = self.state.lock().unwrap();
        let Some((prior_digest, status)) = &*state else {
            return Ok(None);
        };
        let receipt = status.receipt.as_ref().ok_or(CleanupError::Corrupt)?;
        if receipt.id != id {
            return Ok(None);
        }
        if prior_digest != digest || receipt.requested_by != *access.management().actor() {
            return Err(CleanupError::Conflict);
        }
        Ok(Some(status.clone()))
    }
    async fn workspace_status(
        &self,
        access: &CleanupAccess,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        self.read_workspace(access)
    }
    async fn workspace_recover(
        &self,
        access: &CleanupAccess,
        id: &str,
        digest: &str,
    ) -> Result<Option<WorkspaceCleanupStatus>, CleanupError> {
        self.recover_workspace(access, id, digest)
    }
    async fn workspace_submit(
        &self,
        write: WorkspaceCleanupWrite,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        self.submit_workspace(write)
    }
    async fn submit(&self, write: CleanupWrite) -> Result<ChildCleanupStatus, CleanupError> {
        write.require_current()?;
        self.available()?;
        let mut state = self.state.lock().unwrap();
        if state.is_some() {
            return Err(CleanupError::Conflict);
        }
        let mut status = retained::status(write.access().allocation());
        status.receipt = Some(ChildCleanupReceipt {
            id: write.receipt_id().into(),
            bridge_id: status.bridge_id.clone(),
            allocation_id: status.allocation_id.clone(),
            generation: status.generation,
            target: status.target.clone(),
            request: write.request().clone(),
            requested_by: write.access().management().actor().clone(),
            accepted_at_ms: 1_000,
        });
        status.progress = ChildCleanupProgress::Requested;
        status.holds = vec![ChildCleanupHold::PendingCommand];
        status.observed_at_ms = Some(1_000);
        *state = Some((write.request_sha256().into(), status.clone()));
        Ok(status)
    }
}

pub(in crate::rest) struct Fixture {
    pub router: axum::Router,
    pub definitions: BridgeService,
    pub memory: Arc<Memory>,
    pub bridge: BridgeId,
    pub allocation: String,
}
pub(in crate::rest) async fn fixture() -> Fixture {
    fixture_for(false).await
}
pub(in crate::rest) async fn owned_fixture() -> Fixture {
    fixture_for(true).await
}
async fn fixture_for(owned: bool) -> Fixture {
    let resources = Arc::new(InMemorySandboxStore::default());
    let mut sandbox = agents_tests::sandbox("sbx_cleanup_parent", "owner-a");
    let mut workspace =
        agents_tests::workspace("wks_cleanup_parent", sandbox.id.as_str(), "owner-a");
    if owned {
        let bridge =
            BridgeService::creation_id(&context("owner-a"), "cleanup-http-fixture").unwrap();
        let identity = AllocationIdentity {
            bridge_id: bridge,
            key: serde_json::from_value(serde_json::json!({"version":1,"hash":"1".repeat(64)}))
                .unwrap(),
            generation: 1,
        };
        let id = identity.id().unwrap();
        let suffix =
            layer2_core::profiles::content_digest(&("cma.bridge.resource.v1", &id, "workspace"))
                .unwrap();
        workspace.id = layer2_core::WorkspaceId(format!("wks_{}", &suffix[..32]));
        workspace.profile.content.agent_profiles.clear();
    }
    sandbox.profile.digest =
        layer2_core::profiles::content_digest(&sandbox.profile.content).unwrap();
    workspace.profile.digest =
        layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
    resources.create_sandbox(sandbox.clone()).await.unwrap();
    resources.create_workspace(workspace.clone()).await.unwrap();
    let ports = Arc::new(Ports {
        role: AtomicUsize::new(2),
        calls: AtomicUsize::new(0),
        validation: AtomicUsize::new(1),
    });
    let definitions = BridgeService::new(
        Arc::new(MemoryBridgeStore::default()),
        ports.clone(),
        ports.clone(),
    );
    let caller = context("owner-a");
    let bridge = BridgeService::creation_id(&caller, "cleanup-http-fixture").unwrap();
    let mut config = retained::config(&sandbox, &workspace);
    if owned {
        let mut value = serde_json::to_value(&config).unwrap();
        value["placement"]["kind"] = serde_json::json!("workspace_per_partition");
        value["placement"]
            .as_object_mut()
            .unwrap()
            .remove("workspace_id");
        value["placement"]["workspace_profile"] =
            serde_json::json!({"id":workspace.profile.id,"revision":workspace.profile.revision});
        config = serde_json::from_value(value).unwrap();
    }
    definitions
        .mutate(
            &caller,
            Some("request-credential"),
            BridgeMutation {
                id: bridge.clone(),
                expected_revision: 0,
                idempotency_key: "cleanup-http-fixture".into(),
                change: BridgeChange::Create {
                    owner: ResourceOwner::Person {
                        subject: caller.caller.subject.clone(),
                    },
                    config: Box::new(config.clone()),
                },
            },
        )
        .await
        .unwrap();
    let record = retained::allocation(bridge.clone(), config, &sandbox, &workspace);
    let allocation = record.id.clone();
    let memory = Arc::new(Memory {
        records: vec![record],
        failure: AtomicUsize::new(0),
        state: Mutex::new(None),
        workspace_state: Mutex::new(None),
        runtime_supported: AtomicBool::new(true),
        child_complete: AtomicBool::new(true),
    });
    let service = BridgeCleanupService::new(
        definitions.clone(),
        Arc::new(ResourceAccessService::new(resources, ports)),
        memory.clone(),
    );
    Fixture {
        router: super::super::bridge_cleanup_router(Some(Arc::new(service))),
        definitions,
        memory,
        bridge,
        allocation,
    }
}

pub(in crate::rest) fn body() -> serde_json::Value {
    serde_json::json!({"expected_bridge_revision":1,"expected_allocation_revision":4,"action":"remove_child"})
}

pub(in crate::rest) fn workspace_body() -> serde_json::Value {
    serde_json::json!({"expected_bridge_revision":1,"expected_allocation_revision":4,
    "expected_workspace_revision":3,"child_cleanup_receipt_id":workspace::child_receipt_id(),
    "action":"remove_owned_workspace"})
}
