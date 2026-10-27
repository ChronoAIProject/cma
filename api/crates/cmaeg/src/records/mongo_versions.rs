//! Small indexed revision reads for internal Activity consistency checks.
use super::{failure, MongoRecords, RecordError, RecordKind};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, DateTime},
    options::Hint,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    version: i64,
    schema_version: i32,
    #[serde(default)]
    expires_at: Option<DateTime>,
}

impl MongoRecords {
    pub(crate) async fn versions(
        &self,
        owner: &str,
        ids: &[String],
    ) -> Result<Vec<u64>, RecordError> {
        let rows: Vec<Version> = self
            .records
            .clone_with_type::<Version>()
            .find(doc! {"_id":{"$in":ids}})
            .hint(Hint::Name("_id_".into()))
            .projection(doc! {"_id":1,"owner":1,"version":1,"schema_version":1,"expires_at":1})
            .limit(ids.len() as i64)
            .max_time(std::time::Duration::from_secs(2))
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        // Read expiry as well as the revision: the TTL monitor may not yet have
        // deleted an expired bucket, and a malformed expiry must fail closed.
        let now = DateTime::now();
        let mut versions = vec![0; ids.len()];
        let mut seen = vec![false; ids.len()];
        let invalid = || RecordError::Malformed("invalid record watermark".into());
        for row in rows {
            let index = ids
                .iter()
                .position(|id| *id == row.id)
                .ok_or_else(invalid)?;
            if seen[index] || row.owner != owner || row.version < 1 || row.schema_version != 1 {
                return Err(invalid());
            }
            seen[index] = true;
            let live = match (self.kind, row.expires_at) {
                (RecordKind::Interaction, Some(expiry)) => expiry > now,
                (RecordKind::Channel | RecordKind::Connection | RecordKind::ThreadFence, None) => {
                    true
                }
                _ => return Err(invalid()),
            };
            if live {
                versions[index] = row.version as u64;
            }
        }
        Ok(versions)
    }
}
