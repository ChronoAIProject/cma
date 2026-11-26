//! Exclusive transaction lifetime for captured publication proofs.
//!
//! The driver exposes a session id but no transaction number. Never infer
//! transaction identity from the session id or its operation/cluster time.
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{bson::Document, ClientSession, Collection};
use uuid::Uuid;

use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::start_transaction};

mod page;

pub(in crate::store) struct TransactionScope<'session> {
    session: &'session mut ClientSession,
    identity: TransactionIdentity,
}

/// Only capture code may retain this stamp. It cannot be constructed outside
/// this module, serialized, or substituted with a session id.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct TransactionIdentity {
    session_id: Document,
    nonce: Uuid,
}

impl<'session> TransactionScope<'session> {
    /// Own a fresh snapshot/majority transaction, borrowing the caller session
    /// exclusively until release. There is deliberately no adopt constructor.
    pub(in crate::store) async fn start(
        session: &'session mut ClientSession,
    ) -> Result<Self, mongodb::error::Error> {
        start_transaction(session).await?;
        Ok(Self {
            identity: TransactionIdentity::new(session.id().clone()),
            session,
        })
    }

    pub(in crate::store) async fn fence_ordinary_work(
        &mut self,
        store: &crate::store::MongoSandboxStore,
    ) -> Result<(), crate::store::work_publication::FenceError> {
        crate::store::work_publication::fence_transaction(store, self.session).await
    }

    pub(in crate::store) async fn control_snapshot(
        &mut self,
        store: &crate::store::sandbox_mongo::MongoSandboxStore,
        target: &layer2_core::sandbox::store::AgentControlTarget,
    ) -> Result<
        crate::store::agent_control_lineage::ControlSnapshot,
        crate::store::agent_control_lineage::LineageError,
    > {
        crate::store::agent_control_lineage::load_control_snapshot(
            store,
            target,
            self.session,
            false,
        )
        .await
    }

    pub(in crate::store) async fn live_agents(
        &mut self,
        store: &crate::store::MongoSandboxStore,
        sandbox_id: &layer2_core::SandboxId,
        workspace_id: &layer2_core::WorkspaceId,
    ) -> Result<u64, TransactionLogError> {
        use crate::store::shared_child_capacity::{agents, CapacityError};
        agents(store, sandbox_id, workspace_id, self.session)
            .await
            .map_err(|error| match error {
                CapacityError::Domain(error) => error.into(),
                CapacityError::Driver(error) => TransactionLogError::Driver(error),
            })
    }

    pub(in crate::store) async fn retained_reopen_view(
        &mut self,
        store: &crate::store::MongoSandboxStore,
        id: &layer2_core::SandboxId,
        path: layer2_core::sandbox::RetainedReopenPath,
        policy: crate::store::agent_log_integrity::IntegrityFailurePolicy,
    ) -> Result<
        (layer2_core::sandbox::RetainedReopenSnapshot, u64),
        crate::store::sandbox_unpublished_recovery::transaction::ApplyError,
    > {
        use crate::store::sandbox_unpublished_recovery::transaction::NativeFacts;
        let facts = NativeFacts::capture_reopen(store, id, self).await?;
        crate::store::retained_reopen::snapshot_in_transaction(
            store,
            id,
            self.session,
            &facts,
            path,
            policy,
        )
        .await
    }

    pub(in crate::store) async fn retained_reopen_history(
        &mut self,
        store: &crate::store::MongoSandboxStore,
        row: &Document,
        key: &str,
    ) -> Result<
        layer2_core::sandbox::RetainedReopenHistory,
        crate::store::sandbox_unpublished_recovery::transaction::ApplyError,
    > {
        crate::store::retained_reopen::read_history(store, row, Some(key), self.session, None).await
    }

    pub(in crate::store) async fn settle_runtime_loss_budget(
        &mut self,
        store: &crate::store::MongoSandboxStore,
        current: &layer2_core::sandbox::Agent,
        change: &layer2_core::sandbox::store::SandboxRuntimeLossAgentMutation,
        at: &str,
    ) -> Result<(), TransactionLogError> {
        crate::store::budget::settle_runtime_loss(store, current, change, at, self.session).await
    }

    pub(in crate::store) async fn settle_owner_stop_budget(
        &mut self,
        store: &crate::store::MongoSandboxStore,
        current: &layer2_core::sandbox::Agent,
        at: &str,
    ) -> Result<(), TransactionLogError> {
        use crate::store::agent_conversation_store::ApplyError;
        crate::store::budget::settle_owner_stop(store, current, at, self.session)
            .await
            .map_err(|error| match error {
                ApplyError::Driver(error) => TransactionLogError::Driver(error),
                ApplyError::Domain(error) => TransactionLogError::Domain(error),
            })
    }

    /// Release consumes the scope. The caller may commit/abort and do normal
    /// outcome reconciliation; a later transaction requires a new scope/stamp.
    pub(in crate::store) fn release(self) -> &'session mut ClientSession {
        self.session
    }

    /// Caller-owned reads may join the transaction without obtaining the raw
    /// session or being able to commit, abort or restart it under an old proof.
    pub(in crate::store) async fn find_one(
        &mut self,
        collection: &Collection<Document>,
        filter: Document,
    ) -> Result<Option<Document>, mongodb::error::Error> {
        crate::store::stale_turns::find_with_proof_budget(
            collection,
            crate::store::work_schema::for_collection(collection, filter),
            &mut *self.session,
            false,
        )
        .await
    }

    pub(in crate::store) async fn find_projected(
        &mut self,
        collection: &Collection<Document>,
        filter: Document,
        projection: Document,
    ) -> Result<Option<Document>, mongodb::error::Error> {
        collection
            .find_one(crate::store::work_schema::for_collection(
                collection, filter,
            ))
            .projection(projection)
            .session(&mut *self.session)
            .await
    }

    /// Bounded enumeration in this transaction. No cursor, callback or raw
    /// session escapes; callers cannot change transaction identity mid-read.
    pub(in crate::store) async fn find_page(
        &mut self,
        collection: &Collection<Document>,
        filter: Document,
        sort: Document,
        limit: u32,
    ) -> Result<Vec<Document>, mongodb::error::Error> {
        self.find_projected_page(collection, filter, sort, limit, None)
            .await
    }

    pub(in crate::store) async fn find_projected_page(
        &mut self,
        collection: &Collection<Document>,
        filter: Document,
        sort: Document,
        limit: u32,
        projection: Option<Document>,
    ) -> Result<Vec<Document>, mongodb::error::Error> {
        page::validate_limit(limit)?;
        let query = collection
            .find(crate::store::work_schema::for_collection(
                collection, filter,
            ))
            .sort(sort)
            .limit(i64::from(limit))
            .batch_size(limit);
        let query = match projection {
            Some(projection) => query.projection(projection),
            None => query,
        };
        let mut cursor = query.session(&mut *self.session).await?;
        page::collect(cursor.stream(&mut *self.session), limit).await
    }

    pub(in crate::store) async fn update_one(
        &mut self,
        collection: &Collection<Document>,
        filter: Document,
        update: Document,
    ) -> Result<mongodb::results::UpdateResult, mongodb::error::Error> {
        collection
            .update_one(
                crate::store::work_publication::for_collection(collection, filter)
                    .map_err(mongodb::error::Error::custom)?,
                update,
            )
            .session(&mut *self.session)
            .await
    }

    pub(in crate::store) async fn insert_one(
        &mut self,
        collection: &Collection<Document>,
        document: Document,
    ) -> Result<mongodb::results::InsertOneResult, mongodb::error::Error> {
        collection
            .insert_one(document)
            .session(&mut *self.session)
            .await
    }

    // Restricted to the audited segment implementation. Those modules must
    // never call transaction lifecycle APIs; caller code cannot borrow this.
    pub(super) fn session(&mut self) -> &mut ClientSession {
        self.session
    }

    pub(super) fn identity(&self) -> TransactionIdentity {
        self.identity.clone()
    }

    pub(super) fn require(&self, identity: &TransactionIdentity) -> Result<(), SandboxStoreError> {
        self.identity.require(identity)
    }
}

impl TransactionIdentity {
    fn new(session_id: Document) -> Self {
        Self {
            session_id,
            nonce: Uuid::new_v4(),
        }
    }

    fn require(&self, other: &Self) -> Result<(), SandboxStoreError> {
        if self != other {
            return Err(SandboxStoreError::Unavailable(
                "Agent log proof belongs to another transaction scope".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::doc;

    #[test]
    fn later_transaction_on_the_same_session_cannot_reuse_the_proof() {
        let session = doc! { "id": "test-session" };
        let first = TransactionIdentity::new(session.clone());
        let proof = first.clone();
        assert!(first.require(&proof).is_ok());
        let later = TransactionIdentity::new(session);
        assert!(later.require(&proof).is_err());
    }

    #[test]
    fn another_session_cannot_reuse_the_proof_even_with_the_same_nonce() {
        let first = TransactionIdentity::new(doc! { "id": "first" });
        let other = TransactionIdentity {
            session_id: doc! { "id": "other" },
            nonce: first.nonce,
        };
        assert!(other.require(&first).is_err());
    }
}
