use super::*;

fn hint() -> Document {
    mongodb::bson::serialize_to_document(&Hint {
        version: 1,
        observation: uuid::Uuid::new_v4().to_string(),
        reason: Reason::CapabilityMissing,
        key: Key {
            owner: "owner".into(),
            sandbox: "sandbox".into(),
            workspace: "workspace".into(),
            worktree: "worktree".into(),
            conversation: "conversation".into(),
            generation: 1,
            ready_at: "2026-01-01T00:00:00Z".into(),
            attempts: 0,
            command_generation: 0,
            runtime_id: "runtime".into(),
            boot_id: None,
            runtime_version: "version".into(),
            provision_generation: None,
            bound_at: "2026-01-01T00:00:00Z".into(),
            high_water: 1,
        },
        owner_protocol: 1,
        receiver_lease_ms: 10000,
        retry_classifier_version: 1,
        observed_at: DateTime::from_millis(1000),
        until: DateTime::from_millis(61000),
        owner_epoch: 1,
        claimed_revision: 2,
    })
    .unwrap()
}

#[test]
fn exact_fixed_interval_excludes_future_and_expired_observations() {
    let hint = Hint::read(&doc! {FIELD: hint()}).unwrap();
    assert!(!hint.active(DateTime::from_millis(999)));
    assert!(hint.active(DateTime::from_millis(1000)));
    assert!(hint.active(DateTime::from_millis(60999)));
    assert!(!hint.active(DateTime::from_millis(61000)));
}

#[test]
fn malformed_unknown_oversized_and_overflow_hints_are_inert() {
    assert!(Hint::read(&Document::new()).is_none());
    assert!(Hint::read(&doc! {FIELD: "unexpected"}).is_none());
    for (field, value) in [
        ("version", Bson::Int64(2)),
        ("reason", Bson::String("future".into())),
        ("retry_classifier_version", Bson::Int64(2)),
        ("owner_epoch", Bson::Int64(-1)),
        ("observed_at", Bson::String("not a date".into())),
        ("until", Bson::DateTime(DateTime::from_millis(61001))),
        ("until", Bson::DateTime(DateTime::from_millis(999))),
        (
            "observed_at",
            Bson::DateTime(DateTime::from_millis(i64::MIN)),
        ),
        ("observation", Bson::String("not a UUID".into())),
    ] {
        let mut hint = hint();
        hint.insert(field, value);
        assert!(Hint::read(&doc! {FIELD: hint}).is_none(), "{field}");
    }
    for (field, value) in [
        ("runtime_id", Bson::String("x".repeat(4097))),
        ("owner", Bson::String(String::new())),
        ("generation", Bson::Int64(-1)),
        ("attempts", Bson::Int64(i64::MAX)),
        ("command_generation", Bson::String("overflow".into())),
    ] {
        let mut hint = hint();
        hint.get_document_mut("key").unwrap().insert(field, value);
        assert!(Hint::read(&doc! {FIELD: hint}).is_none(), "{field}");
    }
}

#[test]
fn ownership_retry_hint_diagnostics_do_not_inherit_private_request_fields() {
    use std::io::Write;
    use std::sync::Mutex;
    #[derive(Clone)]
    struct Output(Arc<Mutex<Vec<u8>>>);
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let output = Output(Arc::default());
    let sink = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || sink.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let parent = tracing::info_span!("request", payload = "private-retry-ancestor-marker");
    let _entered = parent.enter();
    for event in [
        "recorded",
        "held",
        "expired",
        "mismatched",
        "stale_write_rejected",
        "malformed",
        "unavailable",
    ] {
        emit(event, Some(Reason::CapabilityMissing));
    }
    let output = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(!output.contains("private-retry-ancestor-marker"));
    let events: Vec<serde_json::Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 7);
    for event in events {
        assert_eq!(event["fields"]["reason"], "capability_missing");
        assert_eq!(event["fields"]["phase"], "challenge");
        assert_eq!(event["fields"]["duration_ms"], HOLD_MS);
    }
}
