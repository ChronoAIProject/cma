//! The rollback audit of stored custom attachment settings, against the fake CMA.
use super::*;
use cmaeg::channels::{
    audit::{audit_custom_attachments, AuditAction, AuditReport},
    definition::channel_pointer,
};
use cmaeg_core::{
    channel::{ChannelDefinition, Secret, Source},
    ids::ChannelId,
    profile::canonical_json,
    seal::{open_blob, seal_blob, MasterKeyRing},
};
use serde_json::Value;
use std::sync::{atomic::Ordering, Arc, Mutex};

const OWNER: &str = "audit-owner-subject-fixture";
const CREDENTIAL: &str = "nyx_audit_credential_fixture";

fn record_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A stored custom channel, optionally carrying the setting and its creation.
fn definition(
    record_id: &str,
    attachments: bool,
    channel: Option<&ChannelId>,
) -> ChannelDefinition {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    value["record_id"] = record_id.into();
    value["owner"]["subject"] = OWNER.into();
    value["profile"]["metadata"]["owner"]["subject"] = OWNER.into();
    if attachments {
        value["sources"][0]["attachments"] =
            json!({"pointer":"/attachments","origins":["https://files.example"]});
    }
    if let Some(channel) = channel {
        value["creation"] =
            json!({"channel_id":channel,"fingerprint":"sha256:fixture","ready":true});
    }
    let mut definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    definition.profile_digest = definition.profile.digest();
    definition.version = 0;
    definition.secrets.provider_credential = Some(Secret::new(CREDENTIAL));
    definition
}

fn seal(keys: &MasterKeyRing, definition: &ChannelDefinition) -> Vec<u8> {
    seal_blob(keys, canonical_json(definition.to_blob_json()).as_bytes()).unwrap()
}

/// Seed a definition and return it with its blob.
fn seed(
    cma: &FakeCma,
    keys: &MasterKeyRing,
    attachments: bool,
    created: bool,
) -> (ChannelDefinition, Option<ChannelId>) {
    let id = record_id();
    let channel = created.then(|| {
        channel_pointer(
            &cma.base_url,
            &cmaeg_core::ids::RecordId::new(id.clone()).unwrap(),
        )
        .unwrap()
        .seal_id(keys)
        .unwrap()
    });
    let definition = definition(&id, attachments, channel.as_ref());
    cma.seed(&id, OWNER, seal(keys, &definition));
    (definition, channel)
}

fn stored(cma: &FakeCma, keys: &MasterKeyRing, record_id: &str) -> (ChannelDefinition, u64) {
    let record = cma.records().remove(record_id).unwrap();
    let opened = open_blob(keys, &record.blob).unwrap();
    (
        serde_json::from_slice(&opened.payload).unwrap(),
        record.version,
    )
}

async fn audit(cma: &FakeCma, clear: bool) -> AuditReport {
    let records = cmaeg::records::RecordClient::new(&cma.base_url, "cma-service").unwrap();
    audit_custom_attachments(&records, &key_ring(1), clear)
        .await
        .unwrap()
}

fn carries(definition: &ChannelDefinition) -> bool {
    matches!(
        &definition.sources[0],
        Source::Custom {
            attachments: Some(_),
            ..
        }
    )
}

#[tokio::test]
async fn listing_pages_through_every_record() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring(1);
    for _ in 0..230 {
        seed(&cma, &keys, false, false);
    }
    let report = audit(&cma, false).await;
    assert_eq!(cma.lists.load(Ordering::SeqCst), 3);
    assert_eq!((report.scanned, report.channels), (230, 230));
    assert!(report.findings.is_empty());
    assert!(report.complete());
    assert_eq!(
        report.lines(),
        vec![
            r#"{"scanned":230,"channels":230,"unreadable":0,"found":0,"cleared":0,"remaining":0}"#
        ]
    );
    cma.close().await;
}

#[tokio::test]
async fn settings_are_reported_with_a_channel_hash_only_when_creation_exists() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring(1);
    let (created, channel) = seed(&cma, &keys, true, true);
    let (direct, _) = seed(&cma, &keys, true, false);
    seed(&cma, &keys, false, true);
    let report = audit(&cma, false).await;
    assert_eq!((report.scanned, report.channels), (3, 3));
    assert_eq!((report.cleared, report.remaining), (0, 2));
    assert!(!report.complete());
    let hash = cmaeg::telemetry::channel_hash(channel.unwrap().as_str());
    for finding in &report.findings {
        assert_eq!(finding.action, AuditAction::Found);
        if finding.record_id == created.record_id {
            assert_eq!(finding.channel_hash.as_deref(), Some(hash.as_str()));
        } else {
            assert_eq!(finding.record_id, direct.record_id);
            assert_eq!(finding.channel_hash, None);
        }
    }
    let lines = report.lines();
    assert_eq!(lines.len(), 3);
    assert!(lines.contains(&format!(
        r#"{{"record_id":"{}","channel_hash":"{hash}","action":"found"}}"#,
        created.record_id
    )));
    assert!(lines.contains(&format!(
        r#"{{"record_id":"{}","channel_hash":null,"action":"found"}}"#,
        direct.record_id
    )));
    assert_eq!(
        lines[2],
        r#"{"scanned":3,"channels":3,"unreadable":0,"found":2,"cleared":0,"remaining":2}"#
    );
    // Reporting alone never writes.
    assert!(cma.records().values().all(|record| record.version == 1));
    cma.close().await;
}

#[tokio::test]
async fn clearing_removes_only_the_setting() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring(1);
    let (original, _) = seed(&cma, &keys, true, true);
    let report = audit(&cma, true).await;
    assert_eq!(report.findings[0].action, AuditAction::Cleared);
    assert_eq!((report.cleared, report.remaining), (1, 0));
    assert!(report.complete());
    let (cleared, version) = stored(&cma, &keys, original.record_id.as_str());
    assert_eq!(version, 2);
    let mut expected = original;
    let Source::Custom { attachments, .. } = &mut expected.sources[0] else {
        panic!("custom source expected");
    };
    *attachments = None;
    // The definition records the version it was written over, as updates do.
    expected.version = 1;
    assert_eq!(cleared, expected);
    let again = audit(&cma, true).await;
    assert!(again.findings.is_empty());
    assert!(again.complete());
    cma.close().await;
}

#[tokio::test]
async fn one_version_conflict_is_reread_and_cleared() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring(1);
    let (original, _) = seed(&cma, &keys, true, false);
    cma.put_conflicts.store(1, Ordering::SeqCst);
    let report = audit(&cma, true).await;
    assert_eq!(report.findings[0].action, AuditAction::Cleared);
    assert_eq!((report.cleared, report.remaining), (1, 0));
    let (cleared, version) = stored(&cma, &keys, original.record_id.as_str());
    assert!(!carries(&cleared));
    assert_eq!((cleared.version, version), (2, 3));
    cma.close().await;
}

#[tokio::test]
async fn a_second_conflict_is_reported_and_remains() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring(1);
    let (original, _) = seed(&cma, &keys, true, false);
    cma.put_conflicts.store(2, Ordering::SeqCst);
    let report = audit(&cma, true).await;
    assert_eq!(report.findings[0].action, AuditAction::Conflict);
    assert_eq!((report.cleared, report.remaining), (0, 1));
    assert!(!report.complete());
    assert!(report.lines()[0].ends_with(r#""action":"conflict"}"#));
    assert!(carries(&stored(&cma, &keys, original.record_id.as_str()).0));
    cma.close().await;
}

#[tokio::test]
async fn unreadable_records_fail_closed() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring(1);
    let other = MasterKeyRing::parse(&format!(
        "9:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode([9_u8; 32])
    ))
    .unwrap();
    cma.seed(&record_id(), OWNER, b"not a sealed blob".to_vec());
    cma.seed(
        &record_id(),
        OWNER,
        seal(&other, &definition(&record_id(), true, None)),
    );
    cma.seed(
        &record_id(),
        OWNER,
        seal_blob(&keys, b"{\"not\":\"a channel\"}").unwrap(),
    );
    seed(&cma, &keys, false, false);
    for clear in [false, true] {
        let report = audit(&cma, clear).await;
        assert_eq!((report.scanned, report.channels), (4, 1));
        assert_eq!(report.unreadable, 3);
        assert_eq!(report.remaining, 0);
        assert!(!report.complete());
        assert_eq!(
            report.lines(),
            vec![
                r#"{"scanned":4,"channels":1,"unreadable":3,"found":0,"cleared":0,"remaining":0}"#
            ]
        );
    }
    cma.close().await;
}

#[tokio::test]
async fn a_record_listed_under_another_owner_is_not_rewritten() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring(1);
    let id = record_id();
    let original = definition(&id, true, None);
    cma.seed(&id, "another-owner", seal(&keys, &original));
    let report = audit(&cma, true).await;
    assert_eq!(report.findings[0].action, AuditAction::Found);
    assert_eq!((report.cleared, report.remaining), (0, 1));
    let (kept, version) = stored(&cma, &keys, &id);
    assert_eq!((kept, version), (original, 1));
    cma.close().await;
}

#[tokio::test]
async fn a_record_store_failure_is_a_classified_error() {
    let cma = FakeCma::start(true).await;
    let keys = key_ring(1);
    seed(&cma, &keys, true, false);
    let records = cmaeg::records::RecordClient::new(&cma.base_url, "cma-service").unwrap();
    let error = audit_custom_attachments(&records, &keys, true)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "record store unavailable");
    let refused = cmaeg::records::RecordClient::new(&cma.base_url, "wrong-token").unwrap();
    let error = audit_custom_attachments(&refused, &keys, false)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "record store rejected a request");
    cma.close().await;
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The fake CMA runs on this current-thread runtime, so a thread-scoped
/// subscriber captures every log line without touching global state.
#[tokio::test(flavor = "current_thread")]
async fn no_secret_owner_subject_or_blob_reaches_the_output_or_logs() {
    let capture = Capture::default();
    let writer = capture.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let cma = FakeCma::start(false).await;
    let keys = key_ring(1);
    let (original, channel) = seed(&cma, &keys, true, true);
    let blob = STANDARD_NO_PAD.encode(&cma.records()[original.record_id.as_str()].blob);
    cma.put_conflicts.store(1, Ordering::SeqCst);
    let mut output = audit(&cma, false).await.lines().join("\n");
    output.push_str(&audit(&cma, true).await.lines().join("\n"));
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let hash = cmaeg::telemetry::channel_hash(channel.unwrap().as_str());
    assert!(logs.contains("custom attachments cleared"), "{logs}");
    assert!(logs.contains(&hash), "{logs}");
    for text in [&output, &logs] {
        for leaked in [
            OWNER,
            CREDENTIAL,
            "cma-service",
            "files.example",
            "sha256:fixture",
        ] {
            assert!(!text.contains(leaked), "{leaked} leaked");
        }
        assert!(!text.contains(&blob[..32]), "blob leaked");
    }
    cma.close().await;
}
