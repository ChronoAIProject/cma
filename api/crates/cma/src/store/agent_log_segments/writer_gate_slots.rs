//! Bounded cross-replica bootstrap leases. Immutable admission survives closure;
//! a lease is separate, expiring authority to perform and publish preparation.
use super::*;
use crate::store::agent_log_segments::{
    publication::{self, PublicationIdentity},
    writer_admission::{Admission, AdmissionKind},
};
use mongodb::bson::{Binary, DateTime};

const LEASE_MILLIS: i64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct Slot {
    pub holder: String,
    pub fingerprint: Binary,
    pub fence: i64,
    pub expires_at: DateTime,
}

#[derive(Debug, Clone)]
pub(in crate::store) struct Permit {
    name: GateName,
    slot: Slot,
}

fn limit(name: GateName) -> usize {
    match name {
        GateName::Segments => 2,
        GateName::CapacityRecovery => 1,
    }
}

pub(super) fn validate(gate: &GateRecord) -> Result<(), SandboxStoreError> {
    let mut holders = HashSet::new();
    let mut fences = HashSet::new();
    if gate.slots.len() > limit(gate.name)
        || gate.slots.iter().any(|slot| {
            slot.holder.is_empty()
                || slot.holder.len() > 1200
                || !super::super::digest::valid_digest(&slot.fingerprint)
                || slot.fence < 1
                || slot.fence > gate.admission_fence
                || slot.expires_at.timestamp_millis() <= 0
                || !holders.insert(&slot.holder)
                || !fences.insert(slot.fence)
        })
    {
        return Err(invalid());
    }
    Ok(())
}

/// Claim/reclaim one slot after exact admission has been committed or observed.
/// Reclaim always issues a new fence, including for the same logical request.
/// Clock skew can cause a conservative conflict; server time decides expiry.
/// Commit uncertainty must be reconciled before this permit is used.
pub(in crate::store) async fn claim(
    database: &Database,
    agent: &Agent,
    operation: &PublicationIdentity,
    admission: &Admission,
    now: DateTime,
    session: &mut ClientSession,
) -> Result<Option<Permit>, TransactionLogError> {
    admission.permits(agent, operation, admission.kind)?;
    let name = match admission.kind {
        AdmissionKind::Activation => GateName::Segments,
        AdmissionKind::CapacityRecovery => GateName::CapacityRecovery,
        AdmissionKind::Rollover => return Ok(None),
    };
    let mut gate = read(database, name, session).await?.ok_or_else(invalid)?;
    let holder = publication::identity(&admission.agent_id, &admission.incarnation, &operation.key);
    let existing = gate.slots.iter().position(|slot| slot.holder == holder);
    if existing.is_some_and(|index| gate.slots[index].fingerprint != operation.fingerprint) {
        return Err(SandboxStoreError::Conflict.into());
    }
    let replace = existing.or_else(|| gate.slots.iter().position(|slot| slot.expires_at <= now));
    if replace.is_none() && gate.slots.len() == limit(name) {
        return Err(
            SandboxStoreError::Unavailable("Agent log preparation slots are busy".into()).into(),
        );
    }
    let fence = gate.admission_fence.checked_add(1).ok_or_else(invalid)?;
    let expires_at = now
        .timestamp_millis()
        .checked_add(LEASE_MILLIS)
        .ok_or_else(invalid)?;
    let slot = Slot {
        holder,
        fingerprint: operation.fingerprint.clone(),
        fence,
        expires_at: DateTime::from_millis(expires_at),
    };
    let mut conditions = vec![
        doc! {"$gt": [slot.expires_at, "$$NOW"]},
        doc! {"$lte": [slot.expires_at, {"$add": ["$$NOW", LEASE_MILLIS]}]},
    ];
    if let Some(index) = replace {
        if existing.is_none() {
            conditions.push(doc! {"$lte": [gate.slots[index].expires_at, "$$NOW"]});
        }
        gate.slots[index] = slot.clone();
    } else {
        gate.slots.push(slot.clone());
    }
    let result = database
        .collection::<Document>(COLLECTION)
        .update_one(
            doc! {"_id":name.as_str(),"format_version":1_i64,"revision":gate.revision,
            "admission_fence":gate.admission_fence,"$expr":{"$and":conditions}},
            doc! {"$set":{"slots":bson::serialize_to_bson(&gate.slots).map_err(|_| invalid())?},
            "$inc":{"admission_fence":1_i64}},
        )
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(Some(Permit { name, slot }))
}

impl Permit {
    pub(in crate::store) fn validate_admission(
        &self,
        admission: &Admission,
    ) -> Result<(), SandboxStoreError> {
        let expected_name = match admission.kind {
            AdmissionKind::Activation => GateName::Segments,
            AdmissionKind::CapacityRecovery => GateName::CapacityRecovery,
            AdmissionKind::Rollover => return Err(invalid()),
        };
        if self.name != expected_name
            || self.slot.holder != admission.id
            || self.slot.fingerprint != admission.fingerprint
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    /// Fence token for the staged generation; never use the gate policy revision
    /// as a preparation lease, since independent claims share that revision.
    pub fn fence(&self) -> i64 {
        self.slot.fence
    }

    /// Renew ongoing bounded preparation without changing its generation fence.
    /// The exact current lease must still be live according to database time.
    pub async fn renew(
        &self,
        database: &Database,
        now: DateTime,
        session: &mut ClientSession,
    ) -> Result<Self, TransactionLogError> {
        let mut renewed = self.clone();
        renewed.slot.expires_at = DateTime::from_millis(
            now.timestamp_millis()
                .checked_add(LEASE_MILLIS)
                .ok_or_else(invalid)?,
        );
        let result = database.collection::<Document>(COLLECTION).update_one(
            doc! {"_id":self.name.as_str(),"format_version":1_i64,
                "slots":{"$elemMatch":bson::serialize_to_document(&self.slot).map_err(|_| invalid())?},
                "$expr":{"$and":[
                    {"$gt":[self.slot.expires_at,"$$NOW"]},
                    {"$gte":[renewed.slot.expires_at,self.slot.expires_at]},
                    {"$lte":[renewed.slot.expires_at,{"$add":["$$NOW",LEASE_MILLIS]}]}
                ]}},
            doc! {"$set":{"slots.$.expires_at":renewed.slot.expires_at}},
        ).session(session).await.map_err(TransactionLogError::Driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
        Ok(renewed)
    }

    /// Consume in the publication transaction, after domain/source revalidation.
    /// Gate closure preserves exact accepted work; expiry/replacement rejects it.
    /// A failed transaction restores the slot together with every other write.
    pub async fn consume(
        &self,
        database: &Database,
        admission: &Admission,
        session: &mut ClientSession,
    ) -> Result<(), TransactionLogError> {
        self.validate_admission(admission)?;
        let result = database.collection::<Document>(COLLECTION).update_one(
            doc! {"_id":self.name.as_str(),"format_version":1_i64,
                "slots":{"$elemMatch":bson::serialize_to_document(&self.slot).map_err(|_| invalid())?},
                "$expr":{"$gt":[self.slot.expires_at,"$$NOW"]}},
            doc! {"$pull":{"slots":{"holder":&self.slot.holder,"fence":self.slot.fence}}},
        ).session(session).await.map_err(TransactionLogError::Driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
        Ok(())
    }
}
