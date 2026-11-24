//! Bind every observed collection generation; a server process alone is not a DB incarnation.
use std::collections::{BTreeMap, BTreeSet};

use mongodb::bson::{spec::BinarySubtype, Binary};
use serde::Deserialize;

use super::{records::*, *};
use crate::store::sandbox_mongo::{
    AGENTS, AGENT_COMMANDS, AGENT_COMMAND_ORDERS, AGENT_LOG, SANDBOXES, SANDBOX_CONTROLS,
    SANDBOX_SESSIONS, WORKSPACES,
};

pub(super) const COLLECTIONS: [&str; 9] = [
    SANDBOXES,
    WORKSPACES,
    AGENTS,
    AGENT_COMMAND_ORDERS,
    AGENT_COMMANDS,
    AGENT_LOG,
    crate::store::agent_log_segments::HEADS,
    SANDBOX_SESSIONS,
    SANDBOX_CONTROLS,
];
const MAX_METADATA_BYTES: usize = 32_768;

// listCollections has no arbitrary server projection. Select only the exact
// names, bound raw metadata bytes, then deserialize just the identity fields;
// validators/options/index definitions never enter the retained proof.
#[derive(Clone, Deserialize)]
struct CollectionIdentity {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    info: CollectionInfo,
}

#[derive(Clone, Deserialize)]
struct CollectionInfo {
    #[serde(rename = "readOnly")]
    read_only: bool,
    uuid: Option<Binary>,
}

pub(super) async fn incarnation(store: &MongoSandboxStore) -> Result<String, ApplyError> {
    let hello = store
        .client
        .database("admin")
        .run_command(doc! {"hello":1})
        .await
        .map_err(ApplyError::Driver)?;
    let process = get(&hello, "topologyVersion.processId")
        .and_then(mongodb::bson::Bson::as_object_id)
        .ok_or_else(invalid)?;
    let database = store.client.database(&store.sandboxes.namespace().db);
    let mut cursor = database
        .list_collections()
        .filter(doc! {"name":{"$in":COLLECTIONS.to_vec()}})
        .batch_size((COLLECTIONS.len() + 1) as u32)
        .await
        .map_err(ApplyError::Driver)?
        .with_type::<CollectionIdentity>();
    let mut rows = Vec::with_capacity(COLLECTIONS.len());
    while cursor.advance().await.map_err(ApplyError::Driver)? {
        if rows.len() == COLLECTIONS.len() || cursor.current().as_bytes().len() > MAX_METADATA_BYTES
        {
            return Err(invalid());
        }
        rows.push(cursor.deserialize_current().map_err(|_| invalid())?);
    }
    let inventory = identities(rows)?;
    layer2_core::profiles::content_digest(&(
        text(&hello, "setName")?,
        process.to_hex(),
        database.name(),
        inventory,
    ))
    .map_err(|_| invalid())
}

fn identities(rows: Vec<CollectionIdentity>) -> Result<BTreeMap<String, Vec<u8>>, ApplyError> {
    if rows.len() != COLLECTIONS.len() {
        return Err(invalid());
    }
    let mut inventory = BTreeMap::new();
    let mut uuids = BTreeSet::new();
    for row in rows {
        let uuid = row.info.uuid.ok_or_else(invalid)?;
        if !COLLECTIONS.contains(&row.name.as_str())
            || row.kind != "collection"
            || row.info.read_only
            || uuid.subtype != BinarySubtype::Uuid
            || uuid.bytes.len() != 16
            || !uuids.insert(uuid.bytes.clone())
            || inventory.insert(row.name, uuid.bytes).is_some()
        {
            return Err(invalid());
        }
    }
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<CollectionIdentity> {
        COLLECTIONS
            .iter()
            .enumerate()
            .map(|(index, name)| CollectionIdentity {
                name: (*name).into(),
                kind: "collection".into(),
                info: CollectionInfo {
                    read_only: false,
                    uuid: Some(Binary {
                        subtype: BinarySubtype::Uuid,
                        bytes: vec![index as u8; 16],
                    }),
                },
            })
            .collect()
    }

    #[test]
    fn complete_uuid_inventory_is_order_independent_and_generation_sensitive() {
        let expected = identities(rows()).unwrap();
        let mut reversed = rows();
        reversed.reverse();
        assert_eq!(identities(reversed).unwrap(), expected);
        let mut changed = rows();
        changed[0].info.uuid.as_mut().unwrap().bytes = vec![100; 16];
        assert_ne!(identities(changed).unwrap(), expected);
    }

    #[test]
    fn missing_duplicate_unknown_or_unsupported_collection_identity_refuses() {
        for case in 0..9 {
            let mut rows = rows();
            match case {
                0 => {
                    rows.pop();
                }
                1 => rows.push(rows[0].clone()),
                2 => rows[1].name = rows[0].name.clone(),
                3 => rows[1].info.uuid = rows[0].info.uuid.clone(),
                4 => rows[0].info.uuid = None,
                5 => rows[0].info.uuid.as_mut().unwrap().subtype = BinarySubtype::UuidOld,
                6 => rows[0].info.uuid.as_mut().unwrap().bytes.truncate(15),
                7 => rows[0].kind = "view".into(),
                8 => rows[0].name = "unrequested".into(),
                _ => unreachable!(),
            }
            assert!(identities(rows).is_err(), "case {case}");
        }
        let mut read_only = rows();
        read_only[0].info.read_only = true;
        assert!(identities(read_only).is_err());
    }
}
