//! Replica-set and server-version requirements shared by consistent readers.
use super::unavailable;
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};

const SNAPSHOT_READ_MIN_WIRE_VERSION: i64 = 13;

pub(in crate::store) async fn require_transaction_topology(
    database: &Database,
) -> Result<(), StoreError> {
    let reply = database
        .run_command(doc! { "hello": 1_i32 })
        .await
        .map_err(|error| unavailable("inspect Mongo transaction topology", error))?;
    validate_transaction_server(&reply)
}

pub(super) fn validate_transaction_server(reply: &Document) -> Result<(), StoreError> {
    if reply.get_str("setName").is_err()
        && !matches!(reply.get_str("msg"), Ok(message) if message == "isdbgrid")
    {
        return Err(StoreError::Unavailable(
            "sandbox-model transactions require a replica set or mongos".into(),
        ));
    }
    let max_wire_version = match reply.get("maxWireVersion") {
        Some(Bson::Int32(version)) => i64::from(*version),
        Some(Bson::Int64(version)) => *version,
        _ => {
            return Err(StoreError::Unavailable(
                "MongoDB did not report a usable maxWireVersion".into(),
            ));
        }
    };
    if max_wire_version < SNAPSHOT_READ_MIN_WIRE_VERSION {
        return Err(StoreError::Unavailable(format!(
            "snapshot reads require MongoDB 5.0 or later (maxWireVersion >= {SNAPSHOT_READ_MIN_WIRE_VERSION}); server reported {max_wire_version}"
        )));
    }
    Ok(())
}
