use super::{invalid, CollectionReference};
use crate::{failure::Result, ledger_reclamation::CollectionEvidence};
use rusqlite::Connection;

pub(super) fn from_evidence(
    evidence: &CollectionEvidence,
    plan_sha256: String,
) -> Result<CollectionReference> {
    Ok(CollectionReference {
        format: layer3_adapter::LEDGER_COLLECTION_ARTIFACT_FORMAT.into(),
        archive_id: evidence.binding().archive_id.clone(),
        archive_version: 3,
        operation_id: evidence.operation_id().into(),
        group_key: evidence.group_key().into(),
        binding_sha256: evidence.binding().binding_sha256().map_err(|_| invalid())?,
        plan_sha256,
        artifacts_sha256: evidence.fingerprint(),
        artifact_lengths: evidence.originals().map(|bytes| bytes.len() as u64),
    })
}

pub(super) fn validate(reference: &CollectionReference) -> Result<()> {
    if !reference.validate() {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn load(db: &Connection, operation: &str) -> Result<CollectionReference> {
    let archive_id: String = db
        .query_row(
            "SELECT archive_id FROM archive_metadata WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| invalid())?;
    let archive_version: u8 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|_| invalid())?;
    let result=db.query_row(
        "SELECT operation_id,group_key,binding_sha256,plan_sha256,artifacts_sha256,candidate_bytes,request_bytes,observation_bytes
        FROM collection_evidence WHERE operation_id=?", [operation], |r| Ok(CollectionReference {
            format:layer3_adapter::LEDGER_COLLECTION_ARTIFACT_FORMAT.into(),archive_id,archive_version,
            operation_id:r.get(0)?,group_key:r.get(1)?,binding_sha256:r.get(2)?,plan_sha256:r.get(3)?,
            artifacts_sha256:r.get(4)?,artifact_lengths:[
                u64::from(r.get::<_,u32>(5)?),
                u64::from(r.get::<_,u32>(6)?),
                u64::from(r.get::<_,u32>(7)?),
            ],
        })).map_err(|_|invalid())?;
    validate(&result)?;
    Ok(result)
}
