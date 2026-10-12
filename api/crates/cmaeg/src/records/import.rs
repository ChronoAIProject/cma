//! Explicit offline transfer; the gateway never receives legacy DB credentials.
use super::RecordError;
use base64::{engine::general_purpose::STANDARD, Engine};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Document},
    options::{Acknowledgment, CollectionOptions, ReadConcern, WriteConcern},
    Database,
};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRecord {
    record_id: String,
    owner: String,
    version: String,
    blob: String,
}
fn decode(line: &str) -> Result<Document, RecordError> {
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
    Ok(
        doc! {"_id":record.record_id,"owner":record.owner,"version":version,"schema_version":1_i32,"blob":Binary{subtype:BinarySubtype::Generic,bytes}},
    )
}
pub async fn import(db: &Database, path: &std::path::Path) -> Result<u64, RecordError> {
    let file = std::fs::File::open(path)
        .map_err(|_| RecordError::Rejected("record export is unreadable".into()))?;
    let mut reader = BufReader::new(file);
    let options = CollectionOptions::builder()
        .read_concern(ReadConcern::majority())
        .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
        .build();
    let records = db.collection_with_options::<Document>(super::mongo::RECORDS, options);
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
        let desired = decode(&line)?;
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
