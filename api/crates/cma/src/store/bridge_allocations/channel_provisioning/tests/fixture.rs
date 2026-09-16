use super::super::*;
use crate::{
    auth::keys::bridge::test_fixture,
    bridge::{custody::BridgeCredentialStore, BridgeControlPlaneLease},
    sandbox::channel_provisioning::{
        state::{ChannelProvisioningSettings, ChannelProvisioningSnapshot},
        ChannelControlPlaneVerifier, ChannelProvisioningDependencies, ChannelProvisioningDriver,
        ChannelProvisioningOutcome,
    },
    store::{MongoSandboxCredentialStore, MongoSandboxStore},
};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    hosted::InMemoryHostedBackend,
    SandboxAdapter,
};
#[path = "fixture_startup.rs"]
mod startup;

pub(crate) struct VerifiedHuman;
#[async_trait::async_trait]
impl ChannelControlPlaneVerifier for VerifiedHuman {
    async fn revalidate_control_plane(
        &self,
        lease: &BridgeControlPlaneLease,
    ) -> Result<(), BridgeError> {
        if tokio::time::Instant::now() >= lease.deadline() {
            Err(BridgeError::AuthorityUnavailable)
        } else {
            Ok(())
        }
    }
}

pub(crate) struct Fixture {
    pub client: Client,
    pub database: String,
    pub bridges: Arc<MongoBridgeStore>,
    pub svc: BridgeService,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub resources: Arc<MongoSandboxStore>,
    pub custody: Arc<MongoSandboxCredentialStore>,
    pub authority: AllocationAuthority,
    pub lease: Arc<BridgeControlPlaneLease>,
    pub record: AllocationRecord,
    pub server: test_fixture::Server,
    pub backend: InMemoryHostedBackend,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub driver: ChannelProvisioningDriver,
    pub settings: ChannelProvisioningSettings,
    pub tools: Arc<crate::store::MongoChannelToolStore>,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::build(false).await
    }
    pub async fn with_callback_route() -> Self {
        Self::build(true).await
    }
    async fn build(callback_route: bool) -> Self {
        let (client, database, bridges, svc, id, allocations) =
            setup_with_callback_route(callback_route).await;
        svc.mutate(
            &caller(),
            None,
            change(
                &id,
                2,
                "grant-disable",
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            ),
        )
        .await
        .unwrap();
        let bridge = bridges.bridge(&id).await.unwrap().unwrap();
        let management = svc
            .management_lease(&caller(), None, &id, bridge.revision)
            .await
            .unwrap();
        let material = bridges
            .credentials
            .get(&bridge.current().unwrap().config.connector.custody)
            .await
            .unwrap()
            .unwrap();
        let lease = test_fixture::lease(
            bridge.owner.clone(),
            id.clone(),
            bridge.current().unwrap().config.connector.clone(),
            material.metadata.revision,
        );
        let mut granted = material;
        granted.control_plane = Some(lease.grant().clone());
        bridges
            .credentials
            .admit(&management, granted)
            .await
            .unwrap();
        svc.mutate(
            &caller(),
            None,
            change(&id, bridge.revision, "grant-enable", BridgeChange::Enable),
        )
        .await
        .unwrap();
        let (admission, authority, _) = proposal(&svc, &bridges, &id, "12345", None).await;
        let record = allocations
            .reserve(AllocationReservation::new(admission, authority.clone(), now()).unwrap())
            .await
            .unwrap();
        let server = test_fixture::Server::new(&lease, "fixture-deployment").await;
        let custody = Arc::new(
            MongoSandboxCredentialStore::open(
                &client,
                &database,
                "fixture-deployment",
                &custody_key(),
            )
            .await
            .unwrap(),
        );
        let resources = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
        let tools = Arc::new(
            crate::store::MongoChannelToolStore::open(
                resources.clone(),
                "fixture-deployment",
                &custody_key(),
                "https://cma.example",
            )
            .await
            .unwrap(),
        );
        let spec = CodexSandboxSpec::default();
        let settings = ChannelProvisioningSettings {
            deployment_id: "fixture-deployment".into(),
            runtime_version: spec.deployment.image.uri.clone(),
            execution: Default::default(),
            home_policy: None,
        };
        let (adapter, backend) = CodexAdapter::in_memory(spec);
        let adapter: Arc<dyn SandboxAdapter> = Arc::new(adapter.with_channel_tools(tools.clone()));
        let allocations = Arc::new(allocations);
        let driver = ChannelProvisioningDriver::new(
            ChannelProvisioningDependencies {
                allocations: allocations.clone(),
                credentials: Arc::new(VerifiedHuman),
                issuer: server.issuer.clone(),
                custody: custody.clone(),
                adapter: adapter.clone(),
            },
            settings.clone(),
            "channel-worker-a".into(),
        )
        .unwrap();
        Self {
            client,
            database,
            bridges,
            svc,
            allocations,
            resources,
            custody,
            authority,
            lease: Arc::new(lease),
            record,
            server,
            backend,
            adapter,
            driver,
            settings,
            tools,
        }
    }
    pub async fn step(&mut self) -> Result<Option<ChannelProvisioningSnapshot>, AllocationError> {
        match self
            .driver
            .run_one(self.record.clone(), &self.authority, &self.lease)
            .await?
        {
            ChannelProvisioningOutcome::Progressed(record) => {
                self.record = record;
                Ok(None)
            }
            ChannelProvisioningOutcome::Children(snapshot) => {
                self.record = snapshot.allocation.clone();
                Ok(Some(*snapshot))
            }
            ChannelProvisioningOutcome::MintOutcomeUnknown(record) => {
                self.record = record;
                Err(AllocationError::Unavailable)
            }
        }
    }
    pub async fn children(&mut self) -> ChannelProvisioningSnapshot {
        for _ in 0..10 {
            if let Some(snapshot) = self.step().await.unwrap() {
                return snapshot;
            }
        }
        panic!("did not reach child admission");
    }
    pub fn reopen_driver(&mut self) {
        self.adapter = Arc::new(
            CodexAdapter::with_backend(self.backend.clone(), CodexSandboxSpec::default())
                .with_channel_tools(self.tools.clone()),
        );
        self.driver = ChannelProvisioningDriver::new(
            ChannelProvisioningDependencies {
                allocations: self.allocations.clone(),
                credentials: Arc::new(VerifiedHuman),
                issuer: self.server.issuer.clone(),
                custody: self.custody.clone(),
                adapter: self.adapter.clone(),
            },
            self.settings.clone(),
            "channel-worker-b".into(),
        )
        .unwrap();
    }
    pub async fn release_current_claim(&mut self) {
        self.record = self
            .allocations
            .generation(&self.record.identity)
            .await
            .unwrap()
            .unwrap();
        if let Some(claim) = self.record.lease.clone() {
            self.record = self
                .allocations
                .mutate(
                    AllocationMutation::progress(
                        self.record.clone(),
                        self.authority.clone(),
                        &claim,
                        AllocationProgress::NeedsConnection,
                        now(),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }
    }
    pub async fn close(self) {
        self.client.database(&self.database).drop().await.unwrap();
    }
}
