//! Explicit Channel reply invocations and ordered delivery items. Retained
//! receipt identity survives turn completion and never implies platform send.

use std::sync::Arc;

use layer2_core::bridge::outbox::*;
use layer2_core::bridge::{BridgeConnectionValidator, BridgeRecord, BridgeValidation};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, IndexModel,
};

use crate::store::{
    durability::majority_collection,
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, read_record, record_bson,
        start_transaction, CommitOutcome, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
    },
};

use super::MongoBridgeInvokeStore;

mod enqueue;
mod management;
mod rate;
mod records;
mod scanning;
mod snapshot;
#[cfg(test)]
mod tests;
mod worker;

pub(crate) struct MongoBridgeReplyToolStore {
    invokes: Arc<MongoBridgeInvokeStore>,
    validator: Arc<dyn BridgeConnectionValidator>,
    allocations: Collection<Document>,
    items: Collection<Document>,
    invocations: Collection<Document>,
    heads: Collection<Document>,
    audits: Collection<Document>,
    scans: Collection<Document>,
    send_rates: Collection<Document>,
    recoveries: Collection<Document>,
    wake: tokio::sync::Notify,
}

impl MongoBridgeReplyToolStore {
    pub(crate) async fn new(
        invokes: Arc<MongoBridgeInvokeStore>,
        validator: Arc<dyn BridgeConnectionValidator>,
    ) -> Result<Self, layer2_core::StoreError> {
        let database = invokes
            .bridges
            .client
            .database(&invokes.bridges.definitions.namespace().db);
        crate::store::schema::initialize_collections(
            &database,
            &[
                "bridge_outbox_items",
                "bridge_outbox_invocations",
                "bridge_outbox_heads",
                "bridge_outbox_audits",
                "bridge_outbox_scans",
                "bridge_outbox_send_rates",
                "bridge_outbox_recoveries",
            ],
        )
        .await?;
        let store = Self {
            invokes,
            validator,
            allocations: majority_collection(&database, "bridge_allocations"),
            items: majority_collection(&database, "bridge_outbox_items"),
            invocations: majority_collection(&database, "bridge_outbox_invocations"),
            heads: majority_collection(&database, "bridge_outbox_heads"),
            audits: majority_collection(&database, "bridge_outbox_audits"),
            scans: majority_collection(&database, "bridge_outbox_scans"),
            send_rates: majority_collection(&database, "bridge_outbox_send_rates"),
            recoveries: majority_collection(&database, "bridge_outbox_recoveries"),
            wake: tokio::sync::Notify::new(),
        };
        store
            .items
            .create_indexes([
                IndexModel::builder()
                    .keys(doc! {"record.source.pin.bridge_id":1,"_id":1})
                    .build(),
                IndexModel::builder()
                    .keys(doc! {"state":1,"next_check_at_ms":1,"_id":1})
                    .build(),
                IndexModel::builder()
                    .keys(doc! {"invocation_id":1,"sequence":1})
                    .build(),
                IndexModel::builder()
                    .keys(doc! {"response_id":1,"sequence":1})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .unique(true)
                            .build(),
                    )
                    .build(),
            ])
            .await
            .map_err(|_| {
                layer2_core::StoreError::Unavailable("create Bridge outbox indexes".into())
            })?;
        Ok(store)
    }
}

#[derive(thiserror::Error, Debug)]
enum MutationError {
    #[error(transparent)]
    Core(#[from] OutboxError),
    #[error(transparent)]
    Driver(#[from] mongodb::error::Error),
}
impl MutationError {
    fn into_core(self) -> OutboxError {
        match self {
            Self::Core(e) => e,
            Self::Driver(_) => OutboxError::Unavailable,
        }
    }
    fn retryable(&self) -> bool {
        matches!(self,Self::Driver(e) if is_transient(e)||is_duplicate_key(e))
    }
}
impl From<crate::store::resource_access::ReadError> for MutationError {
    fn from(value: crate::store::resource_access::ReadError) -> Self {
        match value {
            crate::store::resource_access::ReadError::Driver(e) => Self::Driver(e),
            crate::store::resource_access::ReadError::Domain(_) => {
                Self::Core(OutboxError::SourceMismatch)
            }
        }
    }
}
fn now_ms() -> i64 {
    mongodb::bson::DateTime::now().timestamp_millis()
}
