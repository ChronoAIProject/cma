//! Admitted read-only integrity proof composition.
use super::*;

/// Borrowed inputs retain their caller's custody and snapshot provenance. This
/// read-only proof never starts an audit, quarantine, commit, or abort.
pub(in crate::store) fn read_only_summary_admitted<'a>(
    store: &'a MongoSandboxStore,
    agent: &'a Agent,
    raw: &'a mongodb::bson::RawDocument,
    session: &'a mut mongodb::ClientSession,
    snapshot: bool,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<SummaryRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked(
            "integrity summary result",
            &[128, shared_owner_bytes::<ReadFailure>()],
        )?,
        move |scope| async move {
            let metadata = match row::decode::<proof::SummarySchema>(raw, policy) {
                Ok(value) => value,
                Err(error) => return error::finish(Err(ReadError::Row(error)), &scope),
            };
            let Some(integrity) = metadata.as_ref() else {
                if agent.conversation.last_seq == 0 {
                    let extra = match store
                        .integrity_point_read_admitted(
                            session,
                            snapshot,
                            true,
                            policy,
                            read_plan,
                            || doc! {"agent_id":agent.id.as_str()},
                        )?
                        .await?
                        .transpose()
                    {
                        Ok(value) => value,
                        Err(error) => return Ok(error.map(Err)),
                    };
                    if extra.is_some() {
                        return Ok(scope.initialize_owned(|| {
                            Err(ReadFailure::Domain(SandboxStoreError::Corrupt(
                                "legacy zero-head Agent has retained log rows".into(),
                            )))
                        }));
                    }
                }
                return Ok(scope.initialize_owned(|| Ok(None)));
            };
            let validation = IngressWorkScope::new(policy.admit(
                IngressClass::Transformation,
                proof::summary_validation_plan(agent)?,
            )?);
            let stale = match validation
                .initialize_owned(|| {
                    proof::validate_metadata(agent, integrity).map_err(ReadFailure::Domain)
                })
                .transpose()
            {
                Ok(value) => *value,
                Err(error) => return Ok(error.map(Err)),
            };
            drop(validation);
            if integrity.head_seq > 0 {
                if let Err(error) = store
                    .integrity_tail_admitted(
                        ExpectedTail {
                            agent_id: &agent.id,
                            sequence: integrity.head_seq,
                            hash: integrity.head_hash,
                        },
                        session,
                        snapshot,
                        policy,
                        read_plan,
                    )?
                    .await?
                    .transpose()
                {
                    return Ok(error.map(Err));
                }
            }
            if stale {
                return Ok(scope.initialize_owned(|| Ok(None)));
            }
            let extra = match store
                .integrity_point_read_admitted(session, snapshot, true, policy, read_plan, || {
                    let mut filter = doc! {"agent_id":agent.id.as_str()};
                    if integrity.head_seq > 0 {
                        filter.insert("seq", doc! {"$gt":integrity.head_seq as i64});
                    }
                    filter
                })?
                .await?
                .transpose()
            {
                Ok(value) => value,
                Err(error) => return Ok(error.map(Err)),
            };
            if extra.is_some() {
                return Ok(scope.initialize_owned(|| {
                    Err(ReadFailure::Domain(SandboxStoreError::Corrupt(
                        "Agent log has rows beyond its trusted head".into(),
                    )))
                }));
            }
            if !integrity.semantic.valid {
                return Ok(scope.initialize_owned(|| Ok(None)));
            }
            let summary = proof::copy_summary(&metadata, policy)?;
            Ok(scope.initialize_owned(|| Ok(Some(summary))))
        },
    )
}

use crate::store::ingress_reads::{
    layout::{BsonLayout, LayoutError},
    row::{ConversionPlan, RowSchema},
};
use layer2_core::sandbox::ingress_ownership::OwnedWork;

type ProvenRead<'a> =
    AdmittedOperation<'a, Result<Option<(SharedOwned<Agent>, bool)>, ReadFailure>>;
type LegacyRead<'a> = AdmittedOperation<'a, Result<bool, ReadFailure>>;
type CompleteRead<'a> = AdmittedOperation<'a, Result<LogSummary, ReadFailure>>;

pub(in crate::store) struct ProofInput<'a> {
    pub(in crate::store) agent: &'a Agent,
    pub(in crate::store) raw: &'a mongodb::bson::RawDocument,
}

/// Retain the primary proof result while attempting known-corruption diagnostics.
/// Secondary failures never replace that result. The caller retains input/session
/// provenance and owns any transaction abort or quarantine decision.
pub(in crate::store) fn validate_summary_admitted<'a>(
    store: &'a MongoSandboxStore,
    input: ProofInput<'a>,
    caller: ScanCaller,
    session: &'a mut mongodb::ClientSession,
    snapshot: bool,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<SummaryRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked("integrity proof diagnostics", &[])?,
        move |_scope| async move {
            let result = read_only_summary_admitted(
                store,
                input.agent,
                input.raw,
                session,
                snapshot,
                policy,
                read_plan,
            )?
            .await?;
            // The proof's outer success has already preflighted the immutable
            // raw BSON. Check key presence, including a false quarantine value.
            if matches!(
                &*result,
                Err(ReadFailure::Domain(SandboxStoreError::Corrupt(_)))
            ) && matches!(input.raw.get(QUARANTINE_FIELD), Ok(None))
            {
                if let Ok(scan) = scan::cursor::scan_corruption_admitted(
                    caller,
                    scan::cursor::ScanInput {
                        collection: &store.agent_log,
                        agent_id: &input.agent.id,
                        last_seq: input.agent.conversation.last_seq,
                    },
                    session,
                    ScanOptions::new(snapshot, store.scan_cooperation),
                    policy,
                    read_plan,
                ) {
                    let _ = scan.await;
                }
            }
            Ok(result)
        },
    )
}

/// Recheck the caller's Agent and proof in the supplied transaction, then scan
/// every row and validate creation lineage. Borrowed input/session provenance
/// and the source plan remain caller obligations; this grants no write authority.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "enabled recording awaits complete ingress composition"
    )
)]
pub(in crate::store) fn scan_complete_log_with_session_admitted<'a>(
    caller: ScanCaller,
    store: &'a MongoSandboxStore,
    agent: &'a Agent,
    session: &'a mut mongodb::ClientSession,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<CompleteRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked(
            "read-only complete assessment",
            &[128, shared_owner_bytes::<ReadFailure>()],
        )?,
        move |scope| async move {
            let input = match store
                .integrity_agent_read_admitted(&agent.id, session, false, policy, read_plan)?
                .await?
                .transpose()
            {
                Ok(input) => input,
                Err(error) => return Ok(error.map(Err)),
            };
            let Some(input) = input.as_ref() else {
                return Ok(scope
                    .initialize_owned(|| Err(ReadFailure::Domain(SandboxStoreError::NotFound))));
            };
            if *input.agent != *agent {
                return Ok(scope
                    .initialize_owned(|| Err(ReadFailure::Domain(SandboxStoreError::Conflict))));
            }
            if let Err(error) = read_only_summary_admitted(
                store, agent, &input.raw, session, false, policy, read_plan,
            )?
            .await?
            .transpose()
            {
                return Ok(error.map(Err));
            }
            scan::cursor::scan_collection_admitted(
                caller,
                scan::cursor::ScanInput {
                    collection: &store.agent_log,
                    agent_id: &agent.id,
                    last_seq: agent.conversation.last_seq,
                },
                session,
                ScanOptions::new(false, store.scan_cooperation),
                policy,
                read_plan,
                |entry| {
                    scope
                        .initialize_owned(|| validate_creation_lineage(agent, &entry))
                        .transpose()
                        .map(|_| ())
                },
            )?
            .await
        },
    )
}

/// The returned Agent keeps its Data owner. The boolean has the legacy reader's
/// meaning and does not replace complete fallback or transaction provenance.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "enabled recording awaits complete ingress composition"
    )
)]
pub(in crate::store) fn read_proven_agent_admitted<'a>(
    store: &'a MongoSandboxStore,
    id: &'a AgentId,
    session: &'a mut mongodb::ClientSession,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<ProvenRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked(
            "integrity proven Agent result",
            &[128, shared_owner_bytes::<ReadFailure>()],
        )?,
        move |scope| async move {
            let input = match store
                .integrity_agent_read_admitted(id, session, true, policy, read_plan)?
                .await?
                .transpose()
            {
                Ok(input) => input,
                Err(error) => return Ok(error.map(Err)),
            };
            let Some(input) = input.as_ref() else {
                return Ok(scope.initialize_owned(|| Ok(None)));
            };
            let flags = match row::decode::<ReaderFlagsSchema<true>>(&input.raw, policy) {
                Ok(flags) => flags,
                Err(error) => return error::finish(Err(ReadError::Row(error)), &scope),
            };
            if flags.segmented {
                return Ok(scope.initialize_owned(|| {
                    Err(ReadFailure::Domain(SandboxStoreError::Unavailable(
                        "segmented history requires a bounded reader".into(),
                    )))
                }));
            }
            let trusted = match prove_legacy(
                store,
                LegacyInput {
                    agent: &input.agent,
                    raw: &input.raw,
                    quarantined: flags.quarantined,
                },
                session,
                true,
                policy,
                read_plan,
            )
            .await?
            .transpose()
            {
                Ok(trusted) => *trusted,
                Err(error) => return Ok(error.map(Err)),
            };
            Ok(scope.initialize_owned(|| Ok(Some((input.agent.clone(), trusted)))))
        },
    )
}

/// Borrowed inputs keep their caller's custody and snapshot/transaction identity.
/// Missing format fields remain valid for this metadata-only legacy contract.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "enabled recording awaits complete ingress composition"
    )
)]
pub(in crate::store) fn legacy_proof_for_context_admitted<'a>(
    store: &'a MongoSandboxStore,
    agent: &'a Agent,
    raw: &'a mongodb::bson::RawDocument,
    session: &'a mut mongodb::ClientSession,
    snapshot: bool,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<LegacyRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked("legacy integrity proof operation", &[])?,
        move |scope| async move {
            let flags = match row::decode::<ReaderFlagsSchema<false>>(raw, policy) {
                Ok(flags) => flags,
                Err(error) => return error::finish(Err(ReadError::Row(error)), &scope),
            };
            prove_legacy(
                store,
                LegacyInput {
                    agent,
                    raw,
                    quarantined: flags.quarantined,
                },
                session,
                snapshot,
                policy,
                read_plan,
            )
            .await
        },
    )
}

struct LegacyInput<'a> {
    agent: &'a Agent,
    raw: &'a mongodb::bson::RawDocument,
    quarantined: bool,
}

async fn prove_legacy(
    store: &MongoSandboxStore,
    input: LegacyInput<'_>,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
    policy: &IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<OwnedWork<Result<bool, ReadFailure>>, AdmittedError> {
    if store.bounded_log_enabled(&input.agent.id) && input.quarantined {
        fleet::telemetry::record(fleet::telemetry::Event::Quarantined);
    }
    let result = super::read_only_summary_admitted(
        store,
        input.agent,
        input.raw,
        session,
        snapshot,
        policy,
        read_plan,
    )?
    .await?;
    Ok(result.map(|result| {
        result.map(|proof| proof.is_some() && store.bounded_log_enabled(&input.agent.id))
    }))
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::store::agent_log_integrity) struct ReaderFlags {
    pub(in crate::store::agent_log_integrity) segmented: bool,
    quarantined: bool,
}

pub(in crate::store::agent_log_integrity) struct ReaderFlagsSchema<const CHECK_FORMAT: bool>;

impl<const CHECK_FORMAT: bool> RowSchema for ReaderFlagsSchema<CHECK_FORMAT> {
    type Value = ReaderFlags;
    const INVALID_RAW: &'static str = "invalid raw integrity reader flags";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        // Materialized BSON and fixed original envelope-validation errors.
        let temporary_bytes = layout
            .bson_bytes()?
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(1024))
            .ok_or(LayoutError::Structure)?;
        Ok(ConversionPlan {
            temporary_bytes,
            retained_bytes: shared_owner_bytes::<ReaderFlags>(),
        })
    }

    fn decode(row: Document) -> Result<ReaderFlags, SandboxStoreError> {
        Ok(ReaderFlags {
            segmented: CHECK_FORMAT
                && crate::store::agent_log_segments::read_format(&row)?.is_some(),
            quarantined: row.contains_key(QUARANTINE_FIELD),
        })
    }
}

#[cfg(test)]
#[path = "read_ingress_tests.rs"]
mod tests;
