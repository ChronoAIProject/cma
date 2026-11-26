use super::*;
use layer2_core::billing_authority::{AuthorityObservation, BillingMode};

pub(super) const STATE_ID: &str = "billing-authority-state-v1";
pub(super) const FLOOR_ID: &str = "billing-authority-floor-v1";
const PROTOCOL: &str = "billing-authority-v1";
const VERSION: i64 = 2;

pub(in crate::store) fn proposed_documents() -> [Document; 2] {
    [
        floor(),
        doc! {"_id":STATE_ID,"document_schema_version":VERSION,
        "protocol":PROTOCOL,"revision":1_i64,"epoch":0_i64,"mode":"legacy"},
    ]
}

fn floor() -> Document {
    doc! {"_id":FLOOR_ID,"document_schema_version":VERSION,
    "protocol":PROTOCOL,"minimum_reader":1_i64}
}

pub(super) fn recognized_envelopes() -> Document {
    doc! {"$and":[
        {"_id":{"$in":[STATE_ID,FLOOR_ID]}},
        {"document_schema_version":VERSION},
        {"document_schema_version":{"$type":"long"}},
        {"document_schema_version":{"$not":{"$type":"array"}}}
    ]}
}

pub(super) fn decode_pair(rows: Vec<Document>) -> Result<AuthorityObservation, StoreError> {
    if rows.is_empty() {
        return Ok(AuthorityObservation::Absent);
    }
    if rows.len() != 2 {
        return Err(malformed());
    }
    let floor_row = rows
        .iter()
        .find(|r| r.get_str("_id").ok() == Some(FLOOR_ID))
        .ok_or_else(malformed)?;
    if *floor_row != floor() {
        return Err(malformed());
    }
    let state = rows
        .iter()
        .find(|r| r.get_str("_id").ok() == Some(STATE_ID))
        .ok_or_else(malformed)?;
    if state.len() != 6
        || state.get_i64("document_schema_version").ok() != Some(VERSION)
        || state.get_str("protocol").ok() != Some(PROTOCOL)
        || !state.get_i64("revision").is_ok_and(|r| r > 0)
    {
        return Err(malformed());
    }
    let epoch = state
        .get_i64("epoch")
        .map_err(|_| malformed())?
        .try_into()
        .map_err(|_| malformed())?;
    let mode = match state.get_str("mode").map_err(|_| malformed())? {
        "legacy" => BillingMode::Legacy,
        "nyxid" => BillingMode::NyxId,
        _ => return Err(malformed()),
    };
    Ok(AuthorityObservation::Retained { epoch, mode })
}
