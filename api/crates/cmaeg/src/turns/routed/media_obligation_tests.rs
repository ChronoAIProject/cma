use super::*;
use crate::turns::routed::{
    delivery_tests::accepted,
    event_attestation,
    model::delivery_event,
    test_support::{fixture, supported, Setup},
    EventAttestationRequest,
};
use cmaeg_core::{activity::ActivityKind, seal::EventRef};
use serde_json::json;
use std::sync::atomic::Ordering;
async fn setup() -> Setup {
    let mut activity = fixture::message("Please transcribe the attachment.");
    let ActivityKind::Message { attachments, .. } = &mut activity.kind else {
        unreachable!()
    };
    attachments.push(
        serde_json::from_value(
            json!({"reference":"voice","kind":"audio","filename":"voice.wav","size_bytes":4}),
        )
        .unwrap(),
    );
    Setup::with_request(
        supported(),
        fixture::request(
            |value| {
                value["provider"]["target"] =
                    json!({"type":"existing_agent","agent_id":"agt_existing"});
            },
            activity,
        ),
    )
    .await
}
fn original() -> MediaArtifactClaim {
    MediaArtifactClaim {
        attachment_index: 0,
        kind: MediaContinuationKind::Original,
        content_sha256: sha256(b"file"),
        source_sha256: sha256(b"file"),
        name: "voice.wav".into(),
        media_type: "audio/wav".into(),
        size_bytes: 4,
        text_sha256: None,
    }
}
fn receipt() -> MediaUploadReceipt {
    MediaUploadReceipt {
        id: "att_media_fixture".into(),
        agent_id: "agt_existing".into(),
        name: "voice.wav".into(),
        path: "/home/attachments/agt_existing/att_media_fixture/voice.wav".into(),
        size_bytes: 4,
        media_type: "audio/wav".into(),
        sha256: sha256(b"file"),
    }
}
async fn initialize(setup: &Setup) -> (MediaObligationStore, MediaLease) {
    let store = MediaObligationStore(setup.journal().clone());
    store
        .initialize(
            &setup.plan,
            setup.target(),
            0,
            None,
            Some(MediaContinuationKind::Transcript),
        )
        .await
        .unwrap();
    let lease = store
        .claim(&setup.plan, setup.target(), 0, "media-worker")
        .await
        .unwrap();
    (store, lease)
}
#[tokio::test]
async fn frozen_media_bytes_and_upload_receipt_survive_restart_without_original_lease_mutations() {
    let setup = setup().await;
    let original_lease = accepted(&setup).await;
    let (store, mut lease) = initialize(&setup).await;
    store.freeze_original(&mut lease, original()).await.unwrap();
    assert!(store
        .attests(&setup.plan, setup.target(), &original())
        .await
        .unwrap());
    let mut changed = original();
    changed.source_sha256 = sha256(b"changed");
    changed.content_sha256 = changed.source_sha256.clone();
    assert_eq!(
        store.freeze_original(&mut lease, changed).await,
        Err(ExecutionError::IdempotencyConflict)
    );
    store.record_upload(&mut lease, receipt()).await.unwrap();
    store.release(&mut lease).await.unwrap();
    let restored = MediaObligationStore(setup.journal().clone());
    let lease = restored
        .claim(&setup.plan, setup.target(), 0, "replacement")
        .await
        .unwrap();
    assert!(lease.state().original.as_ref() == Some(&original()));
    assert!(lease.state().upload.as_ref() == Some(&receipt()));
    setup.journal().guard(&original_lease).await.unwrap();
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn supplementary_text_is_frozen_before_admission_and_distinct_from_original() {
    let setup = setup().await;
    let original_lease = accepted(&setup).await;
    let (store, mut lease) = initialize(&setup).await;
    store.freeze_original(&mut lease, original()).await.unwrap();
    store.record_upload(&mut lease, receipt()).await.unwrap();
    let file = store
        .freeze_supplement(&mut lease, original(), "")
        .await
        .unwrap();
    let text = "A long exact UTF-8 transcript 界\n".repeat(1700);
    assert!(text.len() <= 64 * 1024);
    let mut derived = original();
    derived.kind = MediaContinuationKind::Transcript;
    derived.content_sha256 = sha256(text.as_bytes());
    let transcript = store
        .freeze_supplement(&mut lease, derived.clone(), &text)
        .await
        .unwrap();
    assert_ne!(file.kind, transcript.kind);
    assert_eq!(lease.state().supplements.len(), 2);
    assert_eq!(store.derived_text(&lease, &transcript).await.unwrap(), text);
    assert!(store
        .attests(&setup.plan, setup.target(), &transcript)
        .await
        .unwrap());
    let mut fabricated = transcript.clone();
    fabricated.text_sha256 = Some(sha256(b"other command"));
    assert!(!store
        .attests(&setup.plan, setup.target(), &fabricated)
        .await
        .unwrap());
    store
        .record_submission(
            &mut lease,
            MediaContinuationKind::Transcript,
            MediaContinuationReceipt {
                command_id: "cmd_media".into(),
                response_id: "resp_media".into(),
                metadata: [
                    ("cma_parent_command_id", "cmd_original"),
                    (
                        "cma_parent_response_id",
                        match &original_lease.state().phase {
                            super::super::Phase::Accepted { response_id, .. } => response_id,
                            _ => unreachable!(),
                        },
                    ),
                    ("cma_parent_fingerprint", "parent_fingerprint"),
                    ("cma_artifact_id", "0:transcript"),
                    ("cma_command_fingerprint", "command_fingerprint"),
                ]
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
            },
        )
        .await
        .unwrap();
    assert!(!lease.state().supplements[1].settled);
    store.release(&mut lease).await.unwrap();
    let mut recovered = store
        .claim(&setup.plan, setup.target(), 0, "recovery")
        .await
        .unwrap();
    assert_eq!(
        store.derived_text(&recovered, &transcript).await.unwrap(),
        text
    );
    assert_eq!(
        store
            .freeze_supplement(&mut recovered, derived, &text)
            .await
            .unwrap(),
        transcript
    );
}
#[tokio::test]
async fn media_lease_fences_concurrent_and_delayed_writers() {
    let setup = setup().await;
    let (store, mut stale) = initialize(&setup).await;
    assert!(matches!(
        store.claim(&setup.plan, setup.target(), 0, "second").await,
        Err(ExecutionError::Busy)
    ));
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut current = store
        .claim(&setup.plan, setup.target(), 0, "second")
        .await
        .unwrap();
    assert_eq!(
        store.freeze_original(&mut stale, original()).await,
        Err(ExecutionError::LostLease)
    );
    store
        .freeze_original(&mut current, original())
        .await
        .unwrap();
    let mut wrong = receipt();
    wrong.path = "/home/attachments/agt_other/att_media_fixture/voice.wav".into();
    assert_eq!(
        store.record_upload(&mut current, wrong).await,
        Err(ExecutionError::Invalid)
    );
}
#[tokio::test]
async fn artifact_attestation_requires_both_current_parent_and_durable_frozen_bytes() {
    let setup = setup().await;
    let _original_lease = accepted(&setup).await;
    let (store, mut lease) = initialize(&setup).await;
    let event = delivery_event(&setup.plan, setup.target());
    let mut request = EventAttestationRequest {
        event_ref: EventRef::seal(&event, &fixture::keys()).unwrap(),
        provider_binding_id: setup.target().launch.provider.binding_id.clone(),
        channel_id: event.channel_id,
        conversation_id: event.conversation_id,
        event_id: event.event_id,
        activity_sha256: event.activity_sha256.unwrap(),
        event_expires_at: event.expires_at,
        binding_generation: None,
        artifact: Some(original()),
    };
    let verify = |request: EventAttestationRequest| {
        let setup = &setup;
        async move {
            event_attestation::attest(
                &setup.server.store(),
                setup.journal(),
                setup.current.as_ref(),
                &request,
            )
            .await
        }
    };
    assert!(!verify(request.clone()).await.unwrap());
    store.freeze_original(&mut lease, original()).await.unwrap();
    assert!(verify(request.clone()).await.unwrap());
    request.artifact.as_mut().unwrap().size_bytes += 1;
    assert!(!verify(request.clone()).await.unwrap());
    request.artifact = Some(original());
    store.refuse(&mut lease, "source_changed").await.unwrap();
    assert!(!verify(request).await.unwrap());
}

#[tokio::test]
async fn expired_capability_cannot_have_effects_but_keeps_durable_refusal_evidence() {
    let setup = setup().await;
    let (store, mut lease) = initialize(&setup).await;
    store.freeze_original(&mut lease, original()).await.unwrap();
    store.release(&mut lease).await.unwrap();
    setup.server.memory.clock.fetch_add(901, Ordering::SeqCst);
    let mut expired = store
        .claim(&setup.plan, setup.target(), 0, "expiry-worker")
        .await
        .unwrap();
    assert_eq!(
        store.guard_effect(&expired).await,
        Err(ExecutionError::Expired)
    );
    store.refuse(&mut expired, "event_expired").await.unwrap();
    assert_eq!(
        store
            .load(&setup.plan, setup.target(), 0)
            .await
            .unwrap()
            .unwrap()
            .refusal
            .as_deref(),
        Some("event_expired")
    );
    assert!(!store
        .attests(&setup.plan, setup.target(), &original())
        .await
        .unwrap());
}
