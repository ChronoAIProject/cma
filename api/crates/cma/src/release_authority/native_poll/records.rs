use mongodb::{
    bson::{self, doc, Document},
    ClientSession,
};
use serde::{Deserialize, Serialize};

use super::{
    super::{
        store::{document, unavailable},
        validation, *,
    },
    model::*,
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    #[serde(rename = "_id")]
    key: String,
    kind: String,
    schema_version: i64,
    writer: WriterIdentity,
    lane: NativePollLane,
    sequence: i64,
    receipt_key: String,
    receipt_sha256: String,
}

pub(super) fn next_sequence(
    tip: Option<&NativePollReceipt>,
    observed: Option<&NativePollReceipt>,
) -> Result<i64> {
    match (tip, observed) {
        (None, None) => Ok(1),
        (Some(tip), Some(observed)) => {
            tip.0.validate()?;
            observed.0.validate()?;
            if tip.0.command != observed.0.command || tip.sha256() != observed.sha256() {
                return Err(AuthorityError::Conflict);
            }
            validation::increment(tip.sequence())
        }
        _ => Err(AuthorityError::Conflict),
    }
}

fn decode_receipt(row: Document) -> Result<NativePollReceipt> {
    let receipt: ReceiptRecord =
        bson::deserialize_from_document(row).map_err(|_| AuthorityError::Invalid)?;
    receipt.validate()?;
    Ok(NativePollReceipt(receipt))
}

pub(super) async fn receipt(
    store: &AuthorityStore,
    command: &CommandRecord,
    session: &mut ClientSession,
) -> Result<Option<NativePollReceipt>> {
    let row = store
        .store_commands
        .find_one(doc! {"_id": &command.key})
        .session(session)
        .await
        .map_err(unavailable)?;
    row.map(|row| {
        let receipt = decode_receipt(row)?;
        if &receipt.0.command != command {
            return Err(AuthorityError::Conflict);
        }
        Ok(receipt)
    })
    .transpose()
}

/// Cursor and immutable receipt are read in one snapshot. A torn or foreign
/// row is corruption, never an empty initial lane.
pub(super) async fn tip(
    store: &AuthorityStore,
    writer: &WriterIdentity,
    lane: NativePollLane,
    session: &mut ClientSession,
) -> Result<Option<NativePollReceipt>> {
    let key = cursor_key(writer, lane)?;
    let Some(row) = store
        .store_commands
        .find_one(doc! {"_id": &key})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(None);
    };
    let cursor: Cursor =
        bson::deserialize_from_document(row).map_err(|_| AuthorityError::Invalid)?;
    if cursor.key != key
        || cursor.kind != "native_target_poll_cursor"
        || cursor.schema_version != 1
        || &cursor.writer != writer
        || cursor.lane != lane
        || cursor.sequence < 1
        || cursor.receipt_key != command_key(writer, lane, cursor.sequence)?
    {
        return Err(AuthorityError::Invalid);
    }
    let row = store
        .store_commands
        .find_one(doc! {"_id": &cursor.receipt_key})
        .session(session)
        .await
        .map_err(unavailable)?
        .ok_or(AuthorityError::Invalid)?;
    let receipt = decode_receipt(row)?;
    if &receipt.0.command.writer != writer
        || receipt.lane() != lane
        || receipt.sequence() != cursor.sequence
        || receipt.sha256() != cursor.receipt_sha256
    {
        return Err(AuthorityError::Invalid);
    }
    Ok(Some(receipt))
}

pub(super) async fn append(
    store: &AuthorityStore,
    previous: Option<&NativePollReceipt>,
    receipt: &NativePollReceipt,
    session: &mut ClientSession,
) -> Result<()> {
    receipt.0.validate()?;
    let command = &receipt.0.command;
    let cursor = Cursor {
        key: cursor_key(&command.writer, command.lane)?,
        kind: "native_target_poll_cursor".into(),
        schema_version: 1,
        writer: command.writer.clone(),
        lane: command.lane,
        sequence: command.sequence,
        receipt_key: command.key.clone(),
        receipt_sha256: receipt.sha256().into(),
    };
    // Receipts are insert-only. No replacement, delete, TTL or mutable outcome.
    store
        .store_commands
        .insert_one(document(&receipt.0)?)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    match previous {
        None => {
            store
                .store_commands
                .insert_one(document(&cursor)?)
                .session(session)
                .await
                .map_err(unavailable)?;
        }
        Some(previous) => {
            let result = store.store_commands.replace_one(
                doc! {"_id": &cursor.key, "kind": "native_target_poll_cursor", "schema_version": 1_i64,
                    "sequence": previous.sequence(), "receipt_sha256": previous.sha256()},
                document(&cursor)?,
            ).session(session).await.map_err(unavailable)?;
            if result.matched_count != 1 {
                return Err(AuthorityError::Conflict);
            }
        }
    }
    Ok(())
}
