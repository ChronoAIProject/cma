//! MongoDB topology and snapshot-read compatibility.
use bson::{Bson, Document};

const SNAPSHOT_READ_MIN_WIRE_VERSION: i64 = 13;

/// Validate the topology and wire version from a MongoDB hello reply.
pub fn validate_transaction_server(reply: &Document) -> Result<(), String> {
    if reply.get_str("setName").is_err()
        && !matches!(reply.get_str("msg"), Ok(message) if message == "isdbgrid")
    {
        return Err("sandbox-model transactions require a replica set or mongos".into());
    }
    let max_wire_version = match reply.get("maxWireVersion") {
        Some(Bson::Int32(version)) => i64::from(*version),
        Some(Bson::Int64(version)) => *version,
        _ => {
            return Err("MongoDB did not report a usable maxWireVersion".into());
        }
    };
    if max_wire_version < SNAPSHOT_READ_MIN_WIRE_VERSION {
        return Err(format!(
            "snapshot reads require MongoDB 5.0 or later (maxWireVersion >= {SNAPSHOT_READ_MIN_WIRE_VERSION}); server reported {max_wire_version}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bson::doc;

    #[test]
    fn transaction_server_requires_topology_and_snapshot_support() {
        assert!(
            validate_transaction_server(&doc! { "setName": "rs0", "maxWireVersion": 13_i32 })
                .is_ok()
        );
        assert!(
            validate_transaction_server(&doc! { "msg": "isdbgrid", "maxWireVersion": 25_i64 })
                .is_ok()
        );

        let standalone =
            validate_transaction_server(&doc! { "maxWireVersion": 25_i32 }).unwrap_err();
        assert!(standalone.contains("replica set or mongos"));
        let old = validate_transaction_server(&doc! { "setName": "rs0", "maxWireVersion": 12_i32 })
            .unwrap_err();
        assert!(old.contains("MongoDB 5.0 or later"));
        let missing = validate_transaction_server(&doc! { "setName": "rs0" }).unwrap_err();
        assert!(missing.contains("maxWireVersion"));
    }
}
