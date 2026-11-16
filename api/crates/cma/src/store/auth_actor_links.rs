//! Majority-durable actor grants, with authority and profile encrypted at rest.
use super::*;
use crate::auth::actor_links::ActorLinkRecord;

const KIND: &str = "event-actor-link";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ActorLinkDoc {
    #[serde(rename = "_id")]
    actor_key: String,
    document_schema_version: i64,
    link_id: String,
    revision: i64,
    active: bool,
    renewed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wakeup_due_at: Option<i64>,
    authority: CipherEnvelope,
}
impl MongoAuthStore {
    pub(super) async fn actor_wakeups_due(
        &self,
        before: u64,
        limit: usize,
    ) -> Result<Vec<ActorLinkRecord>, StoreError> {
        use futures_util::TryStreamExt as _;
        let docs = self
            .actor_links
            .find(doc! {"active":true,"wakeup_due_at":{
            "$type":"number", "$lte":i64::try_from(before).map_err(|_| unavailable())?}})
            .sort(doc! {"wakeup_due_at":1,"_id":1})
            .limit(limit.clamp(1, 100) as i64)
            .await
            .map_err(|_| unavailable())?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|_| unavailable())?;
        docs.into_iter()
            .map(|doc| self.decode_actor_link(doc))
            .collect()
    }
    pub(super) async fn actor_links_due(
        &self,
        before: u64,
        after_actor_key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ActorLinkRecord>, StoreError> {
        use futures_util::TryStreamExt as _;
        let mut filter = doc! {"active":true,"renewed_at":{"$lte":i64::try_from(before).map_err(|_| unavailable())?}};
        if let Some(key) = after_actor_key {
            filter.insert("_id", doc! {"$gt":key});
        }
        let docs = self
            .actor_links
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(limit.clamp(1, 100) as i64)
            .await
            .map_err(|_| unavailable())?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|_| unavailable())?;
        docs.into_iter()
            .map(|doc| self.decode_actor_link(doc))
            .collect()
    }
    fn actor_link_doc(&self, row: &ActorLinkRecord) -> Result<ActorLinkDoc, StoreError> {
        row.validate()?;
        let payload = serde_json::to_string(row).map_err(|_| unavailable())?;
        Ok(ActorLinkDoc {
            actor_key: row.actor_key.clone(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            link_id: row.link_id.clone(),
            revision: i64::try_from(row.revision).map_err(|_| unavailable())?,
            active: row.active,
            renewed_at: i64::try_from(row.renewed_at).map_err(|_| unavailable())?,
            wakeup_due_at: row
                .wakeup
                .as_ref()
                .and_then(|wakeup| wakeup.due_at())
                .map(|at| i64::try_from(at).map_err(|_| unavailable()))
                .transpose()?,
            authority: self
                .cipher
                .seal(KIND, row.actor_key.as_bytes(), "authority", &payload)?,
        })
    }
    fn decode_actor_link(&self, doc: ActorLinkDoc) -> Result<ActorLinkRecord, StoreError> {
        if doc.document_schema_version != DOCUMENT_SCHEMA_VERSION {
            return Err(unavailable());
        }
        let plaintext =
            self.cipher
                .open(KIND, doc.actor_key.as_bytes(), "authority", doc.authority)?;
        let row: ActorLinkRecord = serde_json::from_str(&plaintext).map_err(|_| unavailable())?;
        row.validate()?;
        if row.actor_key != doc.actor_key
            || row.link_id != doc.link_id
            || i64::try_from(row.revision).ok() != Some(doc.revision)
            || row.active != doc.active
            || i64::try_from(row.renewed_at).ok() != Some(doc.renewed_at)
            || row
                .wakeup
                .as_ref()
                .and_then(|wakeup| wakeup.due_at())
                .and_then(|at| i64::try_from(at).ok())
                != doc.wakeup_due_at
        {
            return Err(unavailable());
        }
        Ok(row)
    }
    pub(super) async fn read_actor_link(
        &self,
        filter: mongodb::bson::Document,
    ) -> Result<Option<ActorLinkRecord>, StoreError> {
        self.actor_links
            .find_one(filter)
            .await
            .map_err(|_| unavailable())?
            .map(|doc| self.decode_actor_link(doc))
            .transpose()
    }
    pub(super) async fn write_actor_link(
        &self,
        expected: Option<u64>,
        row: ActorLinkRecord,
    ) -> Result<bool, StoreError> {
        if row.revision
            != expected
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(unavailable)?
        {
            return Err(unavailable());
        }
        let doc = self.actor_link_doc(&row)?;
        if let Some(expected) = expected {
            let expected = i64::try_from(expected).map_err(|_| unavailable())?;
            Ok(self
                .actor_links
                .replace_one(doc! {"_id":&row.actor_key,"revision":expected}, doc)
                .await
                .map_err(|_| unavailable())?
                .modified_count
                == 1)
        } else {
            match self.actor_links.insert_one(doc).await {
                Ok(_) => Ok(true),
                Err(error) if super::super::sandbox_mongo::is_duplicate_key(&error) => Ok(false),
                Err(_) => Err(unavailable()),
            }
        }
    }
}
fn unavailable() -> StoreError {
    StoreError::Unavailable("actor link storage unavailable".into())
}

#[cfg(test)]
#[path = "auth_actor_links_tests.rs"]
mod tests;
