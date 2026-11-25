use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};
use mongodb::bson::{doc, Document};

use super::{super::agent_log_segments as segments, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Access {
    Audit,
    ActiveProof,
    GateRead,
    GateWrite,
}

pub(super) fn require_inventory(access: Access) -> Result<()> {
    require_inventory_in(access, segments::COLLECTIONS)
}

pub(super) fn require_inventory_in(access: Access, inventory: &[&str]) -> Result<()> {
    if matches!(access, Access::GateRead | Access::GateWrite)
        && [
            segments::writer_gate::COLLECTION,
            segments::writer_admission::COLLECTION,
        ]
        .iter()
        .any(|name| !inventory.contains(name))
    {
        return Err("gate schema is not registered in the verified migration inventory");
    }
    Ok(())
}

pub(super) fn role(database: &str, access: Access) -> Document {
    let mut grants = Vec::new();
    for name in segments::COLLECTIONS {
        let mut actions = vec!["listIndexes"];
        if access == Access::Audit
            || (access == Access::ActiveProof
                && [
                    segments::HEADS,
                    segments::SEGMENTS,
                    segments::AUDITS,
                    segments::PROJECTIONS,
                ]
                .contains(name))
        {
            actions.push("find");
            if *name == segments::HEADS {
                actions.push("update");
            } else if access == Access::Audit
                && [segments::PROJECTIONS, segments::EVIDENCE, segments::AUDITS].contains(name)
            {
                actions.extend(["insert", "update"]);
            }
        }
        grants.push(doc! {"resource":{"db":database,"collection":*name},"actions":actions});
    }
    if matches!(access, Access::Audit | Access::ActiveProof) {
        grants.push(
            doc! {"resource":{"db":database,"collection":"agents"},"actions":["find","update"]},
        );
        grants.push(doc! {"resource":{"db":database,"collection":"agent_log"},"actions":["find","listIndexes"]});
        if access == Access::ActiveProof {
            grants.push(doc! {"resource":{"db":database,"collection":segments::active_proof::COLLECTION},"actions":["find","insert","update","listIndexes"]});
        }
    } else {
        let actions = if access == Access::GateWrite {
            vec!["find", "insert", "update"]
        } else {
            vec!["find"]
        };
        grants.push(doc! {"resource":{"db":database,"collection":segments::writer_gate::COLLECTION},"actions":actions});
    }
    grants.push(doc! {"resource":{"db":database,"collection":""},"actions":["listCollections"]});
    doc! {"createRole":"cma_am_log_operator","privileges":grants,"roles":[]}
}

pub(super) fn verify(
    status: &Document,
    database: &str,
    mode: MongoAuthorityMode,
    access: Access,
) -> Result<()> {
    let required = role(database, access);
    verify_shared_schema(status, database, &required)
        .map_err(|_| "Agent log operator authority is insufficient")?;
    if mode == MongoAuthorityMode::SharedSchema {
        return Ok(());
    }
    let info = status
        .get_document("authInfo")
        .map_err(|_| "operator authority malformed")?;
    let expected = required
        .get_array("privileges")
        .map_err(|_| "operator role malformed")?;
    for grant in info
        .get_array("authenticatedUserPrivileges")
        .map_err(|_| "operator grants malformed")?
    {
        let grant = grant.as_document().ok_or("operator grant malformed")?;
        let resource = grant
            .get_document("resource")
            .map_err(|_| "operator resource malformed")?;
        for action in grant
            .get_array("actions")
            .map_err(|_| "operator actions malformed")?
        {
            if !expected
                .iter()
                .filter_map(|value| value.as_document())
                .any(|value| {
                    value.get_document("resource").ok() == Some(resource)
                        && value
                            .get_array("actions")
                            .is_ok_and(|actions| actions.contains(action))
                })
            {
                return Err("operator credential has unreviewed authority");
            }
        }
    }
    Ok(())
}
