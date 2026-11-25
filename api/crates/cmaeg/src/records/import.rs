//! Explicit offline transfer; the gateway never receives legacy DB credentials.
use super::{ImportGate, RecordError, RecordKind};
use base64::{engine::general_purpose::STANDARD, Engine};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime, Document},
    options::{Acknowledgment, CollectionOptions, ReadConcern, WriteConcern},
    Database,
};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRecord {
    #[serde(default)]
    kind: RecordKind,
    record_id: String,
    owner: String,
    version: String,
    blob: String,
    #[serde(default)]
    expires_at: Option<i64>,
}
fn decode(line: &str) -> Result<(RecordKind, Document), RecordError> {
    let bad = || RecordError::Rejected("invalid legacy record export".into());
    let record: LegacyRecord = serde_json::from_str(line).map_err(|_| bad())?;
    let id = uuid::Uuid::parse_str(&record.record_id).map_err(|_| bad())?;
    let version = record.version.parse::<i64>().map_err(|_| bad())?;
    let bytes = STANDARD.decode(record.blob).map_err(|_| bad())?;
    if id.to_string() != record.record_id
        || version < 1
        || record.owner.is_empty()
        || record.owner.len() > 512
        || bytes.is_empty()
        || bytes.len() > 65536
    {
        return Err(bad());
    }
    let mut document = doc! {"_id":record.record_id,"owner":record.owner,"version":version,"schema_version":1_i32,"blob":Binary{subtype:BinarySubtype::Generic,bytes}};
    match (record.kind, record.expires_at) {
        (RecordKind::Interaction, Some(expiry)) if expiry > 0 => {
            document.insert(
                "expires_at",
                DateTime::from_millis(expiry.checked_mul(1000).ok_or_else(bad)?),
            );
        }
        (RecordKind::Channel | RecordKind::Connection, None) => (),
        _ => return Err(bad()),
    }
    Ok((record.kind, document))
}
/// Import with the deployed channel write policy (`CMAEG_GITHUB_ENABLED`).
pub async fn import(db: &Database, path: &std::path::Path) -> Result<u64, RecordError> {
    import_with(db, path, &ImportGate::from_environment()?).await
}

/// Import under an explicit channel write policy.
pub async fn import_with(
    db: &Database,
    path: &std::path::Path,
    gate: &ImportGate,
) -> Result<u64, RecordError> {
    let file = std::fs::File::open(path)
        .map_err(|_| RecordError::Rejected("record export is unreadable".into()))?;
    let mut reader = BufReader::new(file);
    let mut tracking = None;
    let mut tracking_checked = false;
    let options = CollectionOptions::builder()
        .read_concern(ReadConcern::majority())
        .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
        .build();
    let mut count = 0;
    loop {
        let mut line = String::new();
        let length = (&mut reader)
            .take(131073)
            .read_line(&mut line)
            .map_err(|_| RecordError::Rejected("record export is unreadable".into()))?;
        if length == 0 {
            return Ok(count);
        }
        if length > 131072 {
            return Err(RecordError::Rejected(
                "record export row exceeds limit".into(),
            ));
        }
        let (kind, desired) = decode(&line)?;
        if !tracking_checked {
            // An empty export or invalid first row admits no records. Check
            // durable writer authority once before any valid row can be stored.
            tracking = crate::worker_credentials::WorkerCredentials::from_environment(db).await?;
            tracking_checked = true;
        }
        let records = db.collection_with_options::<Document>(kind.collection(), options.clone());
        let id = desired
            .get_str("_id")
            .map_err(|_| RecordError::Rejected("invalid record ID".into()))?;
        let query = doc! {"_id":id};
        if kind == RecordKind::Channel {
            let version = desired
                .get_i64("version")
                .ok()
                .and_then(|version| u64::try_from(version).ok())
                .ok_or_else(|| RecordError::Rejected("invalid imported channel version".into()))?;
            let blob = desired
                .get_binary_generic("blob")
                .map_err(|_| RecordError::Rejected("invalid imported channel blob".into()))?;
            // A row already stored unchanged is retained; any other row is a
            // create and must pass the channel write gate before any write.
            let retained = records
                .find_one(query.clone())
                .await
                .map_err(|_| RecordError::Unavailable("cannot read existing record".into()))?
                .is_some_and(|stored| stored == desired);
            if !retained {
                gate.check_new_channel(id, version, blob)?;
            }
            if let Some(tracking) = &tracking {
                // Imported consumers can execute without a provider bind: record
                // every admitted worker hash before the Channel becomes visible.
                tracking.record_blob(id, version, blob).await?;
            }
        }
        if desired
            .get_datetime("expires_at")
            .is_ok_and(|expiry| *expiry <= DateTime::now())
        {
            // Expired legacy interactions are already logically absent.
            count += 1;
            continue;
        }
        // Restart-safe insert only. Never overwrite an independently mutated row.
        let result = records
            .update_one(query.clone(), doc! {"$setOnInsert":&desired})
            .upsert(true)
            .await;
        let actual = records
            .find_one(query)
            .await
            .map_err(|_| RecordError::Unavailable("cannot verify imported record".into()))?;
        if actual.as_ref() != Some(&desired) {
            return Err(RecordError::Rejected(
                "import conflicts with an existing record; no row was overwritten".into(),
            ));
        }
        result.map_err(|_| RecordError::Unavailable("cannot import record".into()))?;
        count += 1;
    }
}
