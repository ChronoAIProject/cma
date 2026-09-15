//! Due discovery is a read-only hint. Human authority and every retained
//! identity are resolved and transactionally fenced again before any effect.
use super::super::MongoBridgeAllocationStore;
use crate::{
    sandbox::{
        channel_provisioning::state::ChannelProvisioningState,
        channel_renewal::model::{ChannelRenewalSnapshot, RenewalPhase},
    },
    store::{durability::majority_collection, sandboxes::read_sandbox},
};
use futures_util::TryStreamExt;
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationRecord, ChannelSandboxSidecar},
    sandbox::{SandboxStatus, SandboxWorkStep},
};
use mongodb::{
    bson::{doc, Document},
    Collection, IndexModel,
};
use std::time::{Duration, SystemTime};

impl MongoBridgeAllocationStore {
    pub(crate) async fn initialize_channel_renewal_discovery(&self) -> Result<(), AllocationError> {
        let rows = self.renewal_sandboxes();
        for (name, fields) in [
            (
                "channel_renewal_age",
                doc! {"status":1,"channel_provisioning.phase":1,"record.credential.current.issued_at":1,"_id":1},
            ),
            (
                "channel_renewal_expiry",
                doc! {"status":1,"channel_provisioning.phase":1,"record.credential.current.expires_at":1,"_id":1},
            ),
            (
                "channel_renewal_resume",
                doc! {"status":1,"channel_renewal.phase":1,"channel_renewal.claim.expires_at_ms":1,"_id":1},
            ),
        ] {
            rows.create_index(
                IndexModel::builder()
                    .keys(fields)
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .name(name.to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        }
        self.records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"sandbox_admission.sandbox_id":1})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .name("channel_sandbox_origin".to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let audits: Collection<Document> = majority_collection(
            &self.bridges.client.database(&self.records.namespace().db),
            "channel_credential_renewal_audits",
        );
        audits
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"sandbox_id":1,"revision":1})
                    .build(),
            )
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        Ok(())
    }

    /// Returns at most 32 ordered raw Sandbox IDs. Decoding belongs to the
    /// individual work call, so a corrupt earlier row cannot stop later work.
    pub(crate) async fn due_channel_renewal_sandboxes(
        &self,
        after: Option<&str>,
        now: SystemTime,
    ) -> Result<Vec<String>, AllocationError> {
        let age = layer2_core::rfc3339_at(
            now.checked_sub(Duration::from_secs(12 * 60 * 60))
                .ok_or(AllocationError::Unavailable)?,
        );
        let expiry = layer2_core::rfc3339_at(now + Duration::from_secs(5 * 60));
        let due = doc! {"$or":[{"record.credential.current.issued_at":{"$lte":age}}, {"record.credential.current.expires_at":{"$lte":expiry}}]};
        let mut filter = doc! {"status":"active","channel_provisioning.phase":"ready","channel_creation":{"$exists":true},
            "$and":[{"$or":[{"channel_renewal_retry_at_ms":{"$exists":false}}, {"channel_renewal_retry_at_ms":{"$lte":super::now_ms(now)?}}]}],
        "_id":{"$type":"string"},"$or":[
            {"$and":[{"channel_renewal":{"$exists":false}},due.clone()]},
            {"$and":[{"channel_renewal.phase":"complete"},due]},
            {"channel_renewal.phase":{"$in":["deliver","revoke"]},"channel_renewal.claim.expires_at_ms":{"$lte":super::now_ms(now)?}},
            {"channel_renewal.phase":"mint_started","channel_renewal.mint_intent":{"$exists":true},"channel_mint_reconciliation.renewal.phase":{"$ne":"revoked"},"channel_renewal.claim.expires_at_ms":{"$lte":super::now_ms(now)?}}
        ]};
        if let Some(after) = after {
            filter.insert("_id", doc! {"$type":"string","$gt":after});
        }
        let mut cursor = self
            .renewal_sandboxes()
            .find(filter)
            .projection(doc! {"_id":1})
            .sort(doc! {"_id":1})
            .limit(32)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let mut ids = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|_| AllocationError::Unavailable)?
        {
            ids.push(
                document
                    .get_str("_id")
                    .map_err(|_| AllocationError::Corrupt)?
                    .to_owned(),
            );
        }
        Ok(ids)
    }

    /// Retry timing is only a discovery hint. It never changes aggregate
    /// identity, authority, credential state, runtime/session, or work claims.
    pub(crate) async fn defer_channel_renewal(
        &self,
        sandbox_id: &str,
        delay: Duration,
        state: &str,
    ) -> Result<(), AllocationError> {
        let ready = SystemTime::now()
            .checked_add(delay)
            .ok_or(AllocationError::Unavailable)?;
        self.renewal_sandboxes().update_one(doc! {"_id":sandbox_id,"channel_creation":{"$exists":true}},
            doc! {"$set":{"channel_renewal_retry_at_ms":super::now_ms(ready)?,"channel_renewal_retry_state":state}})
            .await.map_err(|_| AllocationError::Unavailable)?;
        Ok(())
    }

    pub(crate) async fn due_channel_renewal_record(
        &self,
        sandbox_id: &str,
        now: SystemTime,
    ) -> Result<Option<AllocationRecord>, AllocationError> {
        let Some(document) = self
            .renewal_sandboxes()
            .find_one(doc! {"_id":sandbox_id})
            .await
            .map_err(|_| AllocationError::Unavailable)?
        else {
            return Ok(None);
        };
        if document
            .get_i64("channel_renewal_retry_at_ms")
            .ok()
            .is_some_and(|retry| retry > super::now_ms(now).unwrap_or(i64::MIN))
        {
            return Ok(None);
        }
        let sandbox = read_sandbox(&document).map_err(|_| AllocationError::Corrupt)?;
        if sandbox.status != SandboxStatus::Active
            || sandbox.work.step != SandboxWorkStep::None
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
        {
            return Ok(None);
        }
        let creation: ChannelSandboxSidecar = decode(&document, "channel_creation")?;
        let provisioning: ChannelProvisioningState = decode(&document, "channel_provisioning")?;
        let allocation = self
            .channel_allocation(&creation.allocation_id)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        let snapshot = ChannelRenewalSnapshot {
            allocation,
            sandbox,
            creation,
            provisioning,
            lifecycle_credential: document
                .get("channel_lifecycle_credential")
                .map(|value| {
                    mongodb::bson::deserialize_from_bson(value.clone())
                        .map_err(|_| AllocationError::Corrupt)
                })
                .transpose()?,
            renewal: document
                .get("channel_renewal")
                .map(|value| {
                    mongodb::bson::deserialize_from_bson(value.clone())
                        .map_err(|_| AllocationError::Corrupt)
                })
                .transpose()?,
        };
        snapshot.validate()?;
        if snapshot.renewal.as_ref().is_some_and(|state| {
            (state.phase == RenewalPhase::MintStarted
                && (state.mint_intent.is_none()
                    || document
                        .get_document("channel_mint_reconciliation")
                        .ok()
                        .and_then(|value| value.get_document("renewal").ok())
                        .is_some_and(|value| value.get_str("phase").ok() == Some("revoked"))))
                || state.claim.expires_at_ms > super::now_ms(SystemTime::now()).unwrap_or(i64::MIN)
        }) || !snapshot.due(super::now_ms(now)?)?
        {
            return Ok(None);
        }
        Ok(Some(snapshot.allocation))
    }
    fn renewal_sandboxes(&self) -> Collection<Document> {
        majority_collection(
            &self.bridges.client.database(&self.records.namespace().db),
            "sandboxes",
        )
    }
}
fn decode<T: serde::de::DeserializeOwned>(
    document: &Document,
    key: &str,
) -> Result<T, AllocationError> {
    mongodb::bson::deserialize_from_bson(
        document.get(key).cloned().ok_or(AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)
}
