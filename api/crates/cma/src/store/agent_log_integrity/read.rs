//! Read-only integrity proof for public log readers and complete assessments.
use super::*;
use mongodb::options::ReadConcern;

/// Complete assessment inside the caller's transaction. Retain the shared
/// quarantine/metadata/tail proof and creation lineage without audit side effects.
pub(in crate::store) async fn scan_complete_log_with_session(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<LogSummary, TransactionLogError> {
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    if crate::store::agents::read_agent(&row)? != *agent {
        return Err(SandboxStoreError::Conflict.into());
    }
    proof::read_only_summary(store, agent, &row, session, false).await?;
    scan::scan_collection(
        caller,
        &store.agent_log,
        &agent.id,
        agent.conversation.last_seq,
        session,
        crate::store::agent_log_integrity::ScanOptions::new(false, store.scan_cooperation),
        |entry| {
            if let layer2_core::sandbox::AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } = &entry.kind
            {
                if entry.seq != 1
                    || workspace_id != &agent.workspace_id
                    || worktree_id != agent.creation_worktree_id()
                {
                    return Err(SandboxStoreError::Corrupt(
                        "Agent creation log lineage does not match".into(),
                    ));
                }
            }
            Ok(())
        },
    )
    .await
}

#[derive(Debug, PartialEq, Eq)]
#[cfg(test)]
pub(in crate::store) struct ReadHead {
    pub(in crate::store) last_seq: u64,
    pub(in crate::store) trusted: bool,
}

/// Use the supplied snapshot for every query. Absence is distinct from legacy
/// or disabled evidence. Corruption remains an error but never initiates repair,
/// quarantine, an audit, or a transaction operation from a public read.
#[cfg(test)]
pub(in crate::store) async fn read_head(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Option<ReadHead>, SandboxStoreError> {
    Ok(read_proven_agent(store, agent_id, session)
        .await?
        .map(|(agent, trusted)| ReadHead {
            last_seq: agent.conversation.last_seq,
            trusted,
        }))
}

/// Retain the validated Agent for a caller's read-only complete fallback. This
/// avoids rereading the envelope or invoking the writer/audit quarantine path.
pub(in crate::store) async fn read_proven_agent(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Option<(Agent, bool)>, SandboxStoreError> {
    let row = store
        .agents
        .find_one(doc! { "_id": agent_id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let agent = crate::store::agents::read_agent(&row)?;
    if crate::store::agent_log_segments::read_format(&row)?.is_some() {
        return Err(SandboxStoreError::Unavailable(
            "segmented history requires a bounded reader".into(),
        ));
    }
    let trusted = legacy_proof(store, &agent, &row, session).await?;
    Ok(Some((agent, trusted)))
}

pub(in crate::store) async fn legacy_proof(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut mongodb::ClientSession,
) -> Result<bool, SandboxStoreError> {
    legacy_proof_for_context(store, agent, row, session, true).await
}

pub(in crate::store) async fn legacy_proof_for_context(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
) -> Result<bool, SandboxStoreError> {
    let agent_id = &agent.id;
    if store.bounded_log_enabled(agent_id) && row.contains_key(QUARANTINE_FIELD) {
        fleet::telemetry::record(fleet::telemetry::Event::Quarantined);
    }
    let proof = proof::read_only_summary(store, agent, row, session, snapshot)
        .await
        .map_err(|error| match error {
            TransactionLogError::Domain(error) => error,
            TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
        })?;
    let trusted = proof.is_some() && store.bounded_log_enabled(agent_id);
    Ok(trusted)
}

pub(in crate::store) use super::proof::ingress::ExpectedTail;

use crate::store::ingress_reads::{
    error::{self, ReadFailure},
    row::{self, ReadError},
};
use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, IngressWorkScope, SharedOwned,
};

pub(super) type SummaryRead<'a> =
    AdmittedOperation<'a, Result<Option<SharedOwned<LogSummary>>, ReadFailure>>;

/// Borrowed inputs retain their caller's custody and snapshot provenance. This
/// read-only proof never starts an audit, quarantine, commit, or abort.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "enabled recording awaits driver resource custody")
)]
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

#[cfg(test)]
mod summary_tests {
    use super::super::proof::summary_tests::{metadata, policy, read};
    use super::*;
    use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
    use crate::store::sandbox_mongo::start_transaction;

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn admitted_summary_matches_legacy_metadata_tail_and_fallback_order() {
        let (client, name, store) = database().await;
        let (_, mut agent) = fixture(&store, "sbx_summary_matrix").await;
        let original = store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap();
        agent.conversation.conversation_id = None;
        agent.conversation.runtime_generation = None;
        let entry = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: 1,
            appended_at: "2026-01-01T00:00:00Z".into(),
            kind: layer2_core::sandbox::AgentLogKind::Deleted,
        };
        let tail = log_document(&entry).unwrap();
        let hash = hash_raw(RawDocumentBuf::try_from(&tail).unwrap().as_bytes());
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut session = client.start_session().await.unwrap();
        for case in [
            "valid",
            "empty",
            "absent",
            "future",
            "zero_legacy",
            "missing",
            "hash",
            "stale",
            "stale_missing",
            "invalid",
            "invalid_extra",
            "extra",
            "ahead",
            "revision",
            "identity",
            "count",
            "bytes",
            "zero_hash",
            "through",
            "semantic",
            "quarantine",
        ] {
            store
                .agent_log
                .delete_many(doc! {"agent_id":agent.id.as_str()})
                .await
                .unwrap();
            agent.conversation.last_seq = if matches!(case, "empty" | "zero_legacy" | "zero_hash") {
                0
            } else {
                1
            };
            let mut metadata = metadata(
                &agent,
                if agent.conversation.last_seq == 0 {
                    [0; HASH_BYTES]
                } else {
                    hash
                },
            );
            match case {
                "stale" | "stale_missing" => metadata.bound_agent_revision = agent.revision - 1,
                "invalid" | "invalid_extra" => metadata.semantic.valid = false,
                "hash" | "zero_hash" => metadata.head_hash = [1; HASH_BYTES],
                "ahead" => metadata.head_seq += 1,
                "revision" => metadata.bound_agent_revision += 1,
                "identity" => metadata.history_identity = [1; HASH_BYTES],
                "count" => metadata.entry_count += 1,
                "bytes" => metadata.encoded_bytes = PHYSICAL_AGENT_LOG_BYTES + 1,
                "through" => metadata.semantic.through_seq += 1,
                "semantic" => metadata.semantic.active_turn_sequence = Some(1),
                _ => {}
            }
            let mut row = doc! {INTEGRITY_FIELD:metadata.to_document().unwrap()};
            if matches!(case, "absent" | "zero_legacy") {
                row.remove(INTEGRITY_FIELD);
            }
            if case == "future" {
                row.get_document_mut(INTEGRITY_FIELD)
                    .unwrap()
                    .insert("format", 2_i64);
            }
            if case == "quarantine" {
                row.insert(QUARANTINE_FIELD, true);
            }
            if !matches!(case, "empty" | "missing" | "stale_missing" | "zero_hash") {
                store.agent_log.insert_one(tail.clone()).await.unwrap();
            }
            if matches!(case, "extra" | "invalid_extra" | "stale") {
                store
                    .agent_log
                    .insert_one(
                        doc! {"_id":"summary-extra", "agent_id":agent.id.as_str(), "seq":2_i64},
                    )
                    .await
                    .unwrap();
            }
            start_transaction(&mut session).await.unwrap();
            store
                .agent_log
                .insert_one(doc! {"_id":"summary-marker"})
                .session(&mut session)
                .await
                .unwrap();
            let legacy = proof::read_only_summary(&store, &agent, &row, &mut session, false).await;
            assert_eq!(
                matches!(&legacy, Ok(Some(_))),
                matches!(case, "valid" | "empty")
            );
            let raw = RawDocumentBuf::try_from(&row).unwrap();
            let actual = read(&store, &agent, &raw, &mut session, false, &policy)
                .unwrap()
                .await
                .unwrap()
                .transpose();
            match (legacy, actual) {
                (Ok(expected), Ok(actual)) => {
                    assert_eq!(expected.as_ref(), actual.as_ref().map(|v| &**v), "{case}")
                }
                (Err(TransactionLogError::Domain(expected)), Err(actual)) => {
                    assert!(
                        matches!(&*actual, ReadFailure::Domain(e) if *e == expected),
                        "{case}"
                    );
                }
                other => panic!("summary outcome changed for {case}: {other:?}"),
            }
            proof::ingress::assert_open_transaction(&store, &mut session, "summary-marker").await;
            session.abort_transaction().await.unwrap();
            assert_eq!(root.available_bytes(), idle);
        }
        assert_eq!(
            store
                .agents
                .find_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap(),
            original
        );
        client.database(&name).drop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn admitted_summary_retains_snapshot_and_refuses_without_transaction_effects() {
        let (client, name, store) = database().await;
        let (_, mut agent) = fixture(&store, "sbx_summary_snapshot").await;
        agent.conversation.last_seq = 0;
        agent.conversation.conversation_id = None;
        agent.conversation.runtime_generation = None;
        store
            .agent_log
            .delete_many(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap();
        let row = doc! {INTEGRITY_FIELD:metadata(&agent, [0; HASH_BYTES]).to_document().unwrap()};
        let raw = RawDocumentBuf::try_from(&row).unwrap();
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut pinned = client.start_session().snapshot(true).await.unwrap();
        let old = read(&store, &agent, &raw, &mut pinned, true, &policy)
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert!(old.is_some());
        let timestamp = pinned.snapshot_time();
        store
            .agent_log
            .insert_one(doc! {"_id":"later-summary-row", "agent_id":agent.id.as_str()})
            .await
            .unwrap();
        let same = read(&store, &agent, &raw, &mut pinned, true, &policy)
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert!(same.is_some());
        assert_eq!(pinned.snapshot_time(), timestamp);
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        store
            .agent_log
            .insert_one(doc! {"_id":"summary-refusal-marker"})
            .session(&mut session)
            .await
            .unwrap();
        for class in [IngressClass::Data, IngressClass::Transformation] {
            let available = policy.budget(class).available_bytes();
            let pressure = policy
                .admit(
                    class,
                    AllocationPlan::checked("summary pressure", &[available]).unwrap(),
                )
                .unwrap();
            let operation = read(&store, &agent, &raw, &mut session, false, &policy);
            match operation {
                Ok(operation) => assert!(matches!(operation.await, Err(AdmittedError::Refused(_)))),
                Err(error) => assert!(matches!(error, AdmittedError::Refused(_))),
            }
            drop(pressure);
        }
        let error = read(&store, &agent, &raw, &mut session, false, &policy)
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err();
        assert!(
            matches!(&*error, ReadFailure::Domain(SandboxStoreError::Corrupt(message)) if message == "Agent log has rows beyond its trusted head")
        );
        drop(error);
        proof::ingress::assert_open_transaction(&store, &mut session, "summary-refusal-marker")
            .await;
        session.abort_transaction().await.unwrap();
        drop((old, same, pinned, session));
        assert_eq!(root.available_bytes(), idle);
        client.database(&name).drop().await.unwrap();
    }
}
