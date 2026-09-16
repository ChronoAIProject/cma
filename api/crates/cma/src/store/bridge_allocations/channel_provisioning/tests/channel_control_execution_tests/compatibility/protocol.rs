use super::*;
use layer2_core::bridge::invoke::BridgeInvokeService;
use layer2_core::resource_access::ResourceAccessService;
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReaderCase {
    pub body: Value,
    pub expected: CallbackAcceptance,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scenario {
    pub schema_version: u8,
    pub run_id: String,
    pub database: String,
    pub mode: String,
    pub bridge_id: BridgeId,
    pub source_command_id: layer2_core::ids::AgentCommandId,
    pub control_command_id: layer2_core::ids::AgentCommandId,
    pub source_turn: u64,
    pub runtime_generation: u64,
    pub runtime_conversation: String,
    pub cases: Vec<ReaderCase>,
}

pub(super) struct Harness {
    pub scenario: Scenario,
    pub client: Client,
    pub resources: Arc<crate::store::MongoSandboxStore>,
    pub bridges: Arc<MongoBridgeStore>,
    pub store: Arc<crate::store::MongoBridgeInvokeStore>,
    pub invokes: BridgeInvokeService,
    pub inbox: crate::store::MongoCallbackInbox,
}

impl Harness {
    pub async fn open() -> Self {
        let path = std::env::var("CHRONO_TEST_COMPAT_SCENARIO_PATH").unwrap();
        let scenario: Scenario = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(scenario.schema_version, 1);
        assert_eq!(
            scenario.run_id,
            std::env::var("CHRONO_TEST_COMPAT_RUN").unwrap()
        );
        assert!(scenario.run_id.starts_with("cma-control-compat-"));
        assert!(scenario.database.starts_with("cma_bridge_contract_"));
        let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let marker = client
            .database(&scenario.database)
            .collection::<Document>("compatibility_scenarios")
            .find_one(doc! {"_id":"scenario","run_id":&scenario.run_id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(marker.get_str("mode").unwrap(), scenario.mode);
        let resources = Arc::new(
            crate::store::MongoSandboxStore::open(&client, &scenario.database)
                .await
                .unwrap(),
        );
        let bridges = Arc::new(
            MongoBridgeStore::open(
                &client,
                &scenario.database,
                "fixture-deployment",
                &custody_key(),
            )
            .await
            .unwrap(),
        );
        let store = Arc::new(
            crate::store::MongoBridgeInvokeStore::open(
                bridges.clone(),
                resources.clone(),
                &scenario.database,
            )
            .await
            .unwrap(),
        );
        let invokes = BridgeInvokeService::new(
            BridgeService::new(bridges.clone(), Arc::new(Authority), Arc::new(Authority)),
            Arc::new(ResourceAccessService::new(
                resources.clone(),
                Arc::new(Authority),
            )),
            resources.clone(),
            store.clone(),
        );
        let inbox = crate::store::MongoCallbackInbox::open(
            &bridges,
            &scenario.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap();
        Self {
            scenario,
            client,
            resources,
            bridges,
            store,
            invokes,
            inbox,
        }
    }
    pub async fn command(&self) -> AdmittedAgentCommand {
        self.resources
            .get_command(&self.scenario.control_command_id)
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn control_row(&self) -> Document {
        self.client
            .database(&self.scenario.database)
            .collection::<Document>("agent_commands")
            .find_one(doc! {"_id":self.scenario.control_command_id.as_str()})
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn logs(&self) -> Vec<layer2_core::sandbox::AgentLogEntry> {
        self.resources
            .agent_log(&self.command().await.receipt.envelope.agent_id, 0, 100)
            .await
            .unwrap()
    }
    pub async fn effect_count(&self) -> u64 {
        self.client
            .database(&self.scenario.database)
            .collection::<Document>("compatibility_runtime_inputs")
            .count_documents(doc! {})
            .await
            .unwrap()
    }
    pub async fn record_result(&self, phase: &str, result: Value) {
        self.client.database(&self.scenario.database).collection::<Document>("compatibility_observations")
            .insert_one(doc! {"run_id":&self.scenario.run_id,"mode":&self.scenario.mode,"phase":phase,"result":crate::store::sandbox_mongo::record_bson(&result).unwrap()}).await.unwrap();
    }
}
