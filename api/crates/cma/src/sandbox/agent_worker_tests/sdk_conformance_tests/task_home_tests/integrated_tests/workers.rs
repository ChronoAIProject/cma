use super::*;
use crate::sandbox::credential::ManagedSandboxCredentials;
use crate::sandbox::live_agent_stopper::DurableLiveAgentStopper;
use crate::sandbox::worker::{SandboxWorker, SandboxWorkerConfig, SandboxWorkerDependencies};
use crate::sandbox::workspace_worker::{
    DurableProfileWorkspaceProvisioner, WorkspaceWorker, WorkspaceWorkerConfig,
    WorkspaceWorkerDependencies,
};

pub(super) async fn credentials(
    database: &database::Database,
) -> (
    Arc<ManagedSandboxCredentials>,
    tokio::task::JoinHandle<()>,
    String,
) {
    let (provider, provider_task) = environment::identity_provider().await;
    let credentials = ManagedSandboxCredentials::new(
        Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            provider.clone(),
            DEPLOYMENT_ID,
        )),
        Arc::new(environment::LocalIdentity),
        database.stores.sandbox_credentials.clone(),
    );
    (credentials, provider_task, provider)
}

pub(super) async fn start(
    harness: &Harness<MongoSandboxStore>,
    database: &database::Database,
    sandbox: SandboxId,
    credentials: Arc<ManagedSandboxCredentials>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut workspace_config = WorkspaceWorkerConfig::new("ticket19-workspace", DEPLOYMENT_ID);
    workspace_config.retry_delay = Duration::from_secs(1);
    let workspace_worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
        },
        workspace_config,
    );
    let sandbox_worker = SandboxWorker::new(
        SandboxWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            credentials,
            workspaces: Arc::new(DurableProfileWorkspaceProvisioner::new(
                WorkspaceService::new(
                    harness.store.clone(),
                    database.stores.sandbox_profiles.clone(),
                ),
                workspace_worker.clone(),
            )),
            agents: Arc::new(DurableLiveAgentStopper::new(
                harness.store.clone(),
                harness.registry.clone(),
            )),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        SandboxWorkerConfig::new(
            "ticket19-sandbox",
            DEPLOYMENT_ID,
            desktop::image(),
            SandboxPolicy::default(),
        )
        .with_home_policy(Some(layer3_adapter::HomeVolumePolicy {
            mode: layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2,
            template_name: String::new(),
            template_generation: String::new(),
            ..desktop::home_policy()
        })),
    );
    vec![
        tokio::spawn(async move {
            loop {
                sandbox_worker
                    .run_one(&sandbox, SystemTime::now())
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }),
        tokio::spawn(async move {
            loop {
                workspace_worker.run_due(SystemTime::now()).await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }),
    ]
}
