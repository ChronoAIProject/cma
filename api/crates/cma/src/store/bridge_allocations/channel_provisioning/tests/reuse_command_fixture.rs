//! Signed queued commands on children created by the actual reuse driver.
//! Mongo is disposable; runtime IO uses InMemoryHostedBackend, not a process.
use super::{reuse::fixture::ReuseFixture, *};
use crate::{
    bridge::provisioning::reuse_revival::ChannelReuseRevivalAuthority,
    sandbox::{worker::SandboxWorker, LiveAgentRegistry},
    store::{MongoBridgeInvokeStore, MongoChannelCommandStore},
};
use layer2_core::{
    bridge::invoke::*,
    resource_access::ResourceAccessService,
    sandbox::{agent_io::*, store::*, *},
};
use mongodb::bson::Document;
use std::time::SystemTime;

#[path = "reuse_command_fixture/connections.rs"]
pub(super) mod connections;
#[path = "reuse_command_fixture/reply.rs"]
mod reply;
#[path = "reuse_command_fixture/worker.rs"]
mod worker;

pub(super) struct ReuseCommandFixture {
    pub f: ReuseFixture,
    pub record: AllocationRecord,
    pub worker: Arc<SandboxWorker>,
    pub registry: Arc<LiveAgentRegistry>,
}
pub(super) struct Queued {
    pub invokes: Arc<BridgeInvokeService>,
    pub store: Arc<MongoBridgeInvokeStore>,
    pub channel: Arc<ChannelAgentCommandService>,
    pub command: AdmittedAgentCommand,
}
impl ReuseCommandFixture {
    pub async fn new(shared_workspace: bool, trigger_parent: bool) -> Self {
        Self::from_reuse(ReuseFixture::new(shared_workspace).await, trigger_parent).await
    }
    pub(super) async fn from_reuse(mut f: ReuseFixture, trigger_parent: bool) -> Self {
        f.original.svc = BridgeService::new(
            f.original.bridges.clone(),
            Arc::new(Authority),
            Arc::new(connections::Connections(f.original.resources.clone())),
        );
        if trigger_parent {
            // Origin is set only on the disposable seed, before reuse starts.
            f.parent.origin = SandboxOrigin::Trigger {
                launch_id: "fixture-original-trigger-launch".into(),
            };
            f.parent.validate().unwrap();
            let mut seed = f.parent.clone();
            seed.revision = 1;
            seed.status = SandboxStatus::Provisioning;
            seed.runtime = None;
            seed.lifecycle = Default::default();
            seed.work =
                SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, seed.created_at.clone());
            let name = SandboxAdmissionName::Exact(seed.name.clone());
            let seed_at = seed.created_at.clone();
            let admission = SandboxAdmission::new(seed, name, 10, seed_at).unwrap();
            let mut fields =
                crate::store::sandboxes::sandbox_replacement_fields(&f.parent).unwrap();
            fields.insert(
                "admission_name",
                crate::store::sandbox_mongo::record_bson(&admission.name).unwrap(),
            );
            fields.insert("admission_fingerprint", admission.admission_fingerprint);
            f.original
                .client
                .database(&f.original.database)
                .collection::<Document>("sandboxes")
                .update_one(doc! {"_id":f.parent.id.as_str()}, doc! {"$set":fields})
                .await
                .unwrap();
        }
        let (admission, proof) = f.proposal("67890").await;
        let mut record = f
            .original
            .allocations
            .reserve(AllocationReservation::new(admission, proof, now()).unwrap())
            .await
            .unwrap();
        for _ in 0..8 {
            let proof = f.authority(&record).await;
            record = f
                .driver()
                .run_one(record, &proof, &f.original.lease)
                .await
                .unwrap();
            if record.state == AllocationState::Ready {
                break;
            }
            let proof = f.authority(&record).await;
            if let Some(claim) = record.lease.clone() {
                record = f
                    .original
                    .allocations
                    .mutate(
                        AllocationMutation::progress(
                            record,
                            proof,
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
        assert_eq!(record.state, AllocationState::Ready);
        let registry = worker::publish(&f, &record).await;
        let worker = worker::build(&f, registry.clone());
        Self {
            f,
            record,
            worker,
            registry,
        }
    }
    pub async fn sandbox(&self) -> Sandbox {
        self.f
            .original
            .resources
            .get_sandbox(&self.f.parent.id)
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn pause(&self) -> Sandbox {
        self.f
            .original
            .resources
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                self.f.parent.id.clone(),
                self.f.parent.owner.clone(),
                layer2_core::now_rfc3339(),
                None,
            ))
            .await
            .unwrap();
        for _ in 0..8 {
            if self.sandbox().await.status == SandboxStatus::Paused {
                break;
            }
            self.step().await;
        }
        let paused = self.sandbox().await;
        assert_eq!(paused.status, SandboxStatus::Paused);
        paused
    }
    pub async fn step(&self) {
        assert!(self
            .worker
            .run_one(&self.f.parent.id, SystemTime::now())
            .await
            .unwrap());
    }
    pub async fn permit(&self) -> ChannelReuseRevivalAuthority {
        ChannelReuseRevivalAuthority::fixture(
            self.record.clone(),
            self.f.authority(&self.record).await,
            self.f.original.lease.clone(),
            self.sandbox().await,
        )
    }
    pub async fn queued(&self, message_id: &str) -> Queued {
        let f = &self.f.original;
        let authority = self.f.authority(&self.record).await;
        let store = Arc::new(
            MongoBridgeInvokeStore::open(f.bridges.clone(), f.resources.clone(), &f.database)
                .await
                .unwrap(),
        );
        let invokes = Arc::new(BridgeInvokeService::new(
            f.svc.clone(),
            Arc::new(ResourceAccessService::new(
                f.resources.clone(),
                Arc::new(Authority),
            )),
            f.resources.clone(),
            store.clone(),
        ));
        let grant = invokes
            .issue_channel(
                f.lease.clone(),
                &self.record.id,
                &self.record.pin,
                authority,
            )
            .await
            .unwrap();
        let (body, token, _) =
            crate::store::bridge_definitions::mongo_integration_tests::inbox::callback_for_sender(
                message_id,
                "Continue this channel-created child on its original shared parent.",
                67890,
            );
        let write = crate::store::bridge_definitions::mongo_integration_tests::inbox::write_using_validator(
            &f.bridges,
            &self.record.identity.bridge_id,
            &body,
            &token,
            Duration::from_secs(60),
            &crate::bridge::ingress::NyxidCapabilities,
            &connections::Connections(f.resources.clone()),
        )
        .await;
        let inbox = write.record().clone();
        crate::store::MongoCallbackInbox::open(
            &f.bridges,
            &f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap()
        .accept(write)
        .await
        .unwrap();
        let channel = Arc::new(ChannelAgentCommandService::new(Arc::new(
            MongoChannelCommandStore::new(store.clone()),
        )));
        let proof = invokes
            .authorize(BridgeInvokeRequest {
                grant_id: grant.id,
                pin: inbox.pin.clone(),
                target: self.record.placement.target().clone(),
                effect: BridgeInvokeEffect::AdmitMessage {
                    inbox_id: inbox.id.clone(),
                },
            })
            .await
            .unwrap();
        let held = channel
            .admit(proof, inbox, self.record.clone())
            .await
            .unwrap();
        let execution = channel.authorize_execution(&invokes, &held).await.unwrap();
        channel
            .release_execution_hold(&execution, &held)
            .await
            .unwrap();
        let command = f
            .resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.status, AgentResponseStatus::Queued);
        Queued {
            invokes,
            store,
            channel,
            command,
        }
    }
    pub async fn provenance(&self, id: &layer2_core::AgentId) -> AgentCreationProvenance {
        let row = self
            .f
            .original
            .client
            .database(&self.f.original.database)
            .collection::<Document>("agent_creation_provenance")
            .find_one(doc! {"_id":id.as_str()})
            .await
            .unwrap()
            .unwrap();
        crate::store::agent_provenance::read(&row).unwrap()
    }
    pub async fn close(self) {
        drop(self.worker);
        drop(self.registry);
        self.f.original.close().await;
    }
}
