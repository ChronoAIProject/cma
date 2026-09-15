use serde::{Deserialize, Serialize};

use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Invocation {
    pub id: String,
    pub namespace: String,
    pub request_sha256: String,
    pub source_sha256: String,
    pub items: Vec<String>,
    pub created_at_ms: i64,
}

impl Invocation {
    pub fn document(&self) -> Result<Document, OutboxError> {
        if !self.id.starts_with("brti_")
            || self.id.len() != 69
            || self.namespace.len() != 64
            || self.request_sha256.len() != 64
            || self.source_sha256.len() != 64
            || self.items.is_empty()
            || self.items.len() > 1024
            || self.created_at_ms < 0
        {
            return Err(OutboxError::Corrupt);
        }
        Ok(
            doc! {"_id":&self.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "namespace":&self.namespace,"record":record_bson(self).map_err(|_|OutboxError::Corrupt)?},
        )
    }
    pub fn read(document: &Document) -> Result<Self, OutboxError> {
        let value: Self = read_record(document).map_err(|_| OutboxError::Corrupt)?;
        let expected = value.document()?;
        if ["_id", "namespace"]
            .iter()
            .any(|key| document.get(*key) != expected.get(*key))
        {
            return Err(OutboxError::Corrupt);
        }
        Ok(value)
    }
    pub fn require_request(
        &self,
        namespace: &str,
        request_sha256: &str,
    ) -> Result<(), OutboxError> {
        if self.namespace != namespace {
            return Err(OutboxError::SourceMismatch);
        }
        if self.request_sha256 != request_sha256 {
            return Err(OutboxError::RequestConflict);
        }
        Ok(())
    }
    pub fn receipt(&self, items: &[OutboxItem]) -> Result<ReplyToolReceipt, OutboxError> {
        if items.len() != self.items.len() {
            return Err(OutboxError::Corrupt);
        }
        let mut status = ReplyToolStatus::Sent;
        let mut failure = None;
        let mut duplicate_risk = false;
        for (index, item) in items.iter().enumerate() {
            item.validate()?;
            if item.id != self.items[index]
                || item.tool_invocation_id != self.id
                || item.source.fingerprint()? != self.source_sha256
            {
                return Err(OutboxError::Corrupt);
            }
            duplicate_risk |= item.duplicate_risk;
            match item.state {
                DeliveryState::Unknown { reason, .. } => {
                    status = ReplyToolStatus::Unknown;
                    failure = Some(reason);
                }
                DeliveryState::Rejected { reason } if status != ReplyToolStatus::Unknown => {
                    status = ReplyToolStatus::Rejected;
                    failure = Some(reason);
                }
                DeliveryState::Abandoned { .. }
                    if !matches!(status, ReplyToolStatus::Unknown | ReplyToolStatus::Rejected) =>
                {
                    status = ReplyToolStatus::Abandoned;
                }
                DeliveryState::Sent { .. } => {}
                _ if status == ReplyToolStatus::Sent => status = ReplyToolStatus::Queued,
                _ => {}
            }
        }
        Ok(ReplyToolReceipt {
            receipt_id: self.id.clone(),
            status,
            duplicate_risk,
            failure,
        })
    }
}

pub(super) fn item_document(item: &OutboxItem) -> Result<Document, OutboxError> {
    item.validate()?;
    let state = match item.state {
        DeliveryState::Pending => "pending",
        DeliveryState::Claimed { .. } => "claimed",
        DeliveryState::Sending { .. } => "sending",
        DeliveryState::RetryWait { .. } => "retry_wait",
        DeliveryState::Sent { .. } => "sent",
        DeliveryState::Rejected { .. } => "rejected",
        DeliveryState::Unknown { .. } => "unknown",
        DeliveryState::Abandoned { .. } => "abandoned",
    };
    let next_check_at_ms = match item.state {
        DeliveryState::Claimed { expires_at_ms, .. }
        | DeliveryState::Sending { expires_at_ms, .. } => expires_at_ms,
        DeliveryState::RetryWait { retry_at_ms, .. } => retry_at_ms,
        _ => item.updated_at_ms,
    };
    Ok(
        doc! {"_id":&item.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "invocation_id":&item.tool_invocation_id,"response_id":item.source.response_id().as_str(),
        "sequence":item.sequence as i64,"revision":item.revision as i64,"state":state,
        "updated_at_ms":item.updated_at_ms,"next_check_at_ms":next_check_at_ms,"record":record_bson(item).map_err(|_|OutboxError::Corrupt)?},
    )
}
pub(super) fn read_item(document: &Document) -> Result<OutboxItem, OutboxError> {
    let item: OutboxItem = read_record(document).map_err(|_| OutboxError::Corrupt)?;
    let expected = item_document(&item)?;
    if expected.iter().any(|(key, value)| {
        key != "record" && key != "next_check_at_ms" && document.get(key) != Some(value)
    }) {
        return Err(OutboxError::Corrupt);
    }
    if document.get_i64("next_check_at_ms").is_err() {
        return Err(OutboxError::Corrupt);
    }
    Ok(item)
}

impl MongoBridgeReplyToolStore {
    pub(super) async fn receipt(
        &self,
        invocation: &Invocation,
        session: &mut ClientSession,
    ) -> Result<ReplyToolReceipt, MutationError> {
        let mut items = Vec::with_capacity(invocation.items.len());
        for id in &invocation.items {
            let document = self
                .items
                .find_one(doc! {"_id":id})
                .session(&mut *session)
                .await?
                .ok_or(OutboxError::Corrupt)?;
            items.push(read_item(&document)?);
        }
        Ok(invocation.receipt(&items)?)
    }
    pub(super) async fn replace_item(
        &self,
        previous_revision: u64,
        item: &OutboxItem,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let changed = self
            .items
            .replace_one(
                doc! {"_id":&item.id,"revision":previous_revision as i64},
                item_document(item)?,
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(OutboxError::Conflict.into());
        }
        self.audits.insert_one(doc!{"_id":format!("{}:{}",item.id,item.revision),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "item_id":&item.id,"revision":item.revision as i64,"record":record_bson(item).map_err(|_|OutboxError::Corrupt)?})
            .session(session).await?;
        Ok(())
    }
}
