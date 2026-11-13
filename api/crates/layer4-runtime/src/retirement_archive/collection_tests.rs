use super::*;
use crate::ledger_reclamation::CollectionEvidence;
use crate::mutation_ledger::{
    retirement_inventory::Context, retirement_inventory::SourceRow, Ledger,
};
use crate::retirement_archive::collection_transition::{complete_checked, CollectionBoundary};
use layer3_adapter::runtime_ledger_collection::CollectionGroup;
use layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1;

#[path = "collection_crash_tests.rs"]
mod crash;
#[path = "collection_failure_tests.rs"]
mod failures;

struct Intent {
    binding: LedgerBindingV1,
    bytes: [Vec<u8>; 3],
    proof: GroupProof,
    context: Context,
    rows: Vec<SourceRow>,
}
impl Intent {
    fn evidence(&self) -> CollectionEvidence {
        CollectionEvidence::parse(&self.binding, &self.proof.key, self.bytes.clone()).unwrap()
    }
    async fn retain(&self, archive: &Archive) -> CollectionReference {
        archive
            .retain_collection(self.evidence(), "a".repeat(64))
            .await
            .unwrap()
    }
    async fn prepare(&self, archive: &Archive) {
        let rows = self.rows.clone();
        archive
            .prepare_indexed(self.proof.clone(), self.context.clone(), move |i| {
                Ok(rows[i as usize].clone())
            })
            .await
            .unwrap();
    }
}
impl Fixture {
    async fn collection(&self, create: bool) -> Archive {
        Archive::open_version(
            self.path.clone(),
            self.config.clone(),
            self.id.clone(),
            create,
            32_768,
            Version::Collection,
        )
        .await
        .unwrap()
    }
    async fn source(&self, create: bool) -> Ledger {
        Ledger::open(
            self.root.join("active"),
            self.config.clone(),
            create,
            32_768,
        )
        .await
        .unwrap()
    }
    async fn intent(&self, name: &str, create: bool) -> Intent {
        let ledger = self.source(create).await;
        let operation = self.records(name).0;
        ledger.admit(5, vec![operation.clone()]).await.unwrap();
        let entry = ledger.begin(operation.clone()).await.unwrap();
        ledger
            .finish(
                entry
                    .record
                    .completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
                        operation_id: operation.operation_id.clone(),
                        request_sha256: operation.request_sha256.clone(),
                        agent_id: AgentSlot::new(name),
                    })
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut cursor = None;
        let mut context = None;
        let mut rows = Vec::new();
        loop {
            let page = ledger.retirement_inventory(cursor).await.unwrap();
            context.get_or_insert(page.context);
            rows.extend(page.rows);
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        ledger.close().await.unwrap();
        let context = context.unwrap();
        let (binding, bytes) = crate::retirement_archive::collection_probe::artifacts(
            &self.config,
            &self.id,
            context.revision().unwrap(),
            CollectionGroup::Agent {
                agent_id: name.into(),
            },
        )
        .unwrap();
        let evidence =
            CollectionEvidence::parse(&binding, &group_key(&operation), bytes.clone()).unwrap();
        let mut hash = Sha256::new();
        context.hash_into(&mut hash);
        for row in &rows {
            row.hash_into(&mut hash);
        }
        Intent {
            binding,
            bytes,
            context,
            proof: GroupProof {
                key: group_key(&operation),
                source_sha256: format!("{:x}", hash.finalize()),
                observation_sha256: evidence.observation_sha256().into(),
                records: rows.len() as u32,
            },
            rows,
        }
    }
    async fn finish(
        &self,
        intent: &Intent,
        reference: CollectionReference,
    ) -> CollectionCompletion {
        complete_collection_held(
            self.collection(false).await,
            self.source(false).await,
            intent.binding.clone(),
            reference,
            intent.proof.clone(),
            (),
        )
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn collection_exact_bytes_and_original_receipt_survive_restart_and_later_groups() {
    let fixture = Fixture::new();
    let mut original = None;
    for index in 0..8 {
        let intent = fixture.intent(&format!("cycle-{index}"), index == 0).await;
        let archive = fixture.collection(index == 0).await;
        assert!(archive
            .pending_collection(intent.binding.clone())
            .await
            .unwrap()
            .is_none());
        let reference = intent.retain(&archive).await;
        assert!(archive.require_ready().await.is_err());
        archive.close().await.unwrap();
        let archive = fixture.collection(false).await;
        assert_eq!(
            archive
                .read_collection(intent.binding.clone(), reference.clone())
                .await
                .unwrap()
                .originals(),
            intent.bytes.each_ref().map(Vec::as_slice)
        );
        assert_eq!(
            archive
                .pending_collection(intent.binding.clone())
                .await
                .unwrap(),
            Some(reference.clone())
        );
        assert_eq!(intent.retain(&archive).await, reference);
        intent.prepare(&archive).await;
        archive.close().await.unwrap();
        let receipt = fixture.finish(&intent, reference.clone()).await;
        let archive = fixture.collection(false).await;
        archive.require_ready().await.unwrap();
        assert!(archive
            .authorize(&[fixture.records(&format!("cycle-{index}")).0])
            .await
            .is_err());
        archive.close().await.unwrap();
        let source = fixture.source(false).await;
        let snapshot = source.snapshot().await.unwrap();
        assert_eq!(
            (
                snapshot.operations,
                snapshot.authority,
                snapshot.reserved_bytes
            ),
            (0, 0, 0)
        );
        source.close().await.unwrap();
        if index == 0 {
            original = Some((intent, reference, receipt));
        }
    }
    let (intent, reference, expected) = original.unwrap();
    assert_eq!(fixture.finish(&intent, reference.clone()).await, expected);
    let mut wrong_proof = intent.proof.clone();
    wrong_proof.records += 1;
    assert!(complete_collection_held(
        fixture.collection(false).await,
        fixture.source(false).await,
        intent.binding.clone(),
        reference.clone(),
        wrong_proof,
        ()
    )
    .await
    .is_err());
    let archive = fixture.collection(false).await;
    assert_eq!(intent.retain(&archive).await, reference);
    archive.close().await.unwrap();
}

#[tokio::test]
async fn collection_refuses_changed_plan_original_bytes_and_second_pending_operation() {
    let fixture = Fixture::new();
    let intent = fixture.intent("selected", true).await;
    let archive = fixture.collection(true).await;
    let reference = intent.retain(&archive).await;
    for (field, value) in [
        ("format", serde_json::json!("unknown")),
        ("archive_version", serde_json::json!(2)),
        (
            "archive_id",
            serde_json::json!(uuid::Uuid::new_v4().to_string()),
        ),
        ("artifact_lengths", serde_json::json!([1, 1, 1])),
    ] {
        let mut changed = serde_json::to_value(&reference).unwrap();
        changed[field] = value;
        let changed: CollectionReference = serde_json::from_value(changed).unwrap();
        assert!(archive
            .read_collection(intent.binding.clone(), changed)
            .await
            .is_err());
    }

    assert!(archive
        .retain_collection(intent.evidence(), "b".repeat(64))
        .await
        .is_err());
    let mut bytes = intent.bytes.clone();
    bytes[2].extend_from_slice(b"\n");
    let changed = CollectionEvidence::parse(&intent.binding, &intent.proof.key, bytes).unwrap();
    assert!(archive
        .retain_collection(changed, "a".repeat(64))
        .await
        .is_err());
    let (binding, bytes) = crate::retirement_archive::collection_probe::artifacts(
        &fixture.config,
        &fixture.id,
        intent.context.revision().unwrap(),
        CollectionGroup::Agent {
            agent_id: "another".into(),
        },
    )
    .unwrap();
    let second =
        CollectionEvidence::parse(&binding, &fixture.records("another").2.key, bytes).unwrap();
    assert!(archive
        .retain_collection(second, "a".repeat(64))
        .await
        .is_err());
    let mut changed_binding = intent.binding.clone();
    changed_binding.maintenance_epoch += 1;
    assert!(archive
        .read_collection(changed_binding, reference.clone())
        .await
        .is_err());
    assert!(archive.require_ready().await.is_err());
    intent.prepare(&archive).await;
    archive.close().await.unwrap();
    fixture.finish(&intent, reference).await;
}

#[tokio::test]
async fn collection_refuses_archive_selection_without_durable_intent_or_with_changed_revision() {
    for retain in [false, true] {
        let fixture = Fixture::new();
        let mut intent = fixture.intent("selected", true).await;
        let archive = fixture.collection(true).await;
        let (_, legacy_rows, legacy_proof) = fixture.records("selected");
        assert!(archive
            .prepare(legacy_proof.clone(), move |i| Ok(
                legacy_rows[i as usize].clone()
            ))
            .await
            .is_err());
        assert!(archive
            .transfer_group(legacy_proof, Direction::Deactivate, |_| panic!(
                "legacy callback on v3"
            ))
            .await
            .is_err());
        if retain {
            let (binding, bytes) = crate::retirement_archive::collection_probe::artifacts(
                &fixture.config,
                &fixture.id,
                intent.context.revision().unwrap() + 1,
                CollectionGroup::Agent {
                    agent_id: "selected".into(),
                },
            )
            .unwrap();
            intent.binding = binding;
            intent.bytes = bytes;
            intent.proof.observation_sha256 = intent.evidence().observation_sha256().into();
            intent.retain(&archive).await;
        }
        let rows = intent.rows;
        assert!(archive
            .prepare_indexed(intent.proof, intent.context, move |i| Ok(
                rows[i as usize].clone()
            ))
            .await
            .is_err());
        assert_eq!(archive.require_ready().await.is_err(), retain);
        archive.close().await.unwrap();
        let source = fixture.source(false).await;
        assert_eq!(source.snapshot().await.unwrap().operations, 1);
        source.close().await.unwrap();
    }
}

#[tokio::test]
async fn collection_retains_maximum_request_and_observation_without_raising_sqlite_limits() {
    let fixture = Fixture::new();
    let mut intent = fixture.intent("selected", true).await;
    intent.bytes[1].resize(65_536, b' ');
    intent.bytes[2].resize(4 * 1024 * 1024, b' ');
    let archive = fixture.collection(true).await;
    let reference = intent.retain(&archive).await;
    archive.close().await.unwrap();
    let archive = fixture.collection(false).await;
    assert_eq!(
        archive
            .read_collection(intent.binding.clone(), reference.clone())
            .await
            .unwrap()
            .originals(),
        intent.bytes.each_ref().map(Vec::as_slice)
    );
    archive
        .storage
        .call(|db| {
            assert_eq!(
                db.limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH)
                    .unwrap(),
                crate::ledger_storage::MAX_VALUE_BYTES
            );
            Ok(())
        })
        .await
        .unwrap();
    let mut oversize = intent.bytes.clone();
    oversize[2].push(b' ');
    assert!(CollectionEvidence::parse(&intent.binding, &intent.proof.key, oversize).is_err());
    intent.prepare(&archive).await;
    archive.close().await.unwrap();
    fixture.finish(&intent, reference).await;
}
