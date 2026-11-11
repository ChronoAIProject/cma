//! A/B real SQLite interoperability, awaiting A's test-only main registration.
//! These small fixtures are not the 1,400-cycle V1/V2 qualification or C exclusion.
#[path = "archive_reader.rs"]
mod archive_reader;
#[path = "storage_fixture.rs"]
mod fixture;
#[path = "types.rs"]
pub mod types;

use fixture::{Fixture, Original};
use types::{Binding, Decision, Digest, Intent, SourceFormat};

fn digest(value: &str) -> Digest {
    assert_eq!(value.len(), 64);
    std::array::from_fn(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).unwrap())
}

fn intent(original: &Original) -> Intent {
    let evidence = original.evidence();
    Intent {
        operation: *uuid::Uuid::parse_str(evidence.operation_id())
            .unwrap()
            .as_bytes(),
        binding: Binding {
            sha256: digest(&original.binding.binding_sha256().unwrap()),
            archive_id: *uuid::Uuid::parse_str(&original.binding.archive_id)
                .unwrap()
                .as_bytes(),
            epoch: original.binding.maintenance_epoch,
            format: SourceFormat::V2,
        },
        group: digest(&original.proof.key),
        before_revision: original.context.revision().unwrap(),
        plan_sha256: digest(&"a".repeat(64)),
        source_sha256: digest(&original.proof.source_sha256),
        observation_sha256: digest(evidence.observation_sha256()),
        evidence_sha256: digest(&evidence.fingerprint()),
        artifact_lengths: original.bytes.each_ref().map(|bytes| bytes.len() as u64),
        decision: Decision::SettledDeleted,
    }
}

#[tokio::test]
async fn actual_v2_reopen_and_later_group_preserve_b_reference_and_final_receipt() {
    let fixture = Fixture::new();
    let mut first = None;
    for (index, name) in ["collector-first", "collector-later"]
        .into_iter()
        .enumerate()
    {
        let original = fixture.original(name, index == 0).await;
        let expected = intent(&original);
        assert!(expected.valid());
        let archive = fixture.archive(index == 0).await;
        let reference = archive
            .retain_collection(original.evidence(), "a".repeat(64))
            .await
            .unwrap();
        assert_eq!(reference, expected.reference());
        archive.close().await.unwrap();
        let archive = fixture.archive(false).await;
        assert_eq!(
            archive
                .pending_collection(original.binding.clone())
                .await
                .unwrap(),
            Some(reference.clone())
        );
        assert_eq!(
            archive_reader::read_originals(&archive, &original.binding, &expected, &reference)
                .await
                .unwrap()
                .originals(),
            original.bytes.each_ref().map(Vec::as_slice)
        );
        assert!(
            archive_reader::completion(&archive, &original.binding, &expected, &reference)
                .await
                .unwrap()
                .is_none()
        );
        archive.close().await.unwrap();
        fixture.complete(&original, reference.clone()).await;
        let archive = fixture.archive(false).await;
        let receipt =
            archive_reader::completion(&archive, &original.binding, &expected, &reference)
                .await
                .unwrap()
                .unwrap();
        assert!(receipt.matches(&expected));
        assert!(receipt.operations > 0 && receipt.authority > 0);
        assert!(archive
            .authorize(std::slice::from_ref(&original.operation))
            .await
            .is_err());
        if index == 0 {
            first = Some((original.binding, reference, expected, receipt));
        }
        if let Some((binding, reference, expected, receipt)) = &first {
            let again = archive_reader::completion(&archive, binding, expected, reference)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&again, receipt);
            assert!(again.matches(expected));
        }
        archive.close().await.unwrap();
        let source = fixture.source(false).await;
        assert_eq!(source.snapshot().await.unwrap().operations, 0);
        source.close().await.unwrap();
    }
}

#[tokio::test]
async fn actual_v2_corrupt_reference_refuses_and_preserves_original_pending_work() {
    let fixture = Fixture::new();
    let original = fixture.original("collector-refusal", true).await;
    let expected = intent(&original);
    let archive = fixture.archive(true).await;
    let reference = archive
        .retain_collection(original.evidence(), "a".repeat(64))
        .await
        .unwrap();
    for case in 0..4 {
        let mut changed = reference.clone();
        match case {
            0 => changed.artifact_lengths[0] += 1,
            1 => changed.plan_sha256 = "b".repeat(64),
            2 => changed.operation_id = uuid::Uuid::from_u128(1).to_string(),
            _ => changed.artifacts_sha256 = "c".repeat(64),
        }
        assert_ne!(changed, expected.reference());
        assert!(
            archive_reader::read_originals(&archive, &original.binding, &expected, &changed)
                .await
                .is_err()
        );
        assert!(
            archive_reader::completion(&archive, &original.binding, &expected, &changed)
                .await
                .is_err()
        );
        assert_eq!(
            archive
                .pending_collection(original.binding.clone())
                .await
                .unwrap(),
            Some(reference.clone())
        );
    }
    archive.close().await.unwrap();
    let source = fixture.source(false).await;
    assert_eq!(source.snapshot().await.unwrap().operations, 1);
    source.close().await.unwrap();
}

#[tokio::test]
async fn actual_v2_completion_reader_rejects_changed_intent_without_changing_outcome() {
    let fixture = Fixture::new();
    let original = fixture.original("collector-cold-read", true).await;
    let expected = intent(&original);
    let archive = fixture.archive(true).await;
    let reference = archive
        .retain_collection(original.evidence(), "a".repeat(64))
        .await
        .unwrap();
    archive.close().await.unwrap();
    fixture.complete(&original, reference.clone()).await;
    let archive = fixture.archive(false).await;
    let receipt = archive_reader::completion(&archive, &original.binding, &expected, &reference)
        .await
        .unwrap()
        .unwrap();
    let source = fixture.source(false).await;
    let source_before = source.snapshot().await.unwrap();
    source.close().await.unwrap();
    for case in 0..13 {
        let mut changed = expected.clone();
        match case {
            0 => changed.source_sha256[0] ^= 1,
            1 => changed.observation_sha256[0] ^= 1,
            2 => changed.evidence_sha256[0] ^= 1,
            3 => changed.before_revision += 1,
            4 => changed.plan_sha256[0] ^= 1,
            5 => changed.operation[0] ^= 1,
            6 => changed.binding.epoch += 1,
            7 => changed.binding.sha256[0] ^= 1,
            8 => changed.binding.archive_id[0] ^= 1,
            9 => changed.group[0] ^= 1,
            10 => changed.artifact_lengths[0] += 1,
            11 => changed.decision = Decision::RetainUnknown,
            _ => changed.binding.format = SourceFormat::V1,
        }
        assert_eq!(
            archive_reader::completion(&archive, &original.binding, &changed, &reference).await,
            Err(types::Refusal::InvalidEvidence),
            "changed intent case {case}"
        );
    }
    let mut changed_binding = original.binding.clone();
    changed_binding.maintenance_epoch += 1;
    assert_eq!(
        archive_reader::completion(&archive, &changed_binding, &expected, &reference).await,
        Err(types::Refusal::InvalidEvidence)
    );
    let mut unretained = expected.clone();
    unretained.operation = *uuid::Uuid::from_u128(1).as_bytes();
    assert_eq!(
        archive_reader::completion(
            &archive,
            &original.binding,
            &unretained,
            &unretained.reference()
        )
        .await,
        Err(types::Refusal::UnknownOutcome),
        "missing originals cannot become an empty completion or a new attempt"
    );
    assert_eq!(
        archive_reader::completion(&archive, &original.binding, &expected, &reference)
            .await
            .unwrap(),
        Some(receipt)
    );
    assert!(archive
        .pending_collection(original.binding.clone())
        .await
        .unwrap()
        .is_none());
    archive.close().await.unwrap();
    let source = fixture.source(false).await;
    assert_eq!(source.snapshot().await.unwrap(), source_before);
    source.close().await.unwrap();
}

#[tokio::test]
async fn actual_archive_reader_refuses_v1_instead_of_relabeling_its_binding() {
    use layer3_adapter::runtime_ledger_maintenance::LedgerSourceFormatV1;
    let fixture = Fixture::new();
    let original = fixture.original("collector-v1-refusal", true).await;
    let archive = fixture.archive(true).await;
    let reference = archive
        .retain_collection(original.evidence(), "a".repeat(64))
        .await
        .unwrap();
    let mut v1_binding = original.binding.clone();
    v1_binding.source_format = LedgerSourceFormatV1::V1;
    let mut v1 = intent(&original);
    v1.binding.sha256 = digest(&v1_binding.binding_sha256().unwrap());
    v1.binding.format = SourceFormat::V1;
    assert_eq!(
        archive_reader::completion(&archive, &v1_binding, &v1, &v1.reference()).await,
        Err(types::Refusal::InvalidEvidence)
    );
    assert_eq!(
        archive.pending_collection(original.binding).await.unwrap(),
        Some(reference)
    );
    archive.close().await.unwrap();
    let source = fixture.source(false).await;
    assert_eq!(source.snapshot().await.unwrap().operations, 1);
    source.close().await.unwrap();
}
