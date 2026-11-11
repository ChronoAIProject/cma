//! Positive current-state evidence; never an admission or runtime permission.
//! An audit-only head revision may reuse an otherwise identical full-Agent witness.
use std::time::Duration;

use layer2_core::sandbox::{store::SandboxStoreError, Agent};
use mongodb::{
    bson::{self, doc},
    options::Hint,
};

use super::{
    capture::{self, CapturedFormat},
    collections::collection,
    corrupt,
    publication::{self, Publication},
    transaction_scope::{TransactionIdentity, TransactionScope},
    Head, PUBLICATIONS,
};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};

pub(super) const INDEX: &str = "agent_log_publication_head";
const MAX_RECEIPT_BYTES: usize = 16 * 1024;

/// Constructed only from the current certified source and exactly one immutable
/// publication in the caller's transaction. Original recovery evidence, fresh
/// authorization and current domain/native checks are separate requirements.
pub(in crate::store) struct CurrentPublication {
    transaction: TransactionIdentity,
    database: String,
    agent: Agent,
    publication: Publication,
}

impl CurrentPublication {
    /// Consume only with the same current Agent/database/transaction. A later
    /// transaction, even on the same session, must capture its own proof.
    pub(in crate::store) fn require_current(
        &self,
        store: &MongoSandboxStore,
        current: &Agent,
        scope: &TransactionScope<'_>,
    ) -> Result<&Publication, SandboxStoreError> {
        scope.require(&self.transaction)?;
        if self.database != store.agents.namespace().db || self.agent != *current {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(&self.publication)
    }
}

/// Read-only and bounded by the certified active segment plus two small receipt
/// rows. No scan over lifetime publications and no missing-evidence fallback.
pub(in crate::store) async fn capture(
    store: &MongoSandboxStore,
    current: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<CurrentPublication, TransactionLogError> {
    let source = capture::capture_transaction(store, current, scope).await?;
    if source.format() != CapturedFormat::Segmented {
        return Err(SandboxStoreError::Conflict.into());
    }
    let head = source.active_head();
    let publications = collection(store, PUBLICATIONS);
    let mut rows = publications
        .find(doc! {
            "agent_id":current.id.as_str(),"incarnation":&head.incarnation,
            "head_revision":{"$lte":head.revision},
        })
        .sort(doc! {"head_revision":-1})
        .hint(Hint::Name(INDEX.into()))
        .limit(-2)
        .batch_size(2)
        .max_time(Duration::from_millis(1000))
        .session(scope.session())
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut receipt: Option<Publication> = None;
    let mut seen = 0;
    while let Some(row) = rows.next(scope.session()).await {
        let row = row.map_err(TransactionLogError::Driver)?;
        seen += 1;
        if seen > 2
            || bson::serialize_to_vec(&row).map_err(|_| corrupt())?.len() > MAX_RECEIPT_BYTES
        {
            return Err(corrupt().into());
        }
        let candidate = Publication::read(row)?;
        if let Some(latest) = &receipt {
            // The second row witnesses strict ordering, even if a drifted
            // nonunique index admits two receipts for the newest head.
            if candidate.head_revision >= latest.head_revision {
                return Err(corrupt().into());
            }
        } else {
            receipt = Some(candidate);
        }
    }

    let publication = receipt.ok_or_else(|| {
        SandboxStoreError::Unavailable("current Agent publication witness is unavailable".into())
    })?;
    require_binding(current, head, &publication)?;
    Ok(CurrentPublication {
        transaction: scope.identity(),
        database: store.agents.namespace().db,
        agent: current.clone(),
        publication,
    })
}

fn require_binding(
    current: &Agent,
    head: &Head,
    receipt: &Publication,
) -> Result<(), SandboxStoreError> {
    receipt.validate()?;
    head.validate()?;
    if receipt.format_version != 2
        || receipt.agent_id != current.id.as_str()
        || receipt.incarnation != super::incarnation(current)
        || receipt.agent_id != head.agent_id
        || receipt.incarnation != head.incarnation
        || receipt.head_revision > head.revision
        || receipt.last_seq != head.last_seq
        || receipt.active_ordinal != head.active_ordinal
        || receipt.previous_seal != head.previous_seal
        || u64::try_from(receipt.result_revision).ok() != Some(current.revision)
        || u64::try_from(receipt.last_seq).ok() != Some(current.conversation.last_seq)
        || receipt.agent_digest.as_ref() != Some(&publication::agent_digest(current)?)
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
