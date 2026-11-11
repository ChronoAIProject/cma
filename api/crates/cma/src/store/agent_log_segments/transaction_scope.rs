//! Exclusive transaction lifetime for captured publication proofs.
//!
//! The driver exposes a session id but no transaction number. Never infer
//! transaction identity from the session id or its operation/cluster time.
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{bson::Document, ClientSession, Collection};
use uuid::Uuid;

use crate::store::sandbox_mongo::start_transaction;

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
        collection
            .find_one(filter)
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
        page::validate_limit(limit)?;
        let mut cursor = collection
            .find(filter)
            .sort(sort)
            .limit(i64::from(limit))
            .batch_size(limit)
            .session(&mut *self.session)
            .await?;
        page::collect(cursor.stream(&mut *self.session), limit).await
    }

    pub(in crate::store) async fn update_one(
        &mut self,
        collection: &Collection<Document>,
        filter: Document,
        update: Document,
    ) -> Result<mongodb::results::UpdateResult, mongodb::error::Error> {
        collection
            .update_one(filter, update)
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
