use super::super::sandbox_mongo::{MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::super::{resource_access, sandbox_events, sandboxes};
use super::{records, ApplyError, Change};
use layer2_core::sandbox::{control::*, store::SandboxStore};
use mongodb::{bson::doc, ClientSession};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    change: &Change<'_>,
    session: &mut ClientSession,
) -> Result<SandboxControlRecord, ApplyError> {
    let (record, proof) = match change {
        Change::Reserve(record, proof, new_commitment) => {
            return reserve(store, record, proof, *new_commitment, session).await
        }
        Change::Apply(record, proof) => (*record, Some(*proof)),
        Change::Reject(record, _) => (*record, None),
    };
    require_queued(store, record, session).await?;
    let at = layer2_core::now_rfc3339();
    let next = match change {
        Change::Reject(_, reason) => record.rejected(*reason, &at)?,
        Change::Apply(..) => {
            let proof = proof.expect("apply carries proof");
            if proof.envelope() != &record.envelope {
                return Err(SandboxControlError::Conflict.into());
            }
            let snapshot =
                resource_access::read_execution_snapshot(store, &record.envelope.target(), session)
                    .await?;
            proof
                .validate_snapshot(&snapshot)
                .map_err(|_| SandboxControlError::Conflict)?;
            // Serialize policy mutation and concurrent resource activity even
            // for an already-satisfied command that only writes a receipt.
            resource_access::fence_lineage(store, &snapshot, session).await?;
            let release = store.released_runtime();
            let plan = plan_sandbox_control(
                record,
                &snapshot.lineage.sandbox,
                &at,
                store.managed_runtime_target(&snapshot.lineage.sandbox, &release.agent),
            )?;
            let plan = if let SandboxControlPlan::Transition { sandbox, .. } = &plan {
                if store
                    .legacy_reuse_blocks_quiescence(&snapshot.lineage.sandbox, sandbox, session)
                    .await?
                {
                    SandboxControlPlan::Reject(SandboxControlFailure::NotEligible)
                } else {
                    plan
                }
            } else {
                plan
            };
            let mut next = record.clone();
            next.revision = 2;
            match plan {
                SandboxControlPlan::Reject(reason) => next = record.rejected(reason, &at)?,
                SandboxControlPlan::NoChange { disposition } => {
                    next.status = SandboxControlStatus::Applied {
                        at: at.clone(),
                        sandbox_revision: snapshot.lineage.sandbox.revision,
                        disposition,
                    };
                    next.execution_audit =
                        Some(proof.audit().map_err(|_| SandboxControlError::Conflict)?);
                }
                SandboxControlPlan::Transition { sandbox, event } => {
                    if snapshot.lineage.sandbox.has_managed_runtime_image()
                        && snapshot.lineage.sandbox.runtime_update_request()
                            != sandbox.runtime_update_request()
                    {
                        store
                            .fence_released_admission(
                                &release,
                                layer2_core::released_runtime::ReleasedLane::Agent,
                                session,
                            )
                            .await
                            .map_err(|error| match error {
                                layer2_core::sandbox::SandboxStoreError::Conflict => {
                                    SandboxControlError::Conflict
                                }
                                _ => SandboxControlError::Unavailable,
                            })?;
                    }
                    let fields = sandboxes::sandbox_replacement_fields(&sandbox)
                        .map_err(|_| SandboxControlError::Corrupt)?;
                    let result = store.sandboxes.update_one(
                        crate::store::work_schema::query(doc! { "channel_reuse_cleanup_execution": { "$exists": false }, "_id": sandbox.id.as_str(), "revision": snapshot.lineage.sandbox.revision as i64, "document_schema_version": DOCUMENT_SCHEMA_VERSION }),
                        crate::store::work_lease_update::replacement(fields),
                    ).session(&mut *session).await?;
                    if result.matched_count != 1 {
                        return Err(SandboxControlError::Conflict.into());
                    }
                    crate::store::sandbox_usage::save::record_saved(
                        store,
                        session,
                        &sandbox,
                        snapshot.lineage.sandbox.revision,
                    )
                    .await
                    .map_err(|error| match error {
                        sandbox_events::TransactionEventError::Driver(error) => {
                            ApplyError::Driver(error)
                        }
                        sandbox_events::TransactionEventError::Domain(_) => {
                            ApplyError::Domain(SandboxControlError::Corrupt)
                        }
                    })?;
                    sandbox_events::append_in_transaction(
                        store,
                        session,
                        &sandbox.id,
                        at.clone(),
                        event,
                    )
                    .await
                    .map_err(|error| match error {
                        sandbox_events::TransactionEventError::Driver(error) => {
                            ApplyError::Driver(error)
                        }
                        sandbox_events::TransactionEventError::Domain(_) => {
                            ApplyError::Domain(SandboxControlError::Corrupt)
                        }
                    })?;
                    next.status = SandboxControlStatus::Applied {
                        at,
                        sandbox_revision: sandbox.revision,
                        disposition: SandboxControlDisposition::Scheduled,
                    };
                    next.execution_audit =
                        Some(proof.audit().map_err(|_| SandboxControlError::Conflict)?);
                }
            }
            next
        }
        Change::Reserve(..) => unreachable!(),
    };
    next.validate()?;
    let replaced = store
        .sandbox_controls
        .replace_one(
            doc! { "_id": record.envelope.command_id.as_str(), "revision": 1_i64, "queued": true },
            records::document(&next)?,
        )
        .session(session)
        .await?;
    if replaced.matched_count != 1 {
        return Err(SandboxControlError::Conflict.into());
    }
    Ok(next)
}

async fn reserve(
    store: &MongoSandboxStore,
    record: &SandboxControlRecord,
    proof: &SandboxControlAuthorization,
    new_commitment: Result<(), layer2_core::admission::AdmissionRefusal>,
    session: &mut ClientSession,
) -> Result<SandboxControlRecord, ApplyError> {
    record.validate()?;
    if !record.queued() || proof.envelope() != &record.envelope {
        return Err(SandboxControlError::Invalid.into());
    }
    if record.admission_audit != proof.audit().map_err(|_| SandboxControlError::Conflict)? {
        return Err(SandboxControlError::Conflict.into());
    }
    let snapshot =
        resource_access::read_execution_snapshot(store, &record.envelope.target(), session).await?;
    proof
        .validate_snapshot(&snapshot)
        .map_err(|_| SandboxControlError::Conflict)?;
    resource_access::fence_lineage(store, &snapshot, session).await?;
    if let Some(previous) = store
        .sandbox_controls
        .find_one(doc! { "scope": record.envelope.scope(), "key_sha256": &record.key_sha256 })
        .session(&mut *session)
        .await?
    {
        return Ok(record.replay(records::read(&previous)?)?);
    }
    new_commitment.map_err(SandboxControlError::from)?;
    if record.envelope.expected_revision != snapshot.lineage.sandbox.revision
        || record.expired(&layer2_core::now_rfc3339())?
    {
        return Err(SandboxControlError::Conflict.into());
    }
    if let Some(pending) = store
        .sandbox_controls
        .find_one(doc! { "sandbox_id": record.envelope.sandbox_id.as_str(), "queued": true })
        .session(&mut *session)
        .await?
    {
        records::read(&pending)?;
        return Err(SandboxControlError::Busy.into());
    }
    if store
        .sandbox_controls
        .find_one(doc! { "_id": record.envelope.command_id.as_str() })
        .session(&mut *session)
        .await?
        .is_some()
    {
        return Err(SandboxControlError::Conflict.into());
    }
    store
        .sandbox_controls
        .insert_one(records::document(record)?)
        .session(session)
        .await?;
    Ok(record.clone())
}

async fn require_queued(
    store: &MongoSandboxStore,
    expected: &SandboxControlRecord,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    expected.validate()?;
    if !expected.queued() {
        return Err(SandboxControlError::Conflict.into());
    }
    let actual = store
        .sandbox_controls
        .find_one(doc! { "_id": expected.envelope.command_id.as_str() })
        .session(session)
        .await?
        .ok_or(SandboxControlError::Conflict)?;
    if &records::read(&actual)? != expected {
        return Err(SandboxControlError::Conflict.into());
    }
    Ok(())
}
