//! Observed orphan identity is durable before DELETE, and remains retained
//! after revocation. No operation here changes MintStarted or admits a mint.
use super::{MongoBridgeAllocationStore, MutationError};
use crate::{
    sandbox::channel_mint_reconciliation::authority::ChannelMintAttempt,
    store::{
        durability::majority_collection,
        sandbox_mongo::{
            commit_transaction, is_transient, record_bson, start_transaction, CommitOutcome,
            DOCUMENT_SCHEMA_VERSION,
        },
    },
};
use layer2_core::{bridge::allocation::AllocationError, sandbox::SandboxCredentialAuthority};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use serde::{Deserialize, Serialize};

pub(crate) enum MintReconciliationChange<'a> {
    Read,
    Observe(&'a SandboxCredentialAuthority),
    Revoke(&'a SandboxCredentialAuthority),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelMintOrphanReceipt {
    pub key: SandboxCredentialAuthority,
    pub revoked: bool,
}

impl MongoBridgeAllocationStore {
    pub(crate) async fn change_channel_mint_reconciliation(
        &self,
        attempt: &ChannelMintAttempt<'_>,
        change: MintReconciliationChange<'_>,
    ) -> Result<Option<ChannelMintOrphanReceipt>, AllocationError> {
        for retry in 0..16 {
            if retry > 0 {
                tokio::task::yield_now().await;
            }
            attempt.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = self
                .apply_mint_reconciliation(attempt, &change, &mut session)
                .await;
            let receipt = match result {
                Ok(receipt) => receipt,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error) if is_transient(&error) => continue,
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                    }
                }
            };
            if let Err(error) = attempt.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }

    async fn apply_mint_reconciliation(
        &self,
        attempt: &ChannelMintAttempt<'_>,
        change: &MintReconciliationChange<'_>,
        session: &mut ClientSession,
    ) -> Result<Option<ChannelMintOrphanReceipt>, MutationError> {
        attempt.fence(self, session).await?;
        let database = self.bridges.client.database(&self.records.namespace().db);
        let rows: Collection<Document> = majority_collection(&database, "channel_mint_orphans");
        let id = format!(
            "{}:{}:{}:{}",
            attempt.allocation().id,
            attempt.kind(),
            attempt.generation()?,
            attempt.credential_id()?
        );
        let identity = doc! {"_id":&id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "allocation_id":&attempt.allocation().id,"sandbox_id":attempt.sandbox().id.as_str(),
        "sandbox_created_at":&attempt.sandbox().created_at,"kind":attempt.kind(),
        "generation":i64::try_from(attempt.generation()?).map_err(|_| AllocationError::Corrupt)?,
        "intent":record_bson(attempt.intent()?).map_err(|_| AllocationError::Corrupt)?};
        let previous = rows
            .find_one(doc! {"_id":&id})
            .session(&mut *session)
            .await?;
        let existing = previous
            .as_ref()
            .map(|document| {
                if identity
                    .iter()
                    .any(|(key, value)| document.get(key) != Some(value))
                {
                    return Err(AllocationError::IdentityMismatch);
                }
                let receipt: ChannelMintOrphanReceipt = mongodb::bson::deserialize_from_bson(
                    document
                        .get("receipt")
                        .cloned()
                        .ok_or(AllocationError::Corrupt)?,
                )
                .map_err(|_| AllocationError::Corrupt)?;
                require_key(attempt, &receipt.key)?;
                Ok(receipt)
            })
            .transpose()?;
        let key = match change {
            MintReconciliationChange::Read => existing.as_ref().map(|receipt| &receipt.key),
            MintReconciliationChange::Observe(key) | MintReconciliationChange::Revoke(key) => {
                Some(*key)
            }
        };
        if let Some(key) = key {
            // A successfully custodied generation is never an orphan, even if
            // a damaged sidecar no longer advertises its pending authority.
            let custody: Collection<Document> =
                majority_collection(&database, "sandbox_credentials");
            if custody
                .find_one(doc! {"provider_key_id":&key.provider_key_id})
                .session(&mut *session)
                .await?
                .is_some()
            {
                return Err(AllocationError::IdentityMismatch.into());
            }
        }
        let (receipt, action) = match change {
            MintReconciliationChange::Read => return Ok(existing),
            MintReconciliationChange::Observe(key) => {
                require_key(attempt, key)?;
                if let Some(existing) = existing {
                    if existing.key != **key {
                        return Err(AllocationError::IdentityMismatch.into());
                    }
                    return Ok(Some(existing));
                }
                (
                    ChannelMintOrphanReceipt {
                        key: (*key).clone(),
                        revoked: false,
                    },
                    "observed",
                )
            }
            MintReconciliationChange::Revoke(key) => {
                let mut existing = existing.ok_or(AllocationError::Corrupt)?;
                if existing.key != **key {
                    return Err(AllocationError::IdentityMismatch.into());
                }
                if existing.revoked {
                    return Ok(Some(existing));
                }
                existing.revoked = true;
                (existing, "revoked")
            }
        };
        let mut document = identity;
        document.insert(
            "receipt",
            record_bson(&receipt).map_err(|_| AllocationError::Corrupt)?,
        );
        document.insert(
            "appended_at",
            layer2_core::rfc3339_at(std::time::SystemTime::now()),
        );
        if let Some(previous) = previous {
            let changed = rows
                .replace_one(previous, document.clone())
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(AllocationError::Conflict.into());
            }
        } else {
            rows.insert_one(document.clone())
                .session(&mut *session)
                .await?;
        }
        document.insert("_id", format!("{id}:{action}"));
        document.insert("action", action);
        document.insert("operation_id", &attempt.allocation().operation_id);
        document.insert(
            "fence",
            i64::try_from(attempt.claim_fence()?).map_err(|_| AllocationError::Corrupt)?,
        );
        document.insert(
            "human_actor",
            record_bson(&attempt.human().actor_context().actor)
                .map_err(|_| AllocationError::Corrupt)?,
        );
        let audits: Collection<Document> =
            majority_collection(&database, "channel_mint_orphan_audits");
        audits.insert_one(document).session(&mut *session).await?;
        if receipt.revoked {
            let sandboxes: Collection<Document> = majority_collection(&database, "sandboxes");
            let mut fields = Document::new();
            fields.insert(format!("channel_mint_reconciliation.{}", attempt.kind()),
                doc! {"generation": i64::try_from(attempt.generation()?).map_err(|_| AllocationError::Corrupt)?, "phase":"revoked"});
            let changed = sandboxes.update_one(doc! {"_id":attempt.sandbox().id.as_str(),"revision":attempt.sandbox().revision as i64}, doc! {"$set":fields})
                .session(&mut *session).await?;
            if changed.matched_count != 1 {
                return Err(AllocationError::Conflict.into());
            }
        }
        Ok(Some(receipt))
    }
}

fn require_key(
    attempt: &ChannelMintAttempt<'_>,
    key: &SandboxCredentialAuthority,
) -> Result<(), AllocationError> {
    attempt
        .intent()?
        .require_material(key)
        .map_err(|_| AllocationError::IdentityMismatch)?;
    let credential = &attempt.sandbox().credential;
    if key.generation != attempt.generation()?
        || key.provider_state_version != 1
        || key.provider_key_id == attempt.human().connector().key_id
        || [credential.current.as_ref(), credential.pending.as_ref()]
            .into_iter()
            .flatten()
            .any(|known| known.provider_key_id == key.provider_key_id)
        || credential
            .revoke_debt
            .iter()
            .any(|known| known.provider_key_id == key.provider_key_id)
    {
        return Err(AllocationError::IdentityMismatch);
    }
    Ok(())
}
