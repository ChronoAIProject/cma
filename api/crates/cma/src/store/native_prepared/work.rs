use super::{model::Record, read, unavailable, NativePreparedPlan};
use crate::store::sandbox_mongo::{signed, MongoSandboxStore};
use layer2_core::sandbox::{SandboxStoreError, WarmSandbox, WarmSandboxState};
use mongodb::bson::{doc, Bson};
use mongodb::options::ReturnDocument;

pub(crate) struct NativePreparedClaim {
    pub record: WarmSandbox,
    pub plan: NativePreparedPlan,
    pub workspace_proof: Option<super::NativePreparedWorkspaceProof>,
    token: String,
}

impl MongoSandboxStore {
    pub(crate) async fn claim_native_prepared_work(
        &self,
        at: &str,
        until: &str,
    ) -> Result<Option<NativePreparedClaim>, SandboxStoreError> {
        if layer2_core::seconds_from_rfc3339(at).is_none()
            || layer2_core::seconds_from_rfc3339(until).is_none()
            || until <= at
        {
            return Err(SandboxStoreError::Conflict);
        }
        let token = uuid::Uuid::new_v4().to_string();
        self.native_prepared.find_one_and_update(
            doc!{"kind":"reservation","state":{"$in":["preparing","installing","ready","retiring"]},"lease_until":{"$lte":at},"check_after":{"$lte":at}},
            doc!{"$set":{"lease_token":&token,"lease_until":until}},
        ).sort(doc!{"check_after":1,"_id":1}).return_document(ReturnDocument::After).await.map_err(unavailable)?
            .as_ref().map(|value|{let record=read(value)?;let workspace_proof=super::workspace_proof::read(value,&record.payload)?;Ok(NativePreparedClaim{record:record.payload,plan:record.plan,workspace_proof,token})}).transpose()
    }

    pub(crate) async fn renew_native_prepared_work(
        &self,
        claim: &NativePreparedClaim,
        until: &str,
    ) -> Result<(), SandboxStoreError> {
        let result = self
            .native_prepared
            .update_one(filter(claim)?, doc! {"$set":{"lease_until":until}})
            .await
            .map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn finish_native_prepared_work(
        &self,
        claim: &NativePreparedClaim,
        next: WarmSandbox,
        at: &str,
        check_after: &str,
    ) -> Result<(), SandboxStoreError> {
        let proof = claim
            .workspace_proof
            .clone()
            .filter(|proof| proof.validate(&next).is_ok());
        self.finish_native_prepared_work_with_proof(claim, next, proof, at, check_after)
            .await
    }

    pub(crate) async fn finish_native_prepared_work_with_proof(
        &self,
        claim: &NativePreparedClaim,
        mut next: WarmSandbox,
        workspace_proof: Option<super::NativePreparedWorkspaceProof>,
        at: &str,
        check_after: &str,
    ) -> Result<(), SandboxStoreError> {
        if next.id != claim.record.id
            || next.plan != claim.record.plan
            || next.created_at != claim.record.created_at
            || next.revision != claim.record.revision
            || !allowed(claim.record.state, next.state)
            || at < claim.record.updated_at.as_str()
            || check_after < at
            || layer2_core::seconds_from_rfc3339(check_after).is_none()
        {
            return Err(SandboxStoreError::Conflict);
        }
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        next.updated_at = at.into();
        let mut value = super::workspace_proof::document(
            &Record {
                plan: claim.plan.clone(),
                payload: next,
                ownership: None,
            },
            workspace_proof.as_ref(),
        )?;
        value.insert("lease_token", Bson::Null);
        value.insert("lease_until", at);
        value.insert("check_after", check_after);
        let mut fence = filter(claim)?;
        fence.insert("lease_until", doc! {"$gt":at});
        let result = self
            .native_prepared
            .replace_one(fence, value)
            .await
            .map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        self.warmup_changed.notify_one();
        Ok(())
    }
}

fn allowed(old: WarmSandboxState, next: WarmSandboxState) -> bool {
    use WarmSandboxState::*;
    matches!(
        (old, next),
        (Preparing, Preparing | Installing | Retiring)
            | (Installing, Installing | Ready | Retiring)
            | (Ready, Ready | Retiring)
            | (Retiring, Retiring | Retired)
    )
}
fn filter(claim: &NativePreparedClaim) -> Result<mongodb::bson::Document, SandboxStoreError> {
    Ok(
        doc! {"_id":claim.record.id.as_str(),"kind":"reservation","lease_token":&claim.token,
        "revision":signed(claim.record.revision,"native prepared revision").map_err(SandboxStoreError::Corrupt)?},
    )
}
