//! A/B real SQLite interoperability, awaiting A's test-only main registration.
//! These small fixtures are not the 1,400-cycle V1/V2 qualification or C exclusion.
#[path = "storage_fixture.rs"]
mod fixture;
#[path = "types.rs"]
pub mod types;

use fixture::{Fixture, Original};
use types::{Binding, Decision, Digest, Intent, Phase, Receipt, SourceFormat};

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

// A's Archive::collection_completion already checked the original encoding,
// reference, checksum and source proof before this private B projection exists.
fn project(cold: crate::retirement_archive::CollectionCompletion) -> Receipt {
    assert_eq!(cold.format, "runtime-ledger-transition-receipt-v1");
    assert_eq!(cold.phase, "acknowledged");
    Receipt {
        version: 1,
        binding_sha256: digest(&cold.binding_sha256),
        operation: *uuid::Uuid::parse_str(&cold.operation_id)
            .unwrap()
            .as_bytes(),
        plan_sha256: digest(&cold.plan_sha256),
        group: digest(&cold.group_key),
        source_sha256: digest(&cold.source_sha256),
        observation_sha256: digest(&cold.observation_sha256),
        before_sha256: digest(&cold.before_sha256),
        after_sha256: digest(&cold.after_sha256),
        before_revision: cold.before_revision,
        after_revision: cold.after_revision,
        operations: cold.operations,
        authority: cold.authority,
        cleanup: cold.cleanup,
        free_pages_before: cold.free_pages_before,
        free_pages_after: cold.free_pages_after,
        phase: Phase::Acknowledged,
        sha256: digest(&cold.receipt_sha256),
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
            archive
                .read_collection(original.binding.clone(), reference.clone())
                .await
                .unwrap()
                .originals(),
            original.bytes.each_ref().map(Vec::as_slice)
        );
        assert!(archive
            .collection_completion(original.binding.clone(), reference.clone())
            .await
            .unwrap()
            .is_none());
        archive.close().await.unwrap();
        fixture.complete(&original, reference.clone()).await;
        let archive = fixture.archive(false).await;
        let receipt = project(
            archive
                .collection_completion(original.binding.clone(), reference.clone())
                .await
                .unwrap()
                .unwrap(),
        );
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
            let again = project(
                archive
                    .collection_completion(binding.clone(), reference.clone())
                    .await
                    .unwrap()
                    .unwrap(),
            );
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
        assert!(archive
            .read_collection(original.binding.clone(), changed.clone())
            .await
            .is_err());
        assert!(archive
            .collection_completion(original.binding.clone(), changed)
            .await
            .is_err());
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
