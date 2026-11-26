use super::{model::Record, read, unavailable, NativePreparedPlan};
use crate::store::sandbox_mongo::{signed, MongoSandboxStore};
use layer2_core::sandbox::{SandboxStoreError, WarmSandbox, WarmSandboxState};
use mongodb::bson::{doc, Bson, DateTime, Document};
use mongodb::options::ReturnDocument;

pub(crate) struct NativePreparedClaim {
    pub record: WarmSandbox,
    pub plan: NativePreparedPlan,
    pub workspace_proof: Option<super::NativePreparedWorkspaceProof>,
    token: String,
}

impl MongoSandboxStore {
    #[cfg(test)]
    pub(crate) async fn claim_native_prepared_work(
        &self,
        at: &str,
        until: &str,
    ) -> Result<Option<NativePreparedClaim>, SandboxStoreError> {
        self.claim_native_prepared_work_for(at, until, [true, true])
            .await
    }

    /// Scheduling eligibility only; the durable token still owns every effect.
    /// The two ready-class flags select agent-only and machine health probes.
    pub(crate) async fn claim_native_prepared_work_for(
        &self,
        at: &str,
        until: &str,
        ready_classes: [bool; 2],
    ) -> Result<Option<NativePreparedClaim>, SandboxStoreError> {
        let until_date = timestamp(until)?;
        if layer2_core::seconds_from_rfc3339(at).is_none() || until <= at {
            return Err(SandboxStoreError::Conflict);
        }
        let lane_filter = match (
            self.default_pool_holds.lane(false),
            self.default_pool_holds.lane(true),
        ) {
            (true, true) => return Ok(None),
            (true, false) => doc! {"machine": true},
            (false, true) => doc! {"machine": {"$ne": true}},
            (false, false) => Document::new(),
        };
        let mut states = vec![doc! {"state":{"$in":["preparing","installing","retiring"]}}];
        for (machine, allowed) in [false, true].into_iter().zip(ready_classes) {
            if allowed {
                let channel = if machine {
                    Bson::Boolean(true)
                } else {
                    doc! {"$ne":true}.into()
                };
                states.push(doc! {"state":"ready","machine":channel});
            }
        }
        let token = uuid::Uuid::new_v4().to_string();
        let mut eligible = doc! {"kind":"reservation","$or":states,
        "lease_until":{"$lte":at,"$regex":CANONICAL_TIME},
        "check_after":{"$lte":at,"$regex":CANONICAL_TIME},
        "$expr":{"$and":[
            server_due("lease_until"),server_due("check_after"),
            {"$gt":[until_date,"$$NOW"]},
        ]}};
        eligible.extend(lane_filter);
        // Keep indexed caller bounds as discovery hints, but an ahead caller
        // cannot steal server-live work or run a future scheduled probe.
        self.native_prepared
            .find_one_and_update(
                eligible,
                doc! {"$set":{"lease_token":&token,"lease_until":until}},
            )
            .sort(doc! {"check_after":1,"_id":1})
            .return_document(ReturnDocument::After)
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(|value| {
                let record = read(value)?;
                let workspace_proof = super::workspace_proof::read(value, &record.payload)?;
                Ok(NativePreparedClaim {
                    record: record.payload,
                    plan: record.plan,
                    workspace_proof,
                    token,
                })
            })
            .transpose()
    }

    pub(crate) async fn renew_native_prepared_work(
        &self,
        claim: &NativePreparedClaim,
        until: &str,
    ) -> Result<(), SandboxStoreError> {
        let until_date = timestamp(until)?;
        let mut fence = filter(claim)?;
        fence.insert("lease_until", doc! {"$regex":CANONICAL_TIME});
        // An exact token is necessary but insufficient: a paused worker must
        // not revive an expired claim before the next worker has taken over.
        fence.insert(
            "$expr",
            doc! {"$and":[
                {"$gt":[stored_timestamp("lease_until"),"$$NOW"]},
                {"$gt":[until_date,"$$NOW"]},
            ]},
        );
        let result = self
            .native_prepared
            .update_one(fence, doc! {"$max":{"lease_until":until}})
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
            || layer2_core::seconds_from_rfc3339(at).is_none()
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
        fence.insert("lease_until", doc! {"$gt":at,"$regex":CANONICAL_TIME});
        // The captured timestamp cannot authorize completion after real expiry,
        // even if no successor has replaced the original token yet.
        fence.insert(
            "$expr",
            doc! {"$gt":[stored_timestamp("lease_until"),"$$NOW"]},
        );
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

// Lease metadata keeps its existing canonical string format. Parsing in the
// authority predicate rejects malformed dates and arrays without BSON ordering
// accidentally treating them as live/expired; no new persisted format is used.
const CANONICAL_TIME: &str = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$";

fn stored_timestamp(field: &str) -> Document {
    doc! {"$convert":{"input":format!("${field}"),"to":"date","onError":Bson::Null,"onNull":Bson::Null}}
}

fn server_due(field: &str) -> Document {
    doc! {"$and":[
        {"$ne":[stored_timestamp(field),Bson::Null]},
        {"$lte":[stored_timestamp(field),"$$NOW"]},
    ]}
}

fn timestamp(value: &str) -> Result<DateTime, SandboxStoreError> {
    layer2_core::seconds_from_rfc3339(value)
        .and_then(|seconds| seconds.checked_mul(1000))
        .and_then(|millis| i64::try_from(millis).ok())
        .map(DateTime::from_millis)
        .ok_or(SandboxStoreError::Conflict)
}
