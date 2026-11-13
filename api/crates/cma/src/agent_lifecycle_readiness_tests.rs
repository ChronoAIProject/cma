use super::*;
use crate::agent_lifecycle::workspace::tests::fixture;
use layer2_core::profiles::store::InMemoryProfileStore;
use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxStore, WorkspaceStore};
use layer2_core::sandbox::{FixedSandboxCredentialPrerequisite, SandboxPolicy};

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId::new(subject),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

struct Fixture {
    store: Arc<InMemorySandboxStore>,
    sandboxes: Arc<SandboxService>,
    workspaces: Arc<WorkspaceService>,
    caller: Caller,
    workspace: Workspace,
}

impl Fixture {
    async fn new() -> Self {
        let store = Arc::new(InMemorySandboxStore::default());
        let workspace = fixture::seed(&store).await;
        let profiles = Arc::new(InMemoryProfileStore::default());
        Self {
            sandboxes: SandboxService::new(
                store.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                Arc::new(FixedSandboxCredentialPrerequisite::Ready),
            ),
            workspaces: WorkspaceService::new(store.clone(), profiles),
            caller: caller("owner"),
            store,
            workspace,
        }
    }

    async fn observe(&self, exact: bool) -> Result<Observation, AgentLifecycleProvisioningError> {
        observe(
            &self.sandboxes,
            &self.workspaces,
            &self.caller,
            &self.workspace.sandbox_id,
            exact.then_some(&self.workspace.id),
        )
        .await
    }

    async fn ready(
        &self,
        exact: bool,
    ) -> Result<Option<WorkspaceId>, AgentLifecycleProvisioningError> {
        self.observe(exact)
            .await?
            .ready_workspace(&self.workspaces, &self.caller)
            .await
    }

    async fn sandbox_status(&self, status: SandboxStatus) {
        let mut sandbox = self
            .store
            .get_sandbox(&self.workspace.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let revision = sandbox.revision;
        sandbox.status = status;
        sandbox.revision += 1;
        self.store.save_sandbox(revision, sandbox).await.unwrap();
    }

    async fn workspace_status(&self, status: WorkspaceStatus) {
        let mut workspace = self
            .store
            .get_workspace(&self.workspace.id)
            .await
            .unwrap()
            .unwrap();
        let revision = workspace.revision;
        workspace.status = status;
        workspace.revision += 1;
        self.store
            .save_workspace(revision, workspace)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn exact_and_discovered_workspaces_require_both_ready_gates() {
    for exact in [true, false] {
        let f = Fixture::new().await;
        assert_eq!(f.ready(exact).await.unwrap(), Some(f.workspace.id.clone()));
        f.sandbox_status(SandboxStatus::Installing).await;
        assert_eq!(f.ready(exact).await.unwrap(), None);
        f.workspace_status(WorkspaceStatus::Preparing).await;
        f.sandbox_status(SandboxStatus::Active).await;
        assert_eq!(f.ready(exact).await.unwrap(), None);
        f.workspace_status(WorkspaceStatus::Ready).await;
        assert_eq!(f.ready(exact).await.unwrap(), Some(f.workspace.id));
    }
}

#[tokio::test]
async fn recorded_workspace_beyond_first_page_is_ready_without_sibling_discovery() {
    let f = Fixture::new().await;
    let limit = layer2_core::sandbox::store::PageRequest::default().limit;
    for index in 0..limit {
        let mut sibling = f.workspace.clone();
        sibling.id = WorkspaceId(format!("wk_{index:04}"));
        sibling.name = format!("sibling-{index}");
        sibling.path = format!("/home/cma/{}", sibling.name);
        sibling.status = WorkspaceStatus::Preparing;
        f.store.create_workspace(sibling).await.unwrap();
    }
    let first_page = f
        .workspaces
        .list(&f.caller, &f.workspace.sandbox_id, &Default::default())
        .await
        .unwrap();
    assert!(first_page.next.is_some());
    assert!(!first_page.items.iter().any(|w| w.id == f.workspace.id));
    assert_eq!(f.ready(true).await.unwrap(), Some(f.workspace.id));
}

#[tokio::test]
async fn another_ready_sibling_cannot_complete_the_recorded_workspace() {
    let f = Fixture::new().await;
    let mut sibling = f.workspace.clone();
    sibling.id = WorkspaceId("wk_sibling".into());
    sibling.name = "sibling".into();
    sibling.path = "/home/cma/sibling".into();
    f.store.create_workspace(sibling).await.unwrap();
    f.workspace_status(WorkspaceStatus::Preparing).await;
    assert_eq!(f.ready(true).await.unwrap(), None);
    assert_eq!(
        f.ready(false).await.unwrap(),
        Some(WorkspaceId("wk_sibling".into()))
    );
}

#[tokio::test]
async fn recorded_workspace_cannot_belong_to_another_owned_sandbox() {
    let f = Fixture::new().await;
    let mut other = f
        .store
        .get_sandbox(&f.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    other.id = SandboxId("sbx_other".into());
    other.name = "Other".into();
    f.store.create_sandbox(other.clone()).await.unwrap();
    let result = observe(
        &f.sandboxes,
        &f.workspaces,
        &f.caller,
        &other.id,
        Some(&f.workspace.id),
    )
    .await;
    assert!(
        matches!(result, Err(AgentLifecycleProvisioningError::Conflict(detail))
        if detail == "the recorded workspace belongs to another Sandbox")
    );
}

#[tokio::test]
async fn reads_enforce_capability_and_each_resource_owner() {
    let mut f = Fixture::new().await;
    f.caller = Caller::new(SubjectId::new("owner"), Authorization::new([]));
    assert!(matches!(
        f.observe(true).await,
        Err(AgentLifecycleProvisioningError::Forbidden)
    ));
    f.caller = caller("stranger");
    assert!(matches!(
        f.observe(true).await,
        Err(AgentLifecycleProvisioningError::Forbidden)
    ));
    f.caller = caller("owner");
    let mut sandbox = f
        .store
        .get_sandbox(&f.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    sandbox.owner = SubjectId::new("stranger");
    sandbox.id = SandboxId("sbx_foreign".into());
    f.workspace.sandbox_id = sandbox.id.clone();
    f.store.create_sandbox(sandbox).await.unwrap();
    assert!(matches!(
        f.observe(true).await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
}

#[tokio::test]
async fn missing_workspace_or_parent_is_not_ready() {
    let mut f = Fixture::new().await;
    let original_id = f.workspace.id.clone();
    f.workspace.id = WorkspaceId("wk_missing".into());
    assert!(matches!(
        f.observe(true).await,
        Err(AgentLifecycleProvisioningError::Forbidden)
    ));
    f.workspace.id = original_id;
    f.workspace.sandbox_id = SandboxId("sbx_missing".into());
    assert!(matches!(
        f.observe(true).await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
}

#[tokio::test]
async fn failed_or_retiring_parent_never_completes_creation() {
    for status in [
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ] {
        let f = Fixture::new().await;
        f.sandbox_status(status).await;
        for exact in [true, false] {
            assert!(
                matches!(f.observe(exact).await, Err(AgentLifecycleProvisioningError::Conflict(detail))
                if detail == "Sandbox provisioning failed")
            );
        }
    }
}

#[tokio::test]
async fn workspace_failure_takes_precedence_over_parent_failure() {
    let f = Fixture::new().await;
    f.workspace_status(WorkspaceStatus::Failed).await;
    f.sandbox_status(SandboxStatus::Failed).await;
    assert!(
        matches!(f.observe(true).await, Err(AgentLifecycleProvisioningError::Conflict(detail))
        if detail == "Workspace preparation failed. The task has not run.")
    );
}

#[tokio::test]
async fn repository_failure_preserves_the_actionable_diagnostic() {
    let f = Fixture::new().await;
    let mut workspace = f.workspace.clone();
    workspace.status = WorkspaceStatus::Failed;
    workspace.failed = Some(layer2_core::sandbox::TypedFailure {
        class: "repository_unavailable".into(),
        detail: "Repository access must be restored".into(),
    });
    workspace.revision += 1;
    f.store.save_workspace(1, workspace).await.unwrap();
    assert!(
        matches!(f.observe(true).await, Err(AgentLifecycleProvisioningError::Conflict(detail))
        if detail == "repository_unavailable: Repository access must be restored")
    );
}
