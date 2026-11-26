//! One-way, resumable baseline import. Never inventories object storage itself.
use super::quota::*;
use futures_util::TryStreamExt;
use layer2_core::document_artifacts::*;
use mongodb::{
    bson::{doc, Document},
    Database,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const MAX_MANIFEST_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_BASELINE_OBJECTS: usize = 100_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub schema_version: u32,
    pub activation_id: String,
    pub complete_storage_inventory: bool,
    pub writers_drained: bool,
    pub objects: Vec<DocumentSourceReservation>,
}

pub(super) struct Inventory {
    pub digest: String,
    pub activation: String,
    objects: BTreeMap<String, DocumentSourceReservation>,
    documents: BTreeMap<String, Document>,
    owners: BTreeMap<String, Document>,
}
impl Inventory {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, DocumentError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(DocumentError::Capacity);
        }
        let manifest: Manifest =
            serde_json::from_slice(bytes).map_err(|_| DocumentError::Invalid)?;
        if manifest.schema_version != 1
            || !manifest.complete_storage_inventory
            || !manifest.writers_drained
            || !valid_activation(&manifest.activation_id)
            || manifest.objects.len() > MAX_BASELINE_OBJECTS
        {
            return Err(DocumentError::Invalid);
        }
        let mut objects = BTreeMap::new();
        let mut documents = BTreeMap::new();
        let mut usage = BTreeMap::<String, (BTreeSet<String>, u64, u64)>::new();
        for source in manifest.objects {
            source.validate()?;
            if objects
                .insert(source.object_key(), source.clone())
                .is_some()
            {
                return Err(DocumentError::Invalid);
            }
            let document = document_row(&source);
            documents.insert(
                document
                    .get_str("_id")
                    .map_err(|_| DocumentError::Corrupt)?
                    .into(),
                document,
            );
            let used = usage.entry(source.owner_hash.clone()).or_default();
            used.0.insert(source.document_id.clone());
            used.1 = used
                .1
                .checked_add(source.bytes)
                .ok_or(DocumentError::Capacity)?;
            used.2 += 1;
        }
        let owners = usage.into_iter().map(|(hash, (documents, bytes, objects))| {
            let id = owner_id(&hash);
            (id.clone(), doc! {"_id": id, "document_schema_version": 1_i64, "kind": "owner", "fence": 0_i64,
                "documents": documents.len() as i64, "bytes": bytes as i64, "objects": objects as i64})
        }).collect();
        Ok(Self {
            digest: digest(bytes),
            activation: manifest.activation_id,
            objects,
            documents,
            owners,
        })
    }

    pub(super) fn row_count(&self) -> usize {
        self.objects.len() + self.documents.len() + self.owners.len()
    }

    /// Check every referenced revision, including attribution/key/size. Orphans
    /// have no Mongo reference; the storage owner's complete inventory attests them.
    async fn check_references(&self, database: &Database) -> Result<(), DocumentError> {
        let mut cursor = database
            .collection::<Document>(super::COLLECTION)
            .find(doc! {})
            .projection(
                doc! {"owner_subject": 1, "document_schema_version": 1, "record.metadata.id": 1,
                "record.metadata.owner_subject": 1, "record.revisions.object_key": 1,
                "record.revisions.metadata.sha256": 1, "record.revisions.metadata.size_bytes": 1, "deletion": 1, "audit": 1, "audit_owner_subject": 1},
            )
            .limit(MAX_BASELINE_OBJECTS as i64 + 1)
            .batch_size(16)
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        let mut count = 0;
        while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
            count += 1;
            if count > MAX_BASELINE_OBJECTS {
                return Err(DocumentError::Capacity);
            }
            self.check_reference(&row)?;
        }
        Ok(())
    }

    fn check_reference(&self, row: &Document) -> Result<(), DocumentError> {
        let corrupt = |_| DocumentError::Corrupt;
        if super::deletion::decode(row)?.is_some() {
            return Ok(());
        }
        if row.get_i64("document_schema_version").ok() != Some(1) {
            return Err(DocumentError::Corrupt);
        }
        let record = row.get_document("record").map_err(corrupt)?;
        let meta = record.get_document("metadata").map_err(corrupt)?;
        let owner = row.get_str("owner_subject").map_err(corrupt)?;
        let id = row.get_str("_id").map_err(corrupt)?;
        if meta.get_str("owner_subject").ok() != Some(owner) || meta.get_str("id").ok() != Some(id)
        {
            return Err(DocumentError::Corrupt);
        }
        let revisions = record.get_array("revisions").map_err(corrupt)?;
        if revisions.is_empty() || revisions.len() > MAX_REVISIONS {
            return Err(DocumentError::Corrupt);
        }
        for revision in revisions {
            let revision = revision.as_document().ok_or(DocumentError::Corrupt)?;
            let metadata = revision.get_document("metadata").map_err(corrupt)?;
            let source = DocumentSourceReservation {
                owner_hash: digest(owner.as_bytes()),
                document_id: id.into(),
                sha256: metadata.get_str("sha256").map_err(corrupt)?.into(),
                bytes: number(metadata, "size_bytes")?,
            };
            source.validate()?;
            if revision.get_str("object_key").ok() != Some(source.object_key().as_str())
                || self.objects.get(&source.object_key()) != Some(&source)
            {
                return Err(DocumentError::Corrupt);
            }
        }
        Ok(())
    }
}

pub(super) fn valid_activation(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}

impl MongoDocumentQuotaStore {
    pub(super) async fn prepare(&self, activation: &str) -> Result<(), DocumentError> {
        if !valid_activation(activation) {
            return Err(DocumentError::Invalid);
        }
        if let Some(state) = self
            .rows
            .find_one(doc! {"_id": STATE})
            .max_time(IO)
            .await
            .map_err(unavailable)?
        {
            return check_activation(&state, activation);
        }
        if self
            .rows
            .find_one(doc! {})
            .projection(doc! {"_id": 1})
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .is_some()
        {
            return Err(DocumentError::Corrupt);
        }
        let state = doc! {"_id": STATE, "document_schema_version": 1_i64, "kind": "activation", "phase": "preparing", "activation_id": activation};
        self.rows
            .update_one(doc! {"_id": STATE}, doc! {"$setOnInsert": state})
            .upsert(true)
            .await
            .map_err(unavailable)?;
        let state = self
            .rows
            .find_one(doc! {"_id": STATE})
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .ok_or(DocumentError::Unavailable)?;
        check_activation(&state, activation)
    }

    pub(super) async fn install(
        &self,
        database: &Database,
        inventory: &Inventory,
        apply: bool,
    ) -> Result<(), DocumentError> {
        let state = self
            .rows
            .find_one(doc! {"_id": STATE})
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .ok_or(DocumentError::Unavailable)?;
        check_activation(&state, &inventory.activation)?;
        if let Ok(bound) = state.get_str("inventory_sha256") {
            if bound != inventory.digest {
                return Err(DocumentError::Conflict);
            }
        } else if state.contains_key("inventory_sha256") {
            return Err(DocumentError::Corrupt);
        }
        if ready(&state) {
            return Ok(());
        } // Never replay baseline over live counters.
        inventory.check_references(database).await?;
        if !apply {
            return Ok(());
        }
        let bound = self.rows.update_one(doc! {"_id": STATE, "phase": "preparing", "activation_id": &inventory.activation,
            "$or": [{"inventory_sha256": {"$exists": false}}, {"inventory_sha256": &inventory.digest}]},
            doc! {"$set": {"inventory_sha256": &inventory.digest}}).await.map_err(unavailable)?;
        if bound.matched_count != 1 {
            return Err(DocumentError::Conflict);
        }
        let start = if state.contains_key("next_row") {
            number(&state, "next_row")? as usize
        } else {
            0
        };
        if start > inventory.row_count() {
            return Err(DocumentError::Corrupt);
        }
        let rows = inventory
            .owners
            .values()
            .chain(inventory.documents.values())
            .cloned()
            .map(Ok)
            .chain(inventory.objects.values().map(object_row));
        for (position, row) in rows.enumerate().skip(start) {
            self.install_row(row?).await?;
            // Resumes after a bounded command timeout; never marks a row before
            // its majority write/readback, never overwrites an installed row.
            if (position + 1) % 32 == 0 || position + 1 == inventory.row_count() {
                let progress = self.rows.update_one(doc! {"_id": STATE, "phase": "preparing", "inventory_sha256": &inventory.digest},
                    doc! {"$max": {"next_row": (position + 1) as i64}}).await.map_err(unavailable)?;
                if progress.matched_count != 1 {
                    return Err(DocumentError::Conflict);
                }
            }
        }
        let count = self
            .rows
            .count_documents(doc! {})
            .limit(inventory.row_count() as u64 + 2)
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        if count != inventory.row_count() as u64 + 1 {
            return Err(DocumentError::Corrupt);
        }
        inventory.check_references(database).await?;
        let result = self.rows.update_one(doc! {"_id": STATE, "phase": "preparing", "activation_id": &inventory.activation, "inventory_sha256": &inventory.digest},
            doc! {"$set": {"phase": "ready", "baseline_rows": inventory.row_count() as i64, "next_row": inventory.row_count() as i64}}).await.map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(DocumentError::Conflict);
        }
        Ok(())
    }

    async fn install_row(&self, row: Document) -> Result<(), DocumentError> {
        let id = row.get_str("_id").map_err(|_| DocumentError::Corrupt)?;
        self.rows
            .update_one(doc! {"_id": id}, doc! {"$setOnInsert": &row})
            .upsert(true)
            .await
            .map_err(unavailable)?;
        let actual = self
            .rows
            .find_one(doc! {"_id": id})
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        if actual.as_ref() != Some(&row) {
            return Err(DocumentError::Corrupt);
        }
        Ok(())
    }
}

fn check_activation(state: &Document, activation: &str) -> Result<(), DocumentError> {
    validate_row(state, "activation")?;
    if state.get_str("activation_id").ok() != Some(activation)
        || !matches!(state.get_str("phase").ok(), Some("preparing" | "ready"))
    {
        return Err(DocumentError::Conflict);
    }
    Ok(())
}
