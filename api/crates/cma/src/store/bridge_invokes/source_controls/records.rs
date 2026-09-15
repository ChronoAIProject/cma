use super::*;

pub(super) fn policy_document(policy: &SourceControlPolicy) -> Result<Document, BridgeInvokeError> {
    policy.validate()?;
    Ok(
        doc! {"_id":policy.view.grant_id.as_str(),"revision":policy.view.revision as i64,
        "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "record":record_bson(policy).map_err(|_|BridgeInvokeError::Corrupt)?},
    )
}
pub(super) fn read_policy(document: &Document) -> Result<SourceControlPolicy, BridgeInvokeError> {
    let policy: SourceControlPolicy =
        read_record(document).map_err(|_| BridgeInvokeError::Corrupt)?;
    let expected = policy_document(&policy)?;
    if ["_id", "revision"]
        .iter()
        .any(|key| document.get(*key) != expected.get(*key))
    {
        return Err(BridgeInvokeError::Corrupt);
    }
    Ok(policy)
}
pub(super) fn receipt_document(
    receipt: &SourceControlReceipt,
) -> Result<Document, BridgeInvokeError> {
    receipt.validate()?;
    Ok(
        doc! {"_id":&receipt.id,"grant_id":receipt.result.grant_id.as_str(),
        "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "record":record_bson(receipt).map_err(|_|BridgeInvokeError::Corrupt)?},
    )
}
pub(super) fn read_receipt(document: &Document) -> Result<SourceControlReceipt, BridgeInvokeError> {
    let receipt: SourceControlReceipt =
        read_record(document).map_err(|_| BridgeInvokeError::Corrupt)?;
    let expected = receipt_document(&receipt)?;
    if ["_id", "grant_id"]
        .iter()
        .any(|key| document.get(*key) != expected.get(*key))
    {
        return Err(BridgeInvokeError::Corrupt);
    }
    Ok(receipt)
}
pub(super) fn audit_document(policy: &SourceControlPolicy) -> Result<Document, BridgeInvokeError> {
    let mut document = policy_document(policy)?;
    document.insert(
        "_id",
        format!("{}:{}", policy.view.grant_id.as_str(), policy.view.revision),
    );
    document.insert("grant_id", policy.view.grant_id.as_str());
    Ok(document)
}
pub(super) fn read_audit(document: &Document) -> Result<SourceControlPolicy, BridgeInvokeError> {
    let policy: SourceControlPolicy =
        read_record(document).map_err(|_| BridgeInvokeError::Corrupt)?;
    if *document != audit_document(&policy)? {
        return Err(BridgeInvokeError::Corrupt);
    }
    Ok(policy)
}
