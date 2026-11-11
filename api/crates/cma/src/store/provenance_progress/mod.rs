//! Explicitly activated, bounded provenance observations and sweep receipts.
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{
    bson::{doc, Document},
    Collection, Database,
};

mod cli;
mod model;
mod step;
use super::durability::majority_collection;
pub(crate) use cli::{command, is_command};
use model::{Control, Phase, CONTROL};
pub(super) use model::{ACTIONS, COLLECTION};

type Result<T> = std::result::Result<T, SandboxStoreError>;
const IO: std::time::Duration = std::time::Duration::from_secs(5);

fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "durable provenance progress is unavailable; inspect status before retry".into(),
    )
}
fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("durable provenance progress is inconsistent or unsupported".into())
}
fn rows(database: &Database) -> Collection<Document> {
    majority_collection(database, COLLECTION)
}

async fn exists(database: &Database) -> Result<bool> {
    Ok(database
        .list_collection_names()
        .filter(doc! {"name": COLLECTION})
        .await
        .map_err(|_| unavailable())?
        .iter()
        .any(|name| name == COLLECTION))
}

async fn read(database: &Database) -> Result<Control> {
    Control::decode(
        rows(database)
            .find_one(doc! {"_id":CONTROL})
            .max_time(IO)
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(corrupt)?,
    )
}

async fn receipt(database: &Database, id: &str) -> Result<Control> {
    let mut row = rows(database)
        .find_one(doc! {"_id":id})
        .max_time(IO)
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(corrupt)?;
    row.insert("_id", CONTROL);
    let value = Control::decode(row)?;
    if value.phase != Phase::SweepFinished || value.last_receipt.as_deref() != Some(id) {
        return Err(corrupt());
    }
    Ok(value)
}

/// Invoked only by the explicit operator command; no runtime startup DDL.
pub(super) async fn initialize(database: &Database) -> Result<()> {
    super::sandbox_mongo::require_transaction_topology(database)
        .await
        .map_err(|_| unavailable())?;
    if !exists(database).await? {
        match database.create_collection(COLLECTION).await {
            Ok(()) => (),
            Err(error) if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(c) if c.code == 48) =>
                {}
            Err(_) => return Err(unavailable()),
        }
    }
    // No TTL, secondary index, validator, collation or capped collection may
    // silently change receipt retention/identity. Refuse unexpected structures.
    let spec = database
        .list_collections()
        .filter(doc! {"name":COLLECTION})
        .await
        .map_err(|_| unavailable())?
        .try_next()
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(corrupt)?;
    let options = mongodb::bson::serialize_to_document(&spec.options).map_err(|_| corrupt())?;
    if !options.is_empty() {
        return Err(corrupt());
    }
    let indexes: Vec<_> = rows(database)
        .list_indexes()
        .await
        .map_err(|_| unavailable())?
        .try_collect()
        .await
        .map_err(|_| unavailable())?;
    if indexes.len() != 1 || indexes[0].keys != doc! {"_id": 1} {
        return Err(corrupt());
    }
    if rows(database)
        .find_one(doc! {"_id":CONTROL})
        .await
        .map_err(|_| unavailable())?
        .is_none()
    {
        if rows(database)
            .find_one(doc! {})
            .await
            .map_err(|_| unavailable())?
            .is_some()
        {
            return Err(corrupt());
        }
        let mut initial = Control::initial();
        initial.updated_at = *database
            .run_command(doc! {"hello":1_i32})
            .await
            .map_err(|_| unavailable())?
            .get_datetime("localTime")
            .map_err(|_| unavailable())?;
        match rows(database).insert_one(initial.document()?).await {
            Ok(_) => (),
            Err(error) if super::sandbox_mongo::is_duplicate_key(&error) => (),
            Err(_) => return Err(unavailable()),
        }
    }
    let control = read(database).await?;
    if let Some(id) = &control.last_receipt {
        receipt(database, id).await?;
    }
    Ok(())
}

pub(super) async fn status(database: &Database) -> Result<serde_json::Value> {
    if !exists(database).await? {
        return Ok(
            serde_json::json!({"cohort":"agent_provenance","phase":"inactive", "global_completion":false}),
        );
    }
    let control = read(database).await?;
    let completed = match &control.last_receipt {
        Some(id) => Some(model::public_control(&receipt(database, id).await?)),
        None => None,
    };
    Ok(
        serde_json::json!({"current":model::public_control(&control), "last_sweep":completed,
        "global_completion":false, "exception_detail":"internal durable rows; aggregate counts only"}),
    )
}

#[cfg(test)]
pub(super) use step::interrupt_for_test;
