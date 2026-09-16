use futures_util::TryStreamExt;
use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::{
    AgentCommandAuditStore, CommandAuthorizationAudit, CommandLedgerError,
};
use layer2_core::sandbox::store::{Page, PageRequest};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection, IndexModel};

use super::super::sandbox_mongo::{
    read_record, record_bson, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::ApplyError;

fn document(audit: &CommandAuthorizationAudit) -> Result<Document, CommandLedgerError> {
    audit.validate()?;
    Ok(doc! {
        "_id": &audit.id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "command_id": audit.command_id.as_str(),
        "record": record_bson(audit).map_err(|_| CommandLedgerError::Corrupt)?,
    })
}

fn read(value: &Document) -> Result<CommandAuthorizationAudit, CommandLedgerError> {
    let audit: CommandAuthorizationAudit =
        read_record(value).map_err(|_| CommandLedgerError::Corrupt)?;
    let expected = document(&audit)?;
    if expected
        .iter()
        .any(|(key, field)| key != "record" && value.get(key) != Some(field))
    {
        return Err(CommandLedgerError::Corrupt);
    }
    Ok(audit)
}

pub(in crate::store) async fn append(
    store: &MongoSandboxStore,
    audit: &CommandAuthorizationAudit,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    store
        .command_authorization_audits
        .insert_one(document(audit)?)
        .session(session)
        .await?;
    Ok(())
}

pub(in crate::store) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! { "command_id": 1, "_id": 1 })
                .build(),
        )
        .await
        .map(|_| ())
        .map_err(|_| {
            layer2_core::StoreError::Unavailable("create command authorization audit index".into())
        })
}

#[async_trait::async_trait]
impl AgentCommandAuditStore for MongoSandboxStore {
    async fn command_authorization_audits(
        &self,
        command_id: &AgentCommandId,
        page: &PageRequest,
    ) -> Result<Page<CommandAuthorizationAudit>, CommandLedgerError> {
        PageRequest::new(page.limit, page.cursor.clone())
            .map_err(|_| CommandLedgerError::Invalid)?;
        let mut filter = doc! { "command_id": command_id.as_str() };
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let values: Vec<_> = self
            .command_authorization_audits
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let mut items: Vec<_> = values.iter().map(read).collect::<Result<_, _>>()?;
        if items.iter().any(|item| &item.command_id != command_id) {
            return Err(CommandLedgerError::Corrupt);
        }
        let next = if items.len() > usize::from(page.limit) {
            items.truncate(usize::from(page.limit));
            items.last().map(|item| item.id.clone())
        } else {
            None
        };
        Ok(Page { items, next })
    }
}
