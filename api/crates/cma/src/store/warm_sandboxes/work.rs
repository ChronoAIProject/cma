use layer2_core::sandbox::{SandboxStoreError, WarmSandbox};
use mongodb::bson::{doc, Bson};
use mongodb::options::ReturnDocument;

use super::{document, read, unavailable};
use crate::store::sandbox_mongo::{signed, MongoSandboxStore};

pub(crate) struct WarmSandboxClaim {
    pub record: WarmSandbox,
    token: String,
}

impl MongoSandboxStore {
    pub(crate) async fn claim_legacy_warm_cleanup(
        &self,
        at: &str,
        until: &str,
    ) -> Result<Option<WarmSandboxClaim>, SandboxStoreError> {
        self.claim_warm_work(at, until, &["retiring"]).await
    }

    #[cfg(test)]
    pub(crate) async fn claim_warmup_work(
        &self,
        at: &str,
        until: &str,
    ) -> Result<Option<WarmSandboxClaim>, SandboxStoreError> {
        self.claim_warm_work(at, until, &["preparing", "installing", "ready", "retiring"])
            .await
    }

    async fn claim_warm_work(
        &self,
        at: &str,
        until: &str,
        states: &[&str],
    ) -> Result<Option<WarmSandboxClaim>, SandboxStoreError> {
        let token = uuid::Uuid::new_v4().to_string();
        let document = self
            .warm_sandboxes
            .find_one_and_update(
                doc! {
                    "state": { "$in": states },
                    "lease_until": { "$lte": at }, "check_after": { "$lte": at },
                },
                doc! { "$set": { "lease_token": &token, "lease_until": until } },
            )
            .sort(doc! { "check_after": 1, "_id": 1 })
            .return_document(ReturnDocument::After)
            .await
            .map_err(unavailable)?;
        document
            .as_ref()
            .map(|value| {
                Ok(WarmSandboxClaim {
                    record: read(value)?,
                    token,
                })
            })
            .transpose()
    }

    pub(crate) async fn renew_warmup_work(
        &self,
        claim: &WarmSandboxClaim,
        until: &str,
    ) -> Result<(), SandboxStoreError> {
        let result = self
            .warm_sandboxes
            .update_one(
                claim_filter(claim)?,
                doc! { "$set": { "lease_until": until } },
            )
            .await
            .map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    pub(crate) async fn finish_warmup_work(
        &self,
        claim: &WarmSandboxClaim,
        mut next: WarmSandbox,
        at: &str,
        check_after: &str,
    ) -> Result<(), SandboxStoreError> {
        if next.id != claim.record.id
            || next.plan != claim.record.plan
            || next.created_at != claim.record.created_at
            || next.revision != claim.record.revision
        {
            return Err(SandboxStoreError::Conflict);
        }
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        next.updated_at = at.into();
        let mut value = document(&next)?;
        value.insert("lease_token", Bson::Null);
        value.insert("lease_until", at);
        value.insert("check_after", check_after);
        let result = self
            .warm_sandboxes
            .replace_one(claim_filter(claim)?, value)
            .await
            .map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        self.warmup_changed.notify_one();
        Ok(())
    }
}

fn claim_filter(claim: &WarmSandboxClaim) -> Result<mongodb::bson::Document, SandboxStoreError> {
    Ok(
        doc! { "_id": claim.record.id.as_str(), "lease_token": &claim.token,
            "revision": signed(claim.record.revision, "warm Sandbox revision").map_err(SandboxStoreError::Corrupt)?,
        },
    )
}
