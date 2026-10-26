//! Explicit offline transfer; the gateway never receives legacy DB credentials.
use super::{RecordError, RecordKind};
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
pub async fn import(db: &Database, path: &std::path::Path) -> Result<u64, RecordError> {
    let file = std::fs::File::open(path)
        .map_err(|_| RecordError::Rejected("record export is unreadable".into()))?;
    let mut reader = BufReader::new(file);
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
        if desired
            .get_datetime("expires_at")
            .is_ok_and(|expiry| *expiry <= DateTime::now())
        {
            // Expired legacy interactions are already logically absent.
            count += 1;
            continue;
        }
        let records = db.collection_with_options::<Document>(kind.collection(), options.clone());
        let query = doc! {"_id":desired.get_str("_id").map_err(|_| RecordError::Rejected("invalid record ID".into()))?};
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
