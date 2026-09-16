use super::super::super::fixture::{Fixture, VerifiedHuman};
use super::*;
use crate::auth::keys::bridge::test_fixture;
use crate::bridge::{custody::BridgeCredentialStore, BridgeControlPlaneLease};
use crate::sandbox::channel_provisioning::{
    state::ChannelProvisioningSettings, ChannelProvisioningDependencies, ChannelProvisioningDriver,
};
use crate::store::{MongoSandboxCredentialStore, MongoSandboxStore};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    hosted::InMemoryHostedBackend,
    SandboxAdapter,
};

pub(super) struct Setup {
    pub client: Client,
    pub database: String,
    pub bridges: Arc<MongoBridgeStore>,
    pub svc: BridgeService,
    pub id: BridgeId,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub resources: Arc<MongoSandboxStore>,
    pub lease: Arc<BridgeControlPlaneLease>,
    custody: Arc<MongoSandboxCredentialStore>,
    tools: Arc<crate::store::MongoChannelToolStore>,
    backend: InMemoryHostedBackend,
    adapter: Arc<dyn SandboxAdapter>,
    settings: ChannelProvisioningSettings,
}

impl Setup {
    /// This stops before any allocation, Sandbox, Workspace or Agent creation.
    pub async fn new() -> Self {
        let (client, database, bridges, svc, id, allocations) =
            setup_with_callback_route(true).await;
        svc.mutate(
            &caller(),
            None,
            change(
                &id,
                2,
                "wildcard-grant-disable",
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
            change(
                &id,
                bridge.revision,
                "wildcard-grant-enable",
                BridgeChange::Enable,
            ),
        )
        .await
        .unwrap();
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
        let adapter = Arc::new(adapter.with_channel_tools(tools.clone()));
        Self {
            client,
            database,
            bridges,
            svc,
            id,
            allocations: Arc::new(allocations),
            resources,
            lease: Arc::new(lease),
            custody,
            tools,
            backend,
            adapter,
            settings,
        }
    }

    pub async fn reserve_request(&self, inbox: &StoredInbox) -> AllocationReservation {
        let bridge = self.bridges.bridge(&self.id).await.unwrap().unwrap();
        assert_eq!(
            bridge.current().unwrap().config.partition,
            BridgePartitionPolicy::ConversationSender
        );
        assert!(matches!(
            bridge.current().unwrap().config.placement,
            BridgePlacementPolicy::NewSandbox { .. }
        ));
        let source = NormalizedPartitionSource::from_stored(inbox, &bridge).unwrap();
        let admission = AllocationAdmission::from_accepted(bridge.clone(), &source).unwrap();
        let (authority, _) = authority_for_admission(&self.svc, bridge, &admission, 3).await;
        AllocationReservation::new(admission, authority, now()).unwrap()
    }

    pub async fn partition(&self, record: AllocationRecord) -> Fixture {
        let bridge = self.bridges.bridge(&self.id).await.unwrap().unwrap();
        let row = self
            .client
            .database(&self.database)
            .collection::<Document>("bridge_callback_inbox")
            .find_one(doc! {"_id":record.first_source.inbox_id()})
            .await
            .unwrap()
            .unwrap();
        let inbox: StoredInbox = crate::store::sandbox_mongo::read_record(&row).unwrap();
        let source = NormalizedPartitionSource::from_stored(&inbox, &bridge).unwrap();
        let admission = AllocationAdmission::from_accepted(bridge.clone(), &source).unwrap();
        assert_eq!(admission.identity(), &record.identity);
        let (authority, _) = authority_for_admission(&self.svc, bridge, &admission, 3).await;
        // Each real provisioning run has its own owned single-allocation issuer
        // fixture. Concurrent allocation is checked before these sequential runs.
        let server = test_fixture::Server::new(&self.lease, "fixture-deployment").await;
        let driver = ChannelProvisioningDriver::new(
            ChannelProvisioningDependencies {
                allocations: self.allocations.clone(),
                credentials: Arc::new(VerifiedHuman),
                issuer: server.issuer.clone(),
                custody: self.custody.clone(),
                adapter: self.adapter.clone(),
            },
            self.settings.clone(),
            format!("wildcard-provision-{}", record.id),
        )
        .unwrap();
        Fixture {
            client: self.client.clone(),
            database: self.database.clone(),
            bridges: self.bridges.clone(),
            svc: self.svc.clone(),
            allocations: self.allocations.clone(),
            resources: self.resources.clone(),
            custody: self.custody.clone(),
            authority,
            lease: self.lease.clone(),
            record,
            server,
            backend: self.backend.clone(),
            adapter: self.adapter.clone(),
            driver,
            settings: self.settings.clone(),
            tools: self.tools.clone(),
        }
    }
}
