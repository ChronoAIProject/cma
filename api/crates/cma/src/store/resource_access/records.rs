use layer2_core::resource_access::{
    ResourceAccessStoreError, ResourceOwner, ResourcePolicyRecord, ResourceSelector, ResourceShare,
};
use layer2_core::SandboxId;
use mongodb::bson::{doc, Document};

use super::super::sandbox_mongo::{read_record, record_bson, signed, DOCUMENT_SCHEMA_VERSION};

pub(in crate::store) fn policy_document(
    record: &ResourcePolicyRecord,
) -> Result<Document, ResourceAccessStoreError> {
    record.validate()?;
    let (kind, subject) = match &record.owner {
        ResourceOwner::Person { subject } => ("person", subject),
        ResourceOwner::Organization { subject } => ("organization", subject),
    };
    Ok(
        doc! {"_id":record.sandbox_id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,"owner_kind":kind,"owner_subject":subject.as_str(),"revision":signed(record.revision,"resource policy revision").map_err(|_|ResourceAccessStoreError::Corrupt)?,"record":record_bson(record).map_err(|_|ResourceAccessStoreError::Corrupt)?},
    )
}
pub(super) fn read_policy(
    document: &Document,
) -> Result<ResourcePolicyRecord, ResourceAccessStoreError> {
    let record = read_record(document).map_err(|_| ResourceAccessStoreError::Corrupt)?;
    check_projection(document, &policy_document(&record)?)?;
    Ok(record)
}
pub(super) fn share_document(
    sandbox_id: &SandboxId,
    record: &ResourceShare,
) -> Result<Document, ResourceAccessStoreError> {
    if !record.validate() {
        return Err(ResourceAccessStoreError::Corrupt);
    }
    let (kind, id) = match &record.selector {
        ResourceSelector::Agent { agent_id } => ("agent", agent_id.as_str()),
        ResourceSelector::Sandbox { sandbox_id } => ("sandbox", sandbox_id.as_str()),
    };
    let mut document = doc! {"_id":&record.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,"sandbox_id":sandbox_id.as_str(),"selector_kind":kind,"selector_id":id,"target_org_id":record.target_org_id.as_str(),"revoked":record.revoked_at.is_some(),"revision":signed(record.revision,"resource share revision").map_err(|_|ResourceAccessStoreError::Corrupt)?,"record":record_bson(record).map_err(|_|ResourceAccessStoreError::Corrupt)?};
    // Additive projection only for new link records; old documents remain valid
    // without a backfill and continue using their existing organization index.
    if let Some(link) = &record.link {
        document.insert(
            "accepted_subjects",
            link.accepted_subjects
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
        );
    }
    Ok(document)
}
pub(super) fn read_share(
    sandbox_id: &SandboxId,
    document: &Document,
) -> Result<ResourceShare, ResourceAccessStoreError> {
    let record = read_record(document).map_err(|_| ResourceAccessStoreError::Corrupt)?;
    check_projection(document, &share_document(sandbox_id, &record)?)?;
    Ok(record)
}
fn check_projection(
    document: &Document,
    expected: &Document,
) -> Result<(), ResourceAccessStoreError> {
    if expected
        .iter()
        .any(|(key, value)| key != "record" && document.get(key) != Some(value))
    {
        return Err(ResourceAccessStoreError::Corrupt);
    }
    Ok(())
}

pub(super) fn receipt_document(
    receipt: &layer2_core::resource_access::ShareMutationReceipt,
) -> Result<Document, ResourceAccessStoreError> {
    receipt.validate()?;
    Ok(
        doc! {"_id":&receipt.operation_id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,"share_id":&receipt.share.id,"sandbox_id":receipt.sandbox_id.as_str(),"record":record_bson(receipt).map_err(|_|ResourceAccessStoreError::Corrupt)?},
    )
}
pub(super) fn read_receipt(
    document: &Document,
) -> Result<layer2_core::resource_access::ShareMutationReceipt, ResourceAccessStoreError> {
    let record = read_record(document).map_err(|_| ResourceAccessStoreError::Corrupt)?;
    check_projection(document, &receipt_document(&record)?)?;
    Ok(record)
}
