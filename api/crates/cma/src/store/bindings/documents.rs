use super::*;
use crate::store::sandbox_mongo::{read_record, record_bson, signed};

pub(super) fn binding_document(value: &ProviderBinding) -> Result<Document, SandboxStoreError> {
    value.validate().map_err(SandboxStoreError::Corrupt)?;
    Ok(
        doc! {"_id": &value.binding_id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": integer(value.revision)?, "generation": integer(value.generation)?,
        "pending": binding_has_work(value), "record": record_bson(value).map_err(SandboxStoreError::Corrupt)?},
    )
}

pub(super) fn read_binding(doc: Document) -> Result<ProviderBinding, SandboxStoreError> {
    let value: ProviderBinding = read_record(&doc).map_err(SandboxStoreError::Corrupt)?;
    value.validate().map_err(SandboxStoreError::Corrupt)?;
    if doc.get_str("_id").ok() != Some(value.binding_id.as_str())
        || doc.get_i64("revision").ok() != Some(integer(value.revision)?)
        || doc.get_i64("generation").ok() != Some(integer(value.generation)?)
        || doc.get_bool("pending").ok() != Some(binding_has_work(&value))
    {
        return Err(SandboxStoreError::Corrupt(
            "binding index disagrees with its record".into(),
        ));
    }
    Ok(value)
}

pub(super) fn conversation_document(
    value: &BindingConversation,
) -> Result<Document, SandboxStoreError> {
    value.validate().map_err(SandboxStoreError::Corrupt)?;
    Ok(
        doc! {"_id": conversation_storage_key(value), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "binding_id": &value.binding_id, "generation": integer(value.generation)?, "conversation_id": &value.conversation_id,
        "revision": integer(value.revision)?, "pending": conversation_has_work(value),
        "record": record_bson(value).map_err(SandboxStoreError::Corrupt)?},
    )
}

pub(super) fn read_conversation(doc: Document) -> Result<BindingConversation, SandboxStoreError> {
    let value: BindingConversation = read_record(&doc).map_err(SandboxStoreError::Corrupt)?;
    value.validate().map_err(SandboxStoreError::Corrupt)?;
    if doc.get_str("_id").ok() != Some(conversation_storage_key(&value).as_str())
        || doc.get_str("binding_id").ok() != Some(value.binding_id.as_str())
        || doc.get_str("conversation_id").ok() != Some(value.conversation_id.as_str())
        || doc.get_i64("revision").ok() != Some(integer(value.revision)?)
        || doc.get_i64("generation").ok() != Some(integer(value.generation)?)
        || doc.get_bool("pending").ok() != Some(conversation_has_work(&value))
    {
        return Err(SandboxStoreError::Corrupt(
            "conversation index disagrees with its record".into(),
        ));
    }
    Ok(value)
}

pub(super) fn integer(value: u64) -> Result<i64, SandboxStoreError> {
    signed(value, "binding version").map_err(SandboxStoreError::Corrupt)
}
