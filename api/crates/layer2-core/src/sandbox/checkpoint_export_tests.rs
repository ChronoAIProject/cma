use layer3_adapter::{
    CheckpointReceipt, ConversationSnapshot, HomeSnapshotConversationIdentity,
    HomeSnapshotExportConversation, HomeSnapshotObservation, HomeSnapshotReceipt,
    HomeSnapshotRequest, HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt,
    HomeVolumeRequest, WriterState,
};
use sha2::{Digest, Sha256};

use super::*;
use crate::sandbox::SandboxCheckpointConversation;

fn evidence(stamp: &str) -> (HomeSnapshotExportRequest, HomeSnapshotExportReceipt) {
    let home = HomeVolumeRequest {
        scope: None,
        volume_id: "home-one".into(),
        sandbox_id: "sbx_one".into(),
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "cma-home-v1".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    };
    let source = HomeVolumeAttachment {
        receipt: HomeVolumeReceipt {
            native_authority: None,
            volume_id: home.volume_id.clone(),
            request_sha256: home.computed_sha256(),
            storage_reference: "home-pvc".into(),
            storage_generation: "home-pvc-uid".into(),
        },
        request: home,
    };
    let snapshot = HomeSnapshotRequest {
        capture_id: "capture-one".into(),
        epoch: 2,
        home: source,
        snapshot_class: "cma-home-v1".into(),
    };
    let observation = HomeSnapshotObservation {
        receipt: HomeSnapshotReceipt {
            capture_id: snapshot.capture_id.clone(),
            request_sha256: snapshot.computed_sha256(),
            snapshot_reference: "snapshot".into(),
            snapshot_generation: "snapshot-uid".into(),
        },
        captured_at: Some(stamp.into()),
        ready: true,
    };
    let identity = HomeSnapshotConversationIdentity {
        agent_id: "agt_one".into(),
        conversation_id: "conversation-one".into(),
    };
    let request = HomeSnapshotExportRequest {
        snapshot,
        observation,
        runtime_generation: 1,
        conversations: vec![identity.clone()],
    };
    let receipt = HomeSnapshotExportReceipt {
        request_sha256: request.computed_sha256().unwrap(),
        checkpoint: CheckpointReceipt {
            epoch: 2,
            object_reference: format!("home-{}.cma", "a".repeat(64)),
            sha256: "a".repeat(64),
            writers: WriterState::Resumed,
        },
        archive_bytes: 100,
        conversations: vec![HomeSnapshotExportConversation {
            identity,
            archive_sha256: format!("{:x}", Sha256::digest(b"captured conversation")),
        }],
    };
    (request, receipt)
}

#[test]
fn exported_metadata_matches_content_based_sealing_at_the_capture_time() {
    let (request, receipt) = evidence("2026-09-10T00:00:01.987654321Z");
    let exported = SandboxCheckpointRef::seal_export(&request, &receipt).unwrap();
    let snapshot = SandboxCheckpointConversation::new(
        AgentId("agt_one".into()),
        "conversation-one",
        ConversationSnapshot {
            conversation_id: "conversation-one".into(),
            archive_sha256: receipt.conversations[0].archive_sha256.clone(),
            archive: b"captured conversation".to_vec(),
        },
    )
    .unwrap();
    let from_bytes =
        SandboxCheckpointRef::seal(&receipt.checkpoint, &[snapshot], "2026-09-10T00:00:01Z")
            .unwrap();
    assert_eq!(exported, from_bytes);
    exported.validate_export(&request, &receipt).unwrap();
    assert_eq!(
        exported.restore_request().unwrap().sha256,
        receipt.checkpoint.sha256
    );
    assert!(!serde_json::to_string(&exported)
        .unwrap()
        .contains("captured conversation"));
}

#[test]
fn provider_offset_is_normalized_without_changing_the_exact_export_authority() {
    let (request, receipt) = evidence("2026-09-10T08:00:01.5+08:00");
    let checkpoint = SandboxCheckpointRef::seal_export(&request, &receipt).unwrap();
    assert_eq!(checkpoint.sealed_at, "2026-09-10T00:00:01Z");
    let mut changed = request.clone();
    changed.observation.captured_at = Some("2026-09-10T00:00:01.5Z".into());
    assert!(checkpoint.validate_export(&changed, &receipt).is_err());
    let (old, old_receipt) = evidence("1969-12-31T23:59:59Z");
    assert!(SandboxCheckpointRef::seal_export(&old, &old_receipt).is_err());
}

#[test]
fn different_capture_or_conversation_evidence_cannot_seal_or_validate() {
    let (request, receipt) = evidence("2026-09-10T00:00:01Z");
    let checkpoint = SandboxCheckpointRef::seal_export(&request, &receipt).unwrap();
    for index in 0..5 {
        let mut changed = request.clone();
        match index {
            0 => changed.snapshot.epoch += 1,
            1 => changed.runtime_generation += 1,
            2 => changed.observation.ready = false,
            3 => changed.snapshot.home.receipt.storage_generation = "another-disk".into(),
            _ => changed.conversations[0].conversation_id = "newer-conversation".into(),
        }
        assert!(SandboxCheckpointRef::seal_export(&changed, &receipt).is_err());
    }
    let mut changed = receipt.clone();
    changed.conversations[0].archive_sha256 = "b".repeat(64);
    assert!(checkpoint.validate_export(&request, &changed).is_err());
    changed = receipt;
    changed.checkpoint.writers = WriterState::Held;
    assert!(SandboxCheckpointRef::seal_export(&request, &changed).is_err());
}
