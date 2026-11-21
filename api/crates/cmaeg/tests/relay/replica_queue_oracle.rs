//! Independently read the retained FIFO item and its drained head.
use cmaeg::{
    connections::store::index_id,
    ingress::routing_store::AdmittedPlan,
    records::{mongo::MongoRecords, RecordClient, RecordError},
};
use cmaeg_core::seal::{open_connection, open_interaction, MasterKeyRing};
use serde_json::Value;
use std::sync::Arc;

pub struct QueueOracle {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
}

impl QueueOracle {
    pub async fn open(uri: &str, database: &str, keys: Arc<MasterKeyRing>) -> Self {
        let client = mongodb::Client::with_uri_str(uri)
            .await
            .expect("queue oracle client");
        let database = MongoRecords::open(client.database(database))
            .await
            .expect("queue oracle grants");
        Self {
            records: RecordClient::new("http://127.0.0.1:1", "test-service-bearer")
                .unwrap()
                .with_database(database),
            keys,
        }
    }

    pub async fn completed(&self, plan: &AdmittedPlan) -> usize {
        let mut completed = 0;
        for target in &plan.targets {
            let id = index_id("cmaeg-partition-item-v1", &[&target.id]);
            let records = self.records.clone().interactions();
            let row = match records.get(records.record_url(&id).unwrap().as_str()).await {
                Ok(row) => row,
                Err(RecordError::NotFound) => continue,
                Err(_) => panic!("queue oracle item read failed"),
            };
            let bytes = open_interaction(&self.keys, &id, &row.blob)
                .expect("queue item seal")
                .payload;
            let item: Value = serde_json::from_slice(&bytes).expect("queue item schema");
            assert_eq!(item["kind"], "cmaeg_partition_item_v1");
            assert!(item["identity"] == serde_json::to_value(&plan.identity).unwrap());
            assert!(item["target"] == target.id && item["work_id"] == target.id);
            assert!(item["request_hash"] == plan.request_hash);
            assert_eq!(item["retained_until"], plan.retained_until);
            assert_eq!(item["effect"], false);
            if item["done"] != true {
                continue;
            }
            let head_id = item["partition"].as_str().expect("queue item partition");
            let records = self.records.clone().connections();
            let row = records
                .get(records.record_url(head_id).unwrap().as_str())
                .await
                .expect("queue head read");
            let bytes = open_connection(&self.keys, head_id, &row.blob)
                .expect("queue head seal")
                .payload;
            let head: Value = serde_json::from_slice(&bytes).expect("queue head schema");
            assert_eq!(head["kind"], "cmaeg_partition_v1");
            assert!(head["owner"] == plan.identity.owner);
            assert!(head["id"] == head_id);
            assert!(!head["queue"]
                .as_array()
                .unwrap()
                .contains(&target.id.clone().into()));
            completed += 1;
        }
        completed
    }
}
