//! Immutable admission of one exact publication. Closure can finish a finite
//! admitted batch; it cannot turn a prior turn or worker lease into an unlimited
//! permit for future output or different payloads. A fresh lease may advance the
//! aggregate revision; it does not change the original logical publication.
use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, SharedOwned,
};
use layer2_core::{
    sandbox::{store::SandboxStoreError, Agent},
    seconds_from_rfc3339,
};
use mongodb::{
    bson::{self, doc, Binary, Document, RawDocumentBuf},
    ClientSession, Database,
};
use serde::{Deserialize, Serialize};

use super::{
    corrupt, digest,
    publication::{self, PublicationIdentity},
    writer_gate::{self, GateName, GateProof},
};
use crate::store::{
    agent_log_integrity::TransactionLogError,
    ingress_reads::{
        error::{self, ReadFailure},
        layout::{BsonLayout, LayoutError},
        row::{self, ConversionPlan, RowSchema},
    },
    sandbox_mongo::is_duplicate_key,
};

pub(in crate::store) const COLLECTION: &str = "agent_log_admissions";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum AdmissionKind {
    Activation,
    Rollover,
    CapacityRecovery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct Admission {
    #[serde(rename = "_id")]
    pub id: String,
    pub format_version: i64,
    pub agent_id: String,
    pub incarnation: String,
    pub owner: String,
    pub publication_key: String,
    pub fingerprint: Binary,
    pub appended_at: String,
    pub expected_agent_revision: i64,
    pub expected_seq: i64,
    pub admitted_at: String,
    pub kind: AdmissionKind,
    pub writer_gate: GateProof,
    #[serde(deserialize_with = "super::required_nullable")]
    pub recovery_gate: Option<GateProof>,
}

impl Admission {
    pub fn read(row: Document) -> Result<Self, SandboxStoreError> {
        for field in ["format_version", "expected_agent_revision", "expected_seq"] {
            row.get_i64(field).map_err(|_| corrupt())?;
        }
        let value: Self = bson::deserialize_from_document(row).map_err(|_| corrupt())?;
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.format_version != 1
            || self.agent_id.is_empty()
            || self.agent_id.len() > 512
            || self.incarnation.len() != 64
            || self.owner.is_empty()
            || self.owner.len() > 512
            || self.publication_key.is_empty()
            || self.publication_key.len() > 512
            || self.id
                != publication::identity(&self.agent_id, &self.incarnation, &self.publication_key)
            || !digest::valid_digest(&self.fingerprint)
            || self.appended_at.len() > 64
            || seconds_from_rfc3339(&self.appended_at).is_none()
            || self.expected_agent_revision < 0
            || !(0..=super::MAX_SEQUENCE).contains(&self.expected_seq)
            || seconds_from_rfc3339(&self.admitted_at).is_none()
            || self.writer_gate.name != GateName::Segments
            || self.writer_gate.revision < 1
            || (self.kind == AdmissionKind::CapacityRecovery) != self.recovery_gate.is_some()
            || self
                .recovery_gate
                .is_some_and(|proof| proof.name != GateName::CapacityRecovery || proof.revision < 1)
        {
            return Err(corrupt());
        }
        Ok(())
    }

    /// This proves only the original cohort admission of one logical request.
    /// Before publishing, the caller must check the immutable completed receipt
    /// and validate today's domain, lineage and lease authority in its existing
    /// transaction. A known-aborted attempt may replan after a lease or sequence
    /// advance, keeping the same request fingerprint and timestamp. Inserting
    /// the unique result receipt with the batch consumes this finite exception.
    pub fn permits(
        &self,
        agent: &Agent,
        operation: &PublicationIdentity,
        kind: AdmissionKind,
    ) -> Result<(), SandboxStoreError> {
        self.validate()?;
        operation.validate()?;
        if self.agent_id != agent.id.as_str()
            || self.incarnation != digest::incarnation(agent)
            || self.owner != agent.owner.as_str()
            || self.publication_key != operation.key
            || self.fingerprint != operation.fingerprint
            || self.appended_at != operation.appended_at
            || self.kind != kind
            || agent.revision < self.expected_agent_revision as u64
            || agent.conversation.last_seq < self.expected_seq as u64
            || agent.revision > i64::MAX as u64
            || agent.conversation.last_seq > super::MAX_SEQUENCE as u64
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}

pub(in crate::store) async fn read(
    database: &Database,
    agent: &Agent,
    operation: &PublicationIdentity,
    session: &mut ClientSession,
) -> Result<Option<Admission>, TransactionLogError> {
    operation.validate()?;
    let incarnation = digest::incarnation(agent);
    let row = database
        .collection::<Document>(COLLECTION)
        .find_one(
            doc! {"_id":publication::identity(agent.id.as_str(), &incarnation, &operation.key)},
        )
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let value = row.map(Admission::read).transpose()?;
    if let Some(value) = &value {
        validate_read(value, agent, operation, &incarnation)?;
    }
    Ok(value)
}

fn validate_read(
    value: &Admission,
    agent: &Agent,
    operation: &PublicationIdentity,
    incarnation: &str,
) -> Result<(), SandboxStoreError> {
    if value.agent_id != agent.id.as_str()
        || value.incarnation != incarnation
        || value.owner != agent.owner.as_str()
        || value.publication_key != operation.key
    {
        return Err(corrupt());
    }
    if value.fingerprint != operation.fingerprint || value.appended_at != operation.appended_at {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

type AdmissionRead<'a> = AdmittedOperation<'a, Result<Option<SharedOwned<Admission>>, ReadFailure>>;

/// The caller's read plan covers query/raw response/driver storage. Driver
/// bounds and detached cleanup still require their separately qualified port.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "enabled recording awaits driver resource custody")
)]
pub(in crate::store) fn read_admitted<'a>(
    database: &'a Database,
    agent: &'a Agent,
    operation: &'a PublicationIdentity,
    session: &'a mut ClientSession,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<AdmissionRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        // Incarnation formatting, validation diagnostics and the owned error.
        AllocationPlan::checked(
            "publication admission read",
            &[1024, shared_owner_bytes::<SandboxStoreError>()],
        )?,
        move |scope| async move {
            if let Err(error) = operation.validate() {
                return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
            }
            let incarnation = digest::incarnation(agent);
            let result = row::load_with_error::<AdmissionSchema, _, _>(policy, read_plan, || async {
                database
                    .collection::<RawDocumentBuf>(COLLECTION)
                    .find_one(doc! {
                        "_id": publication::identity(agent.id.as_str(), &incarnation, &operation.key)
                    })
                    .session(session)
                    .await
            })
            .await;
            let value = match result {
                Ok(value) => value,
                Err(error) => return error::finish(Err(error), &scope),
            };
            if let Some(value) = &value {
                if let Err(error) = validate_read(value, agent, operation, &incarnation) {
                    return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
                }
            }
            Ok(scope.initialize_owned(|| Ok(value)))
        },
    )
}

struct AdmissionSchema;

impl RowSchema for AdmissionSchema {
    type Value = Admission;
    const INVALID_RAW: &'static str = "invalid raw publication admission";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        // Admission has only inline fields, Strings and one binary Vec; the
        // borrowed layout counts all string/binary bytes, including field names.
        let retained = layout.domain_bytes(0, shared_owner_bytes::<Admission>())?;
        // BSON/serde storage, partial typed output, identity formatting and
        // malformed-field diagnostics can overlap before validation completes.
        let temporary = layout
            .bson_bytes()?
            .checked_mul(3)
            .and_then(|bytes| retained.checked_mul(2)?.checked_add(bytes))
            .and_then(|bytes| layout.wire_bytes.checked_mul(4)?.checked_add(bytes))
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or(LayoutError::Structure)?;
        Ok(ConversionPlan {
            temporary_bytes: temporary,
            retained_bytes: retained,
        })
    }

    fn decode(row: Document) -> Result<Admission, SandboxStoreError> {
        Admission::read(row)
    }
}

/// Run before the publication transaction, using the same stable request key,
/// fingerprint and timestamp across known-abort retries. Commit uncertainty
/// must be resolved by reading this exact admission before proceeding.
pub(in crate::store) async fn admit(
    database: &Database,
    agent: &Agent,
    operation: &PublicationIdentity,
    kind: AdmissionKind,
    admitted_at: &str,
    session: &mut ClientSession,
) -> Result<Admission, TransactionLogError> {
    if let Some(existing) = read(database, agent, operation, session).await? {
        existing.permits(agent, operation, kind)?;
        return Ok(existing);
    }
    let writer_gate = writer_gate::select_and_fence(database, GateName::Segments, agent, session)
        .await?
        .ok_or_else(closed)?;
    let recovery_gate = if kind == AdmissionKind::CapacityRecovery {
        Some(
            writer_gate::select_and_fence(database, GateName::CapacityRecovery, agent, session)
                .await?
                .ok_or_else(closed)?,
        )
    } else {
        None
    };
    let value = Admission {
        id: publication::identity(
            agent.id.as_str(),
            &digest::incarnation(agent),
            &operation.key,
        ),
        format_version: 1,
        agent_id: agent.id.0.clone(),
        incarnation: digest::incarnation(agent),
        owner: agent.owner.as_str().into(),
        publication_key: operation.key.clone(),
        fingerprint: operation.fingerprint.clone(),
        appended_at: operation.appended_at.clone(),
        expected_agent_revision: i64::try_from(agent.revision).map_err(|_| corrupt())?,
        expected_seq: i64::try_from(agent.conversation.last_seq).map_err(|_| corrupt())?,
        admitted_at: admitted_at.into(),
        kind,
        writer_gate,
        recovery_gate,
    };
    value.validate()?;
    database
        .collection::<Document>(COLLECTION)
        .insert_one(bson::serialize_to_document(&value).map_err(|_| corrupt())?)
        .session(session)
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                TransactionLogError::Domain(SandboxStoreError::Conflict)
            } else {
                TransactionLogError::Driver(error)
            }
        })?;
    Ok(value)
}

fn closed() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "Agent log publication admission is closed; retain the original operation".into(),
    )
}
