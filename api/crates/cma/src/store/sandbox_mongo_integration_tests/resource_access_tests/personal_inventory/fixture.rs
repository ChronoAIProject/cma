use super::*;
use axum::Router;
use std::time::Duration;

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub app_name: String,
    pub meter: Arc<meter::Meter>,
    pub store: Arc<MongoSandboxStore>,
    pub legacy: Arc<MongoSandboxStore>,
    pub identity: identity::Identity,
    pub limits: Arc<crate::rate_limit::RateLimiters>,
    pub activity: Arc<crate::activity::ActivityRecorder>,
    pub operations: Arc<crate::store::operation::MongoOperationStore>,
}
impl Fixture {
    pub async fn new(rows: usize, distinct: bool) -> Self {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("cma_agent_list_{suffix}");
        let app_name = format!("agent-list-{suffix}");
        let meter = Arc::new(meter::Meter::default());
        let client = meter.client(&app_name).await;
        let store = Arc::new(
            MongoSandboxStore::open(&client, &name)
                .await
                .unwrap()
                .with_resource_list_batch_reads(true),
        );
        seed(&store, rows, distinct, 0).await;
        let legacy = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let identity = identity::Identity::new(&client, &name).await;
        let shared = Arc::new(
            crate::store::request_limits::MongoRequestLimits::open(&client.database(&name))
                .await
                .unwrap(),
        );
        let n = |v| std::num::NonZeroU32::new(v).unwrap();
        let limits = crate::rate_limit::RateLimiters::from_config(
            crate::config::RateLimitConfig {
                admission_per_minute: n(6000),
                health_per_minute: n(120),
                default_per_minute: n(600),
                create_per_minute: n(10),
                input_per_minute: n(120),
                control_per_minute: n(120),
                response_input_tier_enabled: false,
                control_tier_enabled: false,
                default_lease_enabled: false,
            },
            crate::config::TrustedProxyConfig {
                cidrs: vec![],
                cloudflare_cidrs: vec![],
                forwarded_hops: 1,
            },
            shared,
        );
        let activity =
            crate::activity::ActivityRecorder::new(layer2_core::ActivityService::new(Arc::new(
                crate::store::MongoActivityStore::open(&client, &name, Duration::from_secs(86400))
                    .await
                    .unwrap(),
            )));
        let operations = Arc::new(
            crate::store::operation::MongoOperationStore::open(&client, &name)
                .await
                .unwrap(),
        );
        Self {
            client,
            name,
            app_name,
            meter,
            store,
            legacy,
            identity,
            limits,
            activity,
            operations,
        }
    }
    pub async fn readers(count: usize, rows: usize, distinct: bool) -> Self {
        let fixture = Self::new(rows, distinct).await;
        for reader in 1..count {
            seed(&fixture.store, rows, distinct, reader).await;
            fixture.identity.add_reader(reader).await;
        }
        fixture
    }
    pub fn app(&self, batched: bool, joined: bool) -> Router {
        let store = if batched { &self.store } else { &self.legacy };
        self.app_with_access(store.clone(), store.clone(), joined)
    }
    pub async fn cold(&self) {
        self.client
            .database(&self.name)
            .collection::<Document>(crate::store::request_limits::COLLECTION)
            .delete_many(doc! {})
            .await
            .unwrap();
        self.meter.take();
    }
    pub async fn close(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

async fn seed(store: &MongoSandboxStore, rows: usize, distinct: bool, reader: usize) {
    let suffix = if reader == 0 {
        String::new()
    } else {
        format!("_r{reader:03}")
    };
    let mut sandboxes = Vec::new();
    let mut workspaces = Vec::new();
    let mut worktrees = Vec::new();
    let mut agents = Vec::new();
    for index in 0..rows {
        let group = if distinct { index } else { 0 };
        let mut parent = sandbox(&format!("sbx_list_{group:04}{suffix}"));
        parent.name = format!("sandbox-{group}");
        parent.owner = identity::owner(reader);
        let workspace = ready_workspace(
            &parent,
            &format!("wks_list_{group:04}{suffix}"),
            &format!("workspace-{group}"),
        );
        if distinct || index == 0 {
            sandboxes.push(crate::store::sandboxes::sandbox_document(&parent, 0).unwrap());
            workspaces.push(crate::store::workspaces::workspace_document(&workspace).unwrap());
        }
        let (mut tree, mut agent) = worktree_and_agent(&parent.id);
        tree.owner = parent.owner.clone();
        agent.owner = parent.owner.clone();
        tree.id = WorktreeId(format!("wkt_list_{index:04}{suffix}"));
        tree.workspace_id = workspace.id.clone();
        tree.name = format!("tree-{index}");
        tree.path = format!("/home/cma/project/.worktrees/tree-{index}");
        agent.id = AgentId(format!("agt_list_{index:04}{suffix}"));
        agent.worktree_id = tree.id.clone();
        agent.workspace_id = workspace.id;
        agent.creation_key = format!("create-{index}");
        tree.agent_id = Some(agent.id.clone());
        worktrees.push(crate::store::worktrees::worktree_document(&tree).unwrap());
        agents.push(crate::store::agents::agent_document(&agent).unwrap());
    }
    if !agents.is_empty() {
        store.sandboxes.insert_many(sandboxes).await.unwrap();
        store.workspaces.insert_many(workspaces).await.unwrap();
        store.worktrees.insert_many(worktrees).await.unwrap();
        store.agents.insert_many(agents).await.unwrap();
    }
}
