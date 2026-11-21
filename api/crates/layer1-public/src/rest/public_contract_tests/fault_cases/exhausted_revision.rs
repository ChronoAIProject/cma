use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{Authorization, Capability, SandboxId, SubjectId};

use super::super::super::{
    agents_tests as agents, hierarchy_router, instances_tests as instances, HierarchyState,
};
use super::super::Scenario;

struct ExhaustedRevision(InMemorySandboxStore);

#[async_trait]
impl SandboxStore for ExhaustedRevision {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.0.create_sandbox(sandbox).await
    }

    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        let mut sandbox = agents::sandbox("sbx_route", "owner-a");
        sandbox.revision = u64::MAX;
        Ok((sandbox.id == *id).then_some(sandbox))
    }

    async fn save_sandbox(&self, revision: u64, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.0.save_sandbox(revision, sandbox).await
    }

    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.0.list_sandboxes(owner, page).await
    }
}

macro_rules! delegate_port {
    ($port:ident { $(fn $method:ident($($arg:ident: $kind:ty),*) -> $result:ty;)* }) => {
        #[async_trait]
        impl $port for ExhaustedRevision {
            $(async fn $method(&self, $($arg: $kind),*) -> Result<$result, SandboxStoreError> {
                self.0.$method($($arg),*).await
            })*
        }
    };
}

delegate_port!(SandboxEventStore {
    fn append_sandbox_event(id: &SandboxId, at: String, kind: SandboxEventKind) -> SandboxEvent;
    fn sandbox_events(id: &SandboxId, after: u64, limit: u16) -> Vec<SandboxEvent>;
});
delegate_port!(SandboxAdmissionStore {
    fn sandbox_admission_replay(owner: &SubjectId, origin: &layer2_core::sandbox::SandboxOrigin) -> Option<SandboxAdmissionReplay>;
    fn admit_sandbox(admission: SandboxAdmission) -> SandboxAdmissionResult;
});
delegate_port!(SandboxTransactionStore {
    fn commit(transaction: SandboxTransaction) -> ();
});

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    if name != "instances_revision_exhausted" {
        return None;
    }
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    Some(Scenario {
        router: hierarchy_router(HierarchyState::new(
            SandboxService::new(
                Arc::new(ExhaustedRevision(InMemorySandboxStore::default())),
                profiles.clone(),
                SandboxPolicy::default(),
                Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
            ),
            WorkspaceService::new(store.clone(), profiles),
            HierarchyReadService::new(store),
            Arc::new(instances::MemoryOperationStore::default()),
        )),
        subject: Some("owner-a"),
        authorization: Authorization::new([Capability::DeleteOwnSandbox]),
        symbols: Default::default(),
    })
}
