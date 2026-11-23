use super::*;
pub(super) fn event_document(
    event: &ErasureEvent,
    expiry: Option<&str>,
) -> Result<Document, AccountDeletionError> {
    let mut document = mongodb::bson::serialize_to_document(event)
        .map_err(|_| AccountDeletionError::Store("encode account deletion event".into()))?;
    document.insert("_id", format!("{}:{:020}", event.deletion_id, event.seq));
    document.insert(
        "document_schema_version",
        super::super::schema::DOCUMENT_SCHEMA_VERSION,
    );
    if let Some(expiry) = expiry {
        document.insert("expire_at", records::date(expiry)?);
    }
    Ok(document)
}
#[cfg(test)]
impl MongoAccountDeletionStore {
    pub(super) async fn events(
        &self,
        id: &AccountDeletionId,
    ) -> Result<Vec<ErasureEvent>, AccountDeletionError> {
        let mut cursor = self
            .events
            .find(doc! {"deletion_id":id.as_str()})
            .sort(doc! {"seq":1})
            .await
            .map_err(driver)?;
        let mut events = Vec::new();
        while let Some(mut row) = cursor.try_next().await.map_err(driver)? {
            row.remove("_id");
            row.remove("document_schema_version");
            row.remove("expire_at");
            events.push(mongodb::bson::deserialize_from_document(row).map_err(|_| {
                AccountDeletionError::Store("malformed account deletion event".into())
            })?);
        }
        Ok(events)
    }
}
