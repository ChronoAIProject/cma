//! Report mode: every definition an older image cannot read is listed, and
//! nothing is written.
use super::{fake_cma::FakeCma, support::*};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use cmaeg::channels::audit_routing::{
    audit_routing, RoutingAuditAction, RoutingAuditMode, RoutingFinding,
};
use cmaeg_core::seal::MasterKeyRing;
use std::collections::BTreeSet;

async fn report(
    cma: &FakeCma,
    non_human: &BTreeSet<String>,
) -> cmaeg::channels::audit::AuditReport<RoutingFinding> {
    audit_routing(
        &records(cma),
        &key_ring(),
        non_human,
        RoutingAuditMode::Report,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn report_lists_every_extended_shape_and_skips_legacy_channels() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring();
    let (routed, channel) = seed(&cma, &keys, routed(), true);
    let (relays, _) = seed(&cma, &keys, multi_relay(), false);
    let (custom_key, _) = seed(&cma, &keys, inert_custom_key(), false);
    let (detached, _) = seed(&cma, &keys, zero_sources(), false);
    seed(&cma, &keys, legacy(), true);
    let report = report(&cma, &non_human()).await;
    assert_eq!(
        (report.scanned, report.channels, report.unreadable),
        (5, 5, 0)
    );
    assert_eq!(
        (report.cleared, report.blocked(), report.remaining),
        (0, 0, 4)
    );
    assert!(!report.complete());
    let lines = report.lines();
    assert_eq!(lines.len(), 5);
    assert!(lines.contains(&format!(
        r#"{{"record_id":"{}","channel_hash":"{}","extensions":["routing"],"default_receives_all":false,"action":"found","reason":null}}"#,
        routed.record_id,
        hash(&channel)
    )));
    for sources_only in [&relays, &custom_key, &detached] {
        assert!(lines.contains(&format!(
            r#"{{"record_id":"{}","channel_hash":null,"extensions":["sources"],"default_receives_all":true,"action":"found","reason":null}}"#,
            sources_only.record_id
        )));
    }
    assert_eq!(
        lines[4],
        r#"{"scanned":5,"channels":5,"unreadable":0,"found":4,"cleared":0,"blocked":0,"remaining":4}"#
    );
    assert!(report
        .findings
        .iter()
        .all(|finding| finding.action == RoutingAuditAction::Found && finding.reason.is_none()));
    // Reporting alone never writes.
    assert!(cma.records().values().all(|record| record.version == 1));
    cma.close().await;
}

#[tokio::test]
async fn a_routed_channel_with_both_extensions_names_both() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring();
    let mut value = routed_to_default();
    value["sources"] = serde_json::json!([]);
    value["reply"]["sinks"] = serde_json::json!([]);
    value["routing"]
        .as_object_mut()
        .unwrap()
        .remove("addressing");
    seed(&cma, &keys, value, false);
    let report = report(&cma, &non_human()).await;
    assert_eq!(report.findings[0].extensions, ["routing", "sources"]);
    // No source is attached, so nothing can address another consumer.
    assert!(report.findings[0].default_receives_all);
    cma.close().await;
}

#[tokio::test]
async fn whether_default_answers_custom_events_follows_the_non_human_set() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring();
    seed(&cma, &keys, routed_to_default(), false);
    assert!(report(&cma, &non_human()).await.findings[0].default_receives_all);
    // With a custom source attached, `default` must answer non-human actors.
    assert!(!report(&cma, &BTreeSet::new()).await.findings[0].default_receives_all);
    cma.close().await;
}

#[tokio::test]
async fn an_empty_store_is_complete() {
    let cma = FakeCma::start(false).await;
    let report = report(&cma, &non_human()).await;
    assert!(report.complete());
    assert_eq!(
        report.lines(),
        [
            r#"{"scanned":0,"channels":0,"unreadable":0,"found":0,"cleared":0,"blocked":0,"remaining":0}"#
        ]
    );
    cma.close().await;
}

#[tokio::test]
async fn unreadable_and_invalid_records_fail_closed() {
    let cma = FakeCma::start(false).await;
    let keys = key_ring();
    let other = MasterKeyRing::parse(&format!("9:{}", STANDARD_NO_PAD.encode([9_u8; 32]))).unwrap();
    cma.seed(
        &uuid::Uuid::new_v4().to_string(),
        OWNER,
        b"not a sealed blob".to_vec(),
    );
    let (foreign, _) = seed(&cma, &other, routed(), false);
    // Parses, but the reader refuses it: its profile digest does not match.
    let (mut invalid, _) = seed(&cma, &keys, routed(), false);
    invalid.profile_digest = format!("sha256:{}", "0".repeat(64));
    cma.seed(invalid.record_id.as_str(), OWNER, seal(&keys, &invalid));
    seed(&cma, &keys, legacy(), false);
    let report = report(&cma, &non_human()).await;
    assert_eq!((report.scanned, report.channels), (4, 2));
    assert_eq!(report.unreadable, 3);
    assert!(report.findings.is_empty());
    assert_eq!(report.remaining, 0);
    assert!(!report.complete());
    assert!(!report.lines().concat().contains(foreign.record_id.as_str()));
    assert!(cma.records().values().all(|record| record.version == 1));
    cma.close().await;
}
