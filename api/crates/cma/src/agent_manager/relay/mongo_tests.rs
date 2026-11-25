use super::*;
use layer2_core::agent_ownership::{observed_transport, transport_observation_scope};
use layer3_adapter::{
    hosted::{HostedAgentOpenSettings, HostedControlTransport},
    owner_control::{OwnerControlRoute, OwnerControlRouter},
    wire::{AgentOpenReceipt, AgentRef, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    AgentLaunch, ConversationChannel, ConversationSnapshot, ConversationUpdate, RuntimeHandle,
};
use std::sync::Mutex;

#[path = "settings_http_tests.rs"]
mod settings;

#[path = "drain_tests.rs"]
mod drain;

#[path = "tests/file_handoff.rs"]
mod file_handoff;

#[path = "shared_stream_tests.rs"]
mod shared_streams;

#[path = "preferences_tests.rs"]
mod preferences;

struct CaptureTransport(Mutex<Vec<OwnershipToken>>);
#[async_trait::async_trait]
impl HostedControlTransport for CaptureTransport {
    async fn exchange(
        &self,
        _: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let RequestFrame::CaptureAgent(_) = request else {
            panic!("unexpected control")
        };
        self.0.lock().unwrap().push(
            layer2_core::agent_ownership::execution_owner()
                .expect("capture has current owner authority"),
        );
        Ok(ResponseFrame::CaptureAgent(ConversationSnapshot {
            conversation_id: "synthetic-conversation".into(),
            archive: vec![1, 2, 3],
            archive_sha256: "a".repeat(64),
        }))
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!()
    }
    async fn retire(&self, _: &RuntimeHandle) -> layer3_adapter::Result<()> {
        unreachable!()
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn maintenance_control_uses_owner_and_captures_epoch_with_real_mongo_lineage() {
    use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
    for count in [2, 3] {
        let (mongo, name, durable) = database().await;
        let (sandbox, agent) = fixture(&durable, "maintenance-relay").await;
        let durable = Arc::new(durable);
        let (store, instances) = cluster(count).await;
        let (owner, authority) = acquire(&store, &instances, &agent.id).await;
        let mut transports = Vec::new();
        for instance in &instances {
            let transport = Arc::new(CaptureTransport(Mutex::default()));
            instance.relay.install_controls(
                durable.clone(),
                vec![(OwnerControlRoute::new("synthetic"), transport.clone())],
            );
            transports.push(transport);
        }
        let runtime = RuntimeHandle(sandbox.runtime.unwrap().runtime_id);
        let request = RequestFrame::CaptureAgent(AgentRef {
            agent_id: layer3_adapter::AgentSlot::new(agent.id.to_string()),
        });
        for instance in &instances {
            transport_observation_scope(async {
                let result = instance
                    .relay
                    .route("synthetic", &runtime, &request)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(matches!(result, ResponseFrame::CaptureAgent(_)));
                assert_eq!(observed_transport(&agent.id), Some(authority.token.clone()));
            })
            .await;
        }
        for (index, transport) in transports.iter().enumerate() {
            assert_eq!(
                transport.0.lock().unwrap().len(),
                if index == owner { count } else { 0 }
            );
        }
        assert!(instances[(owner + 1) % count]
            .relay
            .route(
                "synthetic",
                &RuntimeHandle("wrong-runtime".into()),
                &request
            )
            .await
            .unwrap()
            .is_err());
        // Only never-adopted prepared Agents may use legacy control. A released
        // permanent epoch must still refuse fallback if its aggregate is absent.
        let unadopted = RequestFrame::CaptureAgent(AgentRef {
            agent_id: layer3_adapter::AgentSlot::new("prepared-unadopted"),
        });
        assert!(instances[0]
            .relay
            .route("synthetic", &runtime, &unadopted)
            .await
            .is_none());
        mongo
            .database(&name)
            .collection::<mongodb::bson::Document>("agents")
            .delete_one(mongodb::bson::doc! {"_id": agent.id.as_str()})
            .await
            .unwrap();
        instances[owner]
            .coordinator
            .handoff(&authority, async { Ok(()) })
            .await
            .unwrap();
        assert!(store.owner(&agent.id).await.unwrap().is_none());
        assert!(instances[0]
            .relay
            .route("synthetic", &runtime, &request)
            .await
            .unwrap()
            .is_err());
        mongo.database(&name).drop().await.unwrap();
    }
}

#[path = "tests/file_read_retry/mod.rs"]
mod file_read_retry;
