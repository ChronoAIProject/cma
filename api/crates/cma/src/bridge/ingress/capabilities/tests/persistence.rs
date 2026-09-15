use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

#[test]
fn signed_body_mismatch_cannot_reach_capability_extraction() {
    struct Probe(AtomicUsize);
    impl CallbackCapabilityExtractor for Probe {
        fn contract(&self) -> CapabilityContract {
            CapabilityContract::NyxidPlatformV1
        }
        fn observe(
            &self,
            payload: &AuthenticatedCallbackPayload<'_>,
        ) -> Result<PlatformCapabilityObservation, CallbackError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            NyxidCapabilities.observe(payload)
        }
    }
    let fixture = Fixture::new(BridgePlatform::Telegram);
    let probe = Probe(AtomicUsize::new(0));
    let signed = fixture.signed().unwrap();
    let mut changed = fixture.clone();
    changed.body["content"]["text"] = json!("unsigned replacement");
    assert!(matches!(
        signed.classify_capabilities(&changed.bytes(), &probe),
        Err(CallbackError::Authentication)
    ));
    assert_eq!(probe.0.load(Ordering::SeqCst), 0);
}

#[test]
fn persisted_verified_observation_round_trips_without_raw_secrets_and_checks_exact_source() {
    for platform in platforms() {
        let mut fixture = Fixture::new(platform);
        fixture.body["thread_id"] = json!("interaction:application:fixture-secret");
        let write = fixture.write().unwrap();
        assert_eq!(
            write.thread_context(),
            Some("interaction:application:fixture-secret")
        );
        let value = serde_json::to_value(write.record()).unwrap();
        let encoded = value.to_string();
        for secret in [
            "fixture-secret",
            "fixture-header-secret",
            "raw_platform_data",
            "interaction:",
        ] {
            assert!(!encoded.contains(secret));
        }
        assert_eq!(
            write.record().capabilities.as_ref().unwrap().contract(),
            CapabilityContract::NyxidPlatformV1
        );
        let record: StoredInbox = serde_json::from_value(value.clone()).unwrap();
        assert!(NormalizedPartitionSource::from_stored(&record, &fixture.bridge).is_ok());
        for field in [
            "id",
            "message_id",
            "body",
            "config",
            "chat",
            "sender",
            "version",
        ] {
            let mut changed = value.clone();
            match field {
                "id" => changed["id"] = json!("a".repeat(64)),
                "message_id" => changed["message_id"] = json!("another-message"),
                "body" => changed["attestation"]["body_sha256"] = json!("b".repeat(64)),
                "config" => changed["pin"]["config_revision"] = json!(2),
                "chat" => changed["envelope"]["chat_id"] = json!("another-chat"),
                "sender" => changed["envelope"]["sender_id"] = json!("another-sender"),
                _ => changed["capabilities"]["version"] = json!(2),
            }
            let changed: StoredInbox = serde_json::from_value(changed).unwrap();
            assert!(
                NormalizedPartitionSource::from_stored(&changed, &fixture.bridge).is_err(),
                "{field}"
            );
        }
        let mut retained = value;
        retained.as_object_mut().unwrap().remove("capabilities");
        let retained: StoredInbox = serde_json::from_value(retained).unwrap();
        assert!(retained.capabilities.is_none());
        assert!(matches!(
            NormalizedPartitionSource::from_stored(&retained, &fixture.bridge),
            Err(PartitionError::InvalidSource(
                "capability_evidence_unavailable"
            ))
        ));
    }
}
