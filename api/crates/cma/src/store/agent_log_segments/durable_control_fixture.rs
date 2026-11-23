//! Install a small, certified segmented fixture from the lifecycle harness's
//! original logs. This prepares storage format, not a migration assertion.
use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::{store::AgentLogStore, Agent};
use mongodb::bson::doc;

impl MongoSandboxStore {
    pub(crate) async fn install_segmented_control_fixture(&self, agent: &Agent) {
        let database = self.client.database(&self.agents.namespace().db);
        schema::migrate(&database).await.unwrap();
        let entries = self.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(entries.len() as u64, agent.conversation.last_seq);
        self.agents
            .delete_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap();
        self.agent_log
            .delete_many(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap();
        let (installed, entries) =
            mongo_tests::fixtures::install(self, agent.clone(), entries, 0, 2).await;
        assert_eq!(installed, *agent);
        mongo_tests::fixtures::certify(self, agent, &entries).await;
        mongo_tests::fixtures::certify_output(self, agent, &entries).await;
    }
}
