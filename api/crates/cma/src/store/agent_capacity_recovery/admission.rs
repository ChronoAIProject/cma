//! Exact storage boundary. The caller owns the existing transaction and MUST
//! abort it on every error; no recovery effect is permitted until commit proof.
use layer2_core::resource_access::AuthorizedResource;
use layer2_core::sandbox::{
    agent_capacity_recovery::state::{RecoveryRequest, RecoveryWork},
    store::SandboxStoreError,
    Agent,
};
use mongodb::{
    bson::{spec::BinarySubtype, Binary},
    ClientSession,
};

use super::{authority, records, source};
use crate::store::{
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    agent_log_segments::{
        capture::{self, CaptureMode, CapturedFormat},
        publication::{self, Publication, PublicationIdentity},
        writer_admission::{Admission, AdmissionKind},
        writer_bootstrap::BootstrapPlan,
        writer_commit::{self, AggregateUpdate, PublicationInput},
    },
    agents::agent_replacement_fields,
    sandbox_mongo::MongoSandboxStore,
};

pub(in crate::store) struct AdmissionRequest<'a> {
    pub authorized: &'a AuthorizedResource,
    pub request: &'a RecoveryRequest,
    pub admission: &'a Admission,
    pub bootstrap: Option<&'a BootstrapPlan>,
    pub now: &'a str,
}

pub(in crate::store) struct AdmittedRecovery {
    pub work: RecoveryWork,
    pub agent: Agent,
    pub publication: Publication,
}

/// The separate process gate is checked by the admission service. This helper
/// still requires S4's exact CapacityRecovery admission with BOTH durable gates;
/// an ordinary publication admission cannot grant recovery.
pub(in crate::store) async fn persist_admission(
    store: &MongoSandboxStore,
    input: AdmissionRequest<'_>,
    session: &mut ClientSession,
) -> Result<AdmittedRecovery, TransactionLogError> {
    let current =
        authority::capture(store, input.authorized, &input.request.agent_id, session).await?;
    let context = current.context()?;
    let captured =
        capture::capture(store, context.agent, session, CaptureMode::Transaction).await?;
    if captured.format() == CapturedFormat::Legacy && input.bootstrap.is_none() {
        return Err(SandboxStoreError::Unavailable(
            "capacity recovery requires a certified staged legacy generation".into(),
        )
        .into());
    }
    let failures = capture::points(
        store,
        &captured,
        &[input.request.failure_seq],
        session,
        CaptureMode::Transaction,
    )
    .await?;
    let [failure] = failures.as_slice() else {
        return Err(SandboxStoreError::Conflict.into());
    };
    let plan = context
        .plan(failure, &input.request.requested_at)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let work = RecoveryWork::prepare(input.request.clone(), &plan, &context)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let next = plan
        .fenced_agent(&context, input.now)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let identity = work
        .admission_identity()
        .map_err(|_| SandboxStoreError::Conflict)?;
    let operation = PublicationIdentity {
        key: identity.key,
        fingerprint: Binary {
            subtype: BinarySubtype::Generic,
            bytes: identity.fingerprint.to_vec(),
        },
        appended_at: identity.appended_at,
    };
    input
        .admission
        .permits(context.agent, &operation, AdmissionKind::CapacityRecovery)?;
    let prepared = writer_commit::prepare(
        store,
        session,
        &captured,
        PublicationInput {
            operation: &operation,
            current: context.agent,
            next: &next,
            logs: &[],
            capacity: AppendCapacity::Normal,
            result_digest: publication::result_digest(&(&next, &work))?,
        },
        input.bootstrap,
        Some(input.admission),
    )
    .await?;
    let filter = source::fence_filter(store, context.agent, session).await?;
    // No user input or synthetic event is materialized. A carrier/parent/CAS
    // failure aborts the same transaction as the seal, successor and Publication.
    records::insert(store, &work, session).await?;
    authority::fence_parents(store, &current, session).await?;
    authority::require(input.authorized)?;
    let publication = writer_commit::persist(
        store,
        session,
        prepared,
        AggregateUpdate {
            filter,
            fields: agent_replacement_fields(&next)?,
        },
    )
    .await?;
    Ok(AdmittedRecovery {
        work,
        agent: next,
        publication,
    })
}
