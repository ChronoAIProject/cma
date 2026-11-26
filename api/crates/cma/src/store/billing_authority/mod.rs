//! Read-only compatibility boundary; no authority preparation or activation writer.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::time::Duration;

pub(in crate::store) mod format_v1;

pub(in crate::store) fn exclude_known_envelopes(unsupported: Document) -> Document {
    doc! {"$and":[unsupported,{"$nor":[format_v1::recognized_envelopes()]}]}
}

pub(in crate::store) async fn require_legacy_compatible(
    database: &Database,
) -> Result<(), StoreError> {
    let mut session = database
        .client()
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let mut cursor = database
        .collection::<Document>(super::settings::SETTINGS)
        .find(doc! {"_id":{"$in":[format_v1::STATE_ID,format_v1::FLOOR_ID]}})
        .limit(3)
        .max_time(Duration::from_secs(2))
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    let mut rows = Vec::new();
    while let Some(row) = cursor
        .next(&mut session)
        .await
        .transpose()
        .map_err(unavailable)?
    {
        rows.push(row);
    }
    format_v1::decode_pair(rows)?
        .require_legacy_compatible()
        .map_err(|_| {
            StoreError::Malformed("billing authority execution protocol is unsupported".into())
        })
}

fn malformed() -> StoreError {
    StoreError::Malformed("billing authority records are incomplete or unsupported".into())
}

fn unavailable(_: impl std::fmt::Display) -> StoreError {
    StoreError::Unavailable("billing authority compatibility read unavailable".into())
}

#[cfg(test)]
mod mongo_tests;
#[cfg(test)]
mod tests;
