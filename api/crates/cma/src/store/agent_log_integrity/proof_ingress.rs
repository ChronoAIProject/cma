//! Owned tail conversion and the original exact raw-byte proof check.
use super::*;
use crate::store::ingress_reads::{
    layout::{BsonLayout, LayoutError},
    row::{self, ConversionPlan, ReadError, RowSchema},
};
use layer2_core::sandbox::ingress_ownership::sizing::shared_owner_bytes;

/// Expected tail fields from the caller's integrity metadata, not proof of that
/// metadata's validity or of the absence of rows beyond this tail.
#[derive(Clone, Copy)]
pub(in crate::store) struct ExpectedTail<'a> {
    pub(in crate::store) agent_id: &'a AgentId,
    pub(in crate::store) sequence: u64,
    pub(in crate::store) hash: [u8; HASH_BYTES],
}

type TailRead<'a> = AdmittedOperation<'a, Result<SharedOwned<AgentLogEntry>, ReadFailure>>;

impl MongoSandboxStore {
    /// Read and decode the exact tail in the original session. Driver allocation,
    /// cleanup, and the complete summary still require separate qualification.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "enabled recording awaits driver resource custody")
    )]
    pub(in crate::store) fn integrity_tail_admitted<'a>(
        &'a self,
        expected: crate::store::agent_log_integrity::read::ExpectedTail<'a>,
        session: &'a mut mongodb::ClientSession,
        snapshot: bool,
        policy: &'a IngressOwnershipPolicy,
        read_plan: AllocationPlan,
    ) -> Result<TailRead<'a>, AdmittedError> {
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Transformation,
            AllocationPlan::checked(
                "integrity tail result",
                &[128, shared_owner_bytes::<ReadFailure>()],
            )?,
            move |scope| async move {
                let raw = match self
                    .integrity_point_read_admitted(
                        session,
                        snapshot,
                        false,
                        policy,
                        read_plan,
                        || doc! {"_id":log_id(expected.agent_id, expected.sequence)},
                    )?
                    .await?
                    .transpose()
                {
                    Ok(raw) => raw,
                    Err(error) => return Ok(error.map(Err)),
                };
                let Some(raw) = raw.as_ref() else {
                    return Ok(scope.initialize_owned(|| {
                        Err(ReadFailure::Domain(SandboxStoreError::Corrupt(
                            "Agent log integrity tail row is missing".into(),
                        )))
                    }));
                };
                let entry = match row::decode::<TailSchema>(raw, policy) {
                    Ok(entry) => entry,
                    Err(error) => return error::finish(Err(ReadError::Row(error)), &scope),
                };
                Ok(scope.initialize_owned(|| {
                    validate_tail(
                        &entry,
                        expected.agent_id,
                        expected.sequence,
                        &expected.hash,
                        raw,
                    )
                    .map_err(ReadFailure::Domain)?;
                    Ok(entry)
                }))
            },
        )
    }
}

struct TailSchema;

impl RowSchema for TailSchema {
    type Value = AgentLogEntry;
    const INVALID_RAW: &'static str = "invalid raw integrity tail";

    fn plan(layout: BsonLayout) -> Result<ConversionPlan, LayoutError> {
        // AgentLogEntry bounds vector members. Metadata uses BTreeMap<String,
        // String>; include an internal node's eleven pairs, twelve edges/header.
        let maximum_node =
            size_of::<AgentLogEntry>().max(12 * size_of::<(String, String, usize)>() + 64);
        let retained = layout.domain_bytes(maximum_node, shared_owner_bytes::<AgentLogEntry>())?;
        // read_record clones BSON; serde buffers tagged records and diagnostics.
        // Include BSON/domain overlap, log-id formatting and malformed display.
        let temporary = layout
            .bson_bytes()?
            .checked_mul(3)
            .and_then(|bytes| retained.checked_mul(2)?.checked_add(bytes))
            .and_then(|bytes| layout.wire_bytes.checked_mul(8)?.checked_add(bytes))
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or(LayoutError::Structure)?;
        Ok(ConversionPlan {
            temporary_bytes: temporary,
            retained_bytes: retained,
        })
    }

    fn decode(row: Document) -> Result<AgentLogEntry, SandboxStoreError> {
        read_entry(&row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
    use crate::store::sandbox_mongo::start_transaction;
    use layer2_core::sandbox::ingress_ownership::{ClassBounds, IngressBudget};
    use layer2_core::sandbox::{AgentLogKind, AgentPlanStep};

    fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
        let root = IngressBudget::new(128 * 1024 * 1024);
        let plan = |bytes| AllocationPlan::checked("tail fixture", &[bytes]).unwrap();
        let policy = IngressOwnershipPolicy::new(
            root.clone(),
            ClassBounds::new(
                root.capacity_bytes(),
                plan(1024 * 1024),
                plan(96 * 1024 * 1024),
            )
            .unwrap(),
        )
        .unwrap();
        (root, policy)
    }

    fn entry(kind: AgentLogKind) -> AgentLogEntry {
        AgentLogEntry {
            agent_id: AgentId("agt_tail_fixture".into()),
            seq: 41,
            appended_at: "2026-01-01T00:00:00Z".into(),
            kind,
        }
    }

    fn raw(entry: &AgentLogEntry) -> RawDocumentBuf {
        RawDocumentBuf::try_from(&log_document(entry).unwrap()).unwrap()
    }

    fn expected(entry: &AgentLogEntry) -> ExpectedTail<'_> {
        ExpectedTail {
            agent_id: &entry.agent_id,
            sequence: entry.seq,
            hash: hash_raw(raw(entry).as_bytes()),
        }
    }

    fn read_plan() -> AllocationPlan {
        AllocationPlan::checked("tail source", &[50 * 1024 * 1024]).unwrap()
    }

    #[test]
    fn tail_conversion_retains_strings_vectors_and_metadata_map() {
        let (root, policy) = policy();
        let idle = root.available_bytes();
        for kind in [
            AgentLogKind::AgentMessage {
                content: "a".repeat(65536),
            },
            AgentLogKind::PlanSet {
                entries: (0..64)
                    .map(|n| AgentPlanStep {
                        title: format!("step {n}"),
                        status: "pending".into(),
                    })
                    .collect(),
            },
            AgentLogKind::TurnMetadataSet {
                turn_sequence: 1,
                metadata: (0..64)
                    .map(|n| (format!("key {n}"), "value".repeat(32)))
                    .collect(),
            },
        ] {
            let entry = entry(kind);
            let value = row::decode::<TailSchema>(&raw(&entry), &policy).unwrap();
            assert_eq!(*value, entry);
            assert!(value.belongs_to(&policy));
            let held = root.available_bytes();
            let shared = value.clone();
            drop(value);
            assert_eq!(root.available_bytes(), held);
            drop(shared);
            assert_eq!(root.available_bytes(), idle);
        }
    }

    #[test]
    fn tail_check_preserves_agent_sequence_and_exact_raw_hash() {
        let entry = entry(AgentLogKind::Deleted);
        let raw = raw(&entry);
        let hash = hash_raw(raw.as_bytes());
        assert!(validate_tail(&entry, &entry.agent_id, entry.seq, &hash, &raw).is_ok());
        for (id, seq, digest) in [
            (AgentId("agt_other_tail".into()), entry.seq, hash),
            (entry.agent_id.clone(), entry.seq + 1, hash),
            (entry.agent_id.clone(), entry.seq, [0; HASH_BYTES]),
        ] {
            assert!(matches!(validate_tail(&entry, &id, seq, &digest, &raw),
                Err(SandboxStoreError::Corrupt(message)) if message == "Agent log integrity tail proof failed"));
        }
    }

    #[test]
    fn tail_conversion_refusal_does_not_replace_original_domain_errors() {
        use row::RowError;
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut document = log_document(&entry(AgentLogKind::Deleted)).unwrap();
        document.insert("seq", 900_i64);
        let original = read_entry(&document).unwrap_err().to_string();
        let raw = RawDocumentBuf::try_from(&document).unwrap();
        let pressure = policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("tail Data pressure", &[policy.bounds().data_bytes()])
                    .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            row::decode::<TailSchema>(&raw, &policy),
            Err(RowError::Refused(LayoutError::Capacity))
        ));
        drop(pressure);
        let RowError::Domain(error) = row::decode::<TailSchema>(&raw, &policy).unwrap_err() else {
            panic!("expected original domain error")
        };
        assert_eq!(error.to_string(), original);
        assert!(error.belongs_to(&policy));
        drop(error);
        assert_eq!(root.available_bytes(), idle);
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn admitted_tail_rechecks_each_transaction_row_and_retains_previous_data() {
        let (client, name, store) = database().await;
        let (_, agent) = fixture(&store, "sbx_tail_owned").await;
        let mut first = entry(AgentLogKind::AgentMessage {
            content: "a".repeat(64),
        });
        first.agent_id = agent.id;
        let mut next = first.clone();
        next.kind = AgentLogKind::AgentMessage {
            content: "b".repeat(256 * 1024),
        };
        store
            .agent_log
            .insert_one(log_document(&first).unwrap())
            .await
            .unwrap();
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let old = store
            .integrity_tail_admitted(expected(&first), &mut session, false, &policy, read_plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        store
            .agent_log
            .replace_one(
                doc! {"_id":log_id(&first.agent_id, first.seq)},
                log_document(&next).unwrap(),
            )
            .session(&mut session)
            .await
            .unwrap();
        let new = store
            .integrity_tail_admitted(expected(&next), &mut session, false, &policy, read_plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert_eq!(**old, first);
        assert_eq!(**new, next);
        assert!(new.requested_bytes() > old.requested_bytes());
        let wrong = store
            .integrity_tail_admitted(expected(&first), &mut session, false, &policy, read_plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap_err();
        assert!(
            matches!(&*wrong, ReadFailure::Domain(SandboxStoreError::Corrupt(message)) if message == "Agent log integrity tail proof failed")
        );
        drop(wrong);
        session.abort_transaction().await.unwrap();
        let restored = store
            .integrity_tail_admitted(expected(&first), &mut session, false, &policy, read_plan())
            .unwrap()
            .await
            .unwrap()
            .transpose()
            .unwrap();
        assert_eq!(**restored, first);
        assert!(new.belongs_to(&policy));
        drop((old, new, restored, session));
        assert_eq!(root.available_bytes(), idle);
        client.database(&name).drop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
    async fn admitted_tail_refusal_missing_and_corrupt_do_not_quarantine_or_abort() {
        let (client, name, store) = database().await;
        let (_, agent) = fixture(&store, "sbx_tail_refusal").await;
        let mut entry = entry(AgentLogKind::Deleted);
        entry.agent_id = agent.id.clone();
        let original_agent = store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap();
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let mut session = client.start_session().await.unwrap();
        for fault in ["missing", "data", "domain", "deep"] {
            store
                .agent_log
                .delete_one(doc! {"_id":log_id(&entry.agent_id, entry.seq)})
                .await
                .unwrap();
            let mut document = log_document(&entry).unwrap();
            if fault == "domain" {
                document.insert("seq", 900_i64);
            }
            if fault == "deep" {
                let mut nested = doc! {"leaf":true};
                for _ in 0..66 {
                    nested = doc! {"nested":nested};
                }
                document.insert("ignored", nested);
            }
            if fault != "missing" {
                store.agent_log.insert_one(document.clone()).await.unwrap();
            }
            let pressure = (fault == "data").then(|| {
                policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked(
                            "tail Data pressure",
                            &[policy.bounds().data_bytes()],
                        )
                        .unwrap(),
                    )
                    .unwrap()
            });
            start_transaction(&mut session).await.unwrap();
            let marker = format!("tail-marker-{fault}");
            store
                .agent_log
                .insert_one(doc! {"_id":&marker})
                .session(&mut session)
                .await
                .unwrap();
            let result = store
                .integrity_tail_admitted(
                    expected(&entry),
                    &mut session,
                    false,
                    &policy,
                    read_plan(),
                )
                .unwrap()
                .await;
            match fault {
                "data" => assert!(matches!(result, Err(AdmittedError::Refused(_)))),
                "deep" => assert!(matches!(result, Err(AdmittedError::Unsupported(_)))),
                _ => {
                    let error = result.unwrap().transpose().unwrap_err();
                    assert!(matches!(
                        &*error,
                        ReadFailure::Domain(SandboxStoreError::Corrupt(_))
                    ));
                }
            }
            drop(pressure);
            assert!(store
                .agent_log
                .find_one(doc! {"_id":&marker})
                .session(&mut session)
                .await
                .unwrap()
                .is_some());
            assert!(store
                .agent_log
                .find_one(doc! {"_id":&marker})
                .await
                .unwrap()
                .is_none());
            session.abort_transaction().await.unwrap();
            assert_eq!(root.available_bytes(), idle);
            if fault != "missing" {
                assert_eq!(
                    store
                        .agent_log
                        .find_one(doc! {"_id":log_id(&entry.agent_id, entry.seq)})
                        .await
                        .unwrap(),
                    Some(document)
                );
            }
        }
        assert_eq!(
            store
                .agents
                .find_one(doc! {"_id":agent.id.as_str()})
                .await
                .unwrap(),
            original_agent
        );
        drop(session);
        client.database(&name).drop().await.unwrap();
    }
}
