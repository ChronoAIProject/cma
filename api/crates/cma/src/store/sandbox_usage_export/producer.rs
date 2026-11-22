use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Producer {
    #[serde(rename = "_id")]
    id: Identity,
    document_schema_version: i64,
    destination: String,
    forbidden: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    usage_transport_namespace: String,
}

impl CaptureStore {
    // Document IDs cannot collide with C1's string namespace inventory IDs.
    // Existing inventory custody and schema preflight apply; no startup DDL.
    pub(super) async fn bind_producer(&self, destination: &str) -> Result<bool, SandboxStoreError> {
        if destination.len() != 64
            || !destination
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(corrupt());
        }
        let id = doc! {"usage_transport_namespace": &self.config.namespace};
        let rows = self.collection(INVENTORY);
        if rows
            .find_one(doc! {"_id":&id})
            .await
            .map_err(unavailable)?
            .is_none()
        {
            let row = mongodb::bson::serialize_to_document(&Producer {
                id: Identity {
                    usage_transport_namespace: self.config.namespace.clone(),
                },
                document_schema_version: 1,
                destination: destination.into(),
                forbidden: false,
            })
            .map_err(|_| corrupt())?;
            if let Err(error) = rows.insert_one(row).await {
                if !super::super::sandbox_mongo::is_duplicate_key(&error) {
                    return Err(unavailable(error));
                }
            }
        }
        let row = rows
            .find_one(doc! {"_id":id})
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let producer: Producer =
            mongodb::bson::deserialize_from_document(row).map_err(|_| corrupt())?;
        if producer.document_schema_version != 1 || producer.destination != destination {
            return Err(corrupt());
        }
        Ok(producer.forbidden)
    }

    pub(super) async fn forbid_producer(&self, destination: &str) -> Result<(), SandboxStoreError> {
        let result = self.collection(INVENTORY).update_one(
            doc! {"_id":{"usage_transport_namespace":&self.config.namespace},"document_schema_version":1_i64,"destination":destination},
            doc! {"$set":{"forbidden":true}},
        ).await.map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(corrupt());
        }
        tracing::error!("usage producer authority refused; delivery is durably stopped");
        Ok(())
    }
}
