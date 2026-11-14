//! Independent authenticated reads; FIFO records and target executions are separate.
use super::{delivery, ledger::hash};
use cmaeg::{
    connections::store::index_id,
    ingress::routing_store::{EventIdentity, PlanStore},
    records::{mongo::MongoRecords, RecordClient, RecordError},
    turns::routed::TargetState,
};
use cmaeg_core::seal::{open_interaction, MasterKeyRing};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

pub struct Oracle {
    plans: PlanStore,
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    client: mongodb::Client,
    receipts: PathBuf,
}
impl Oracle {
    pub async fn open(
        uri: &str,
        database: &str,
        keys: Arc<MasterKeyRing>,
        receipts: PathBuf,
    ) -> Result<Self, &'static str> {
        let client = super::config::local_mongo(uri).await?;
        let store = MongoRecords::open(client.database(database))
            .await
            .map_err(|_| "oracle runtime grants")?;
        let records = RecordClient::new("http://127.0.0.1:1", &super::bootstrap::random_secret()?)
            .map_err(|_| "oracle client")?
            .with_database(store);
        Ok(Self {
            plans: PlanStore::new(records.clone(), keys.clone()),
            records: records.interactions(),
            keys,
            client,
            receipts,
        })
    }
    pub async fn shutdown(&self) {
        self.client.clone().shutdown().await;
    }
    async fn retained(&self, id: &str) -> Result<Option<(Value, u64, String)>, &'static str> {
        let url = self.records.record_url(id).map_err(|_| "oracle identity")?;
        let row = match self.records.get(url.as_str()).await {
            Ok(row) => row,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err("oracle record read"),
        };
        let bytes = open_interaction(&self.keys, id, &row.blob)
            .map_err(|_| "oracle sealed record")?
            .payload;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "oracle record schema")?;
        let path = self.receipts.join(format!(
            "{}-{}-{}.json",
            hash(id.as_bytes()),
            row.version,
            hash(&bytes)
        ));
        if !path.exists() {
            super::config::private_json(
                &path,
                &json!({"id":id,"version":row.version,"sealed_blob":row.blob,"plaintext":value}),
            )?;
        }
        Ok(Some((value, row.version, hash(&bytes))))
    }
    pub async fn observe(
        &self,
        identity: &EventIdentity,
        replicas: bool,
    ) -> Result<Value, &'static str> {
        let Some(plan) = self
            .plans
            .load(identity)
            .await
            .map_err(|_| "oracle plan read")?
        else {
            return Ok(
                json!({"event_sha256":hash(identity.event_id.as_str().as_bytes()),"admission_sha256":hash(identity.admission_id().as_bytes()),"root_present":false,"targets":[]}),
            );
        };
        let root = self
            .retained(&plan.admission_id)
            .await?
            .ok_or("oracle root vanished")?;
        let deployment = hash(b"http://127.0.0.1:18999");
        let mut rows = Vec::new();
        for target in &plan.targets {
            let partition = index_id(
                "cmaeg-partition-v1",
                &[
                    &deployment,
                    &identity.owner,
                    identity.channel_id.as_str(),
                    identity.record_id.as_str(),
                    target.launch.consumer.as_str(),
                    target.launch.provider.slug.as_str(),
                    target.launch.event.conversation_id.as_str(),
                    &target.launch.event.partition_key,
                ],
            );
            let item_id = index_id("cmaeg-partition-item-v1", &[&target.id]);
            let item = self.retained(&item_id).await?;
            let head = self.retained(&partition).await?;
            let target_state = self.retained(&target.id).await?;
            let mut terminal = false;
            let mut target_hash = None;
            let mut target_version = None;
            if let Some((value, version, digest)) = target_state {
                let state: TargetState =
                    serde_json::from_value(value).map_err(|_| "oracle target schema")?;
                if state.id != target.id
                    || state.root != plan.admission_id
                    || state.owner != identity.owner
                    || state.request_hash != plan.request_hash
                    || state.retained_until != plan.retained_until
                {
                    return Err("oracle target binding");
                }
                terminal = delivery::observe(&state).confirmed_terminal_text();
                target_hash = Some(digest);
                target_version = Some(version);
            }
            let mut fifo_done = false;
            let mut fifo_hash = None;
            let mut fifo_version = None;
            if let Some((value, version, digest)) = item {
                if value["kind"] != "cmaeg_partition_item_v1"
                    || value["target"] != target.id
                    || value["partition"] != partition
                    || value["work_id"] != target.id
                    || value["effect"] != false
                    || value["request_hash"] != plan.request_hash
                    || value["identity"]
                        != serde_json::to_value(identity).map_err(|_| "oracle identity encoding")?
                    || value["retained_until"] != plan.retained_until
                {
                    return Err("oracle FIFO binding");
                }
                fifo_done = value["done"].as_bool().ok_or("oracle FIFO done flag")?;
                fifo_hash = Some(digest);
                fifo_version = Some(version);
            }
            let mut head_released = false;
            let mut head_hash = None;
            if let Some((value, _, digest)) = head {
                if value["kind"] != "cmaeg_partition_v1"
                    || value["id"] != partition
                    || value["deployment"] != deployment
                    || value["owner"] != identity.owner
                {
                    return Err("oracle FIFO head binding");
                }
                let queue = value["queue"].as_array().ok_or("oracle FIFO head queue")?;
                head_released = !queue.iter().any(|entry| entry == &target.id);
                head_hash = Some(digest);
            }
            rows.push(json!({"target_sha256":hash(target.id.as_bytes()),"idempotency_sha256":hash(target.launch.idempotency_key.as_str().as_bytes()),
                "target_present":target_hash.is_some(),"target_plaintext_sha256":target_hash,"target_version":target_version,
                "terminal_confirmed_text":terminal,"fifo_present":fifo_hash.is_some(),"fifo_done":fifo_done,"fifo_plaintext_sha256":fifo_hash,"fifo_version":fifo_version,
                "head_plaintext_sha256":head_hash,"head_released":head_released,"complete":terminal&&(!replicas||(fifo_done&&head_released))}));
        }
        Ok(
            json!({"event_sha256":hash(identity.event_id.as_str().as_bytes()),"admission_sha256":hash(plan.admission_id.as_bytes()),"root_present":true,
            "root_version":root.1,"root_plaintext_sha256":root.2,"request_sha256":plan.request_hash,"targets":rows,"retained_until":plan.retained_until}),
        )
    }
}
