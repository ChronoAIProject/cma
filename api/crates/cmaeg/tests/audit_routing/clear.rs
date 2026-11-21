//! Clear mode: what can be cleared safely returns to the legacy byte shape,
//! and what needs an owner decision is blocked and never written.
use super::{
    fake_cma::FakeCma,
    fake_provider::{FakeProvider, FakeProviderConfig},
    previous_schema,
    routing_fixture::{agent_key, NYX, REVIEWER},
    support::*,
};
use cmaeg::channels::{
    audit::AuditReport,
    audit_routing::{audit_routing, RoutingAuditAction, RoutingAuditMode, RoutingFinding},
};
use cmaeg_core::{channel::ChannelDefinition, ids::RecordId};
use serde_json::json;
use std::{collections::BTreeMap, sync::atomic::Ordering};

async fn clear(
    cma: &FakeCma,
    provider: &FakeProvider,
    allow_default_widening: bool,
) -> AuditReport<RoutingFinding> {
    audit_routing(
        &records(cma),
        &key_ring(),
        Some(&providers(provider)),
        &non_human(),
        RoutingAuditMode::Clear {
            allow_default_widening,
        },
    )
    .await
    .unwrap()
}

/// A fake provider whose binding deletions answer `statuses` by binding ID.
async fn provider(statuses: &[(&str, u16)]) -> FakeProvider {
    FakeProvider::start(FakeProviderConfig {
        unbind_status: statuses
            .iter()
            .map(|(binding, status)| ((*binding).to_owned(), *status))
            .collect::<BTreeMap<_, _>>(),
        ..FakeProviderConfig::default()
    })
    .await
}

fn both_unbound() -> Vec<(String, String)> {
    [("binding-reviewer", REVIEWER), ("binding-nyx", NYX)]
        .into_iter()
        .map(|(binding, consumer)| {
            (
                format!("/api/v1/cmaeg/bindings/{binding}"),
                format!("Bearer {}", agent_key(consumer)),
            )
        })
        .collect()
}

/// The stored definition, after proving the previous image can read it.
fn readable_by_the_previous_image(cma: &FakeCma, record_id: &RecordId) -> (ChannelDefinition, u64) {
    let (payload, version) = stored(cma, &key_ring(), record_id);
    serde_json::from_slice::<previous_schema::ChannelDefinition>(&payload)
        .expect("the previous image reads the cleared definition");
    (serde_json::from_slice(&payload).unwrap(), version)
}

fn unchanged(cma: &FakeCma, record_id: &RecordId) {
    assert_eq!(cma.records()[record_id.as_str()].version, 1);
}

#[tokio::test]
async fn a_channel_whose_default_receives_all_is_unbound_and_stripped() {
    let cma = FakeCma::start(false).await;
    // One binding is already gone; a 404 counts as removed.
    let provider = provider(&[("binding-nyx", 404)]).await;
    let (original, channel) = seed(&cma, &key_ring(), routed_to_default(), true);
    let report = clear(&cma, &provider, false).await;
    assert_eq!(
        report.lines(),
        [
            format!(
                r#"{{"record_id":"{}","channel_hash":"{}","extensions":["routing"],"default_receives_all":true,"action":"cleared","reason":null}}"#,
                original.record_id,
                hash(&channel)
            ),
            r#"{"scanned":1,"channels":1,"unreadable":0,"found":1,"cleared":1,"blocked":0,"remaining":0}"#.to_owned()
        ]
    );
    assert!(report.complete());
    assert_eq!(unbinds(&provider), both_unbound());
    let (cleared, version) = readable_by_the_previous_image(&cma, &original.record_id);
    assert_eq!(version, 2);
    let mut expected = original;
    expected.routing = None;
    expected.secrets.consumer_keys.clear();
    // The definition records the version it was written over, as updates do.
    expected.version = 1;
    assert_eq!(cleared, expected);
    let again = clear(&cma, &provider, false).await;
    assert!(again.findings.is_empty());
    assert!(again.complete());
    assert_eq!(provider.requests().len(), 2);
    provider.close().await;
    cma.close().await;
}

#[tokio::test]
async fn a_mentionable_reviewer_blocks_clearing_unless_widening_is_allowed() {
    let cma = FakeCma::start(false).await;
    let provider = provider(&[]).await;
    let mut value = routed();
    // Every rule includes `default`, but `reviewer` can still be mentioned.
    value["routing"]["rules"] = json!([{"id": "all", "targets": [REVIEWER, "default"]}]);
    let (original, _) = seed(&cma, &key_ring(), value, false);
    let blocked = clear(&cma, &provider, false).await;
    let finding = &blocked.findings[0];
    assert_eq!(finding.action, RoutingAuditAction::Blocked);
    assert_eq!(finding.reason, Some("default_would_widen"));
    assert!(!finding.default_receives_all);
    assert_eq!(
        (blocked.cleared, blocked.blocked(), blocked.remaining),
        (0, 1, 1)
    );
    assert!(!blocked.complete());
    assert!(blocked.lines()[0].ends_with(
        r#""default_receives_all":false,"action":"blocked","reason":"default_would_widen"}"#
    ));
    assert!(blocked.lines()[1].contains(r#""blocked":1,"remaining":1}"#));
    assert!(provider.log().is_empty());
    unchanged(&cma, &original.record_id);

    let widened = clear(&cma, &provider, true).await;
    assert_eq!(widened.findings[0].action, RoutingAuditAction::Cleared);
    assert!(widened.complete());
    assert_eq!(unbinds(&provider), both_unbound());
    let (cleared, _) = readable_by_the_previous_image(&cma, &original.record_id);
    assert!(cleared.routing.is_none());
    provider.close().await;
    cma.close().await;
}

#[tokio::test]
async fn source_shapes_only_an_owner_can_reduce_are_blocked() {
    let cma = FakeCma::start(false).await;
    let provider = provider(&[]).await;
    let keys = key_ring();
    let (relays, _) = seed(&cma, &keys, multi_relay(), false);
    let (detached, _) = seed(&cma, &keys, zero_sources(), false);
    // Routing on top of several relays: the sources block it first, and no
    // consumer is unbound even with widening allowed.
    let mut routed_relays = multi_relay();
    routed_relays["routing"] = routed()["routing"].clone();
    routed_relays["secrets"] = routed()["secrets"].clone();
    let (routed_relays, _) = seed(&cma, &keys, routed_relays, false);
    let report = clear(&cma, &provider, true).await;
    assert_eq!(report.findings.len(), 3);
    for finding in &report.findings {
        assert_eq!(finding.action, RoutingAuditAction::Blocked);
        assert_eq!(finding.reason, Some("sources_need_owner"));
    }
    assert_eq!((report.blocked(), report.remaining), (3, 3));
    assert!(provider.log().is_empty());
    for record in [&relays, &detached, &routed_relays] {
        unchanged(&cma, &record.record_id);
    }
    provider.close().await;
    cma.close().await;
}

#[tokio::test]
async fn an_inert_custom_key_is_stripped_without_a_provider() {
    let cma = FakeCma::start(false).await;
    let (original, _) = seed(&cma, &key_ring(), inert_custom_key(), false);
    // Nothing needs unbinding, so no provider client is needed either.
    let report = audit_routing(
        &records(&cma),
        &key_ring(),
        None,
        &non_human(),
        RoutingAuditMode::Clear {
            allow_default_widening: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(report.findings[0].action, RoutingAuditAction::Cleared);
    assert_eq!(report.findings[0].extensions, ["sources"]);
    assert!(report.complete());
    let (cleared, _) = readable_by_the_previous_image(&cma, &original.record_id);
    assert_eq!(cleared.partition.custom_key, None);
    assert_eq!(cleared.partition.key, original.partition.key);
    assert_eq!(cleared.sources, original.sources);
    cma.close().await;
}

#[tokio::test]
async fn a_failed_unbind_leaves_the_record_unchanged() {
    let cma = FakeCma::start(false).await;
    let provider = provider(&[("binding-nyx", 500)]).await;
    let (original, _) = seed(&cma, &key_ring(), routed_to_default(), false);
    let report = clear(&cma, &provider, false).await;
    assert_eq!(report.findings[0].action, RoutingAuditAction::Blocked);
    assert_eq!(report.findings[0].reason, Some("unbind_failed"));
    assert_eq!((report.cleared, report.remaining), (0, 1));
    assert!(!report.complete());
    assert_eq!(unbinds(&provider), both_unbound());
    unchanged(&cma, &original.record_id);
    provider.close().await;
    cma.close().await;
}

#[tokio::test]
async fn one_version_conflict_is_reread_and_cleared() {
    let cma = FakeCma::start(false).await;
    let provider = provider(&[]).await;
    let (original, _) = seed(&cma, &key_ring(), routed_to_default(), false);
    cma.put_conflicts.store(1, Ordering::SeqCst);
    let report = clear(&cma, &provider, false).await;
    assert_eq!(report.findings[0].action, RoutingAuditAction::Cleared);
    assert!(report.complete());
    let (cleared, version) = readable_by_the_previous_image(&cma, &original.record_id);
    assert_eq!((cleared.version, version), (2, 3));
    // The re-read is classified again, but a removed binding is not removed twice.
    assert_eq!(unbinds(&provider), both_unbound());
    provider.close().await;
    cma.close().await;
}

#[tokio::test]
async fn a_second_conflict_is_reported_and_remains() {
    let cma = FakeCma::start(false).await;
    let provider = provider(&[]).await;
    let (original, _) = seed(&cma, &key_ring(), routed_to_default(), false);
    cma.put_conflicts.store(2, Ordering::SeqCst);
    let report = clear(&cma, &provider, false).await;
    assert_eq!(report.findings[0].action, RoutingAuditAction::Conflict);
    assert_eq!(report.findings[0].reason, Some("conflict"));
    assert_eq!((report.cleared, report.remaining), (0, 1));
    assert!(report.lines()[0].ends_with(r#""action":"conflict","reason":"conflict"}"#));
    let (payload, _) = stored(&cma, &key_ring(), &original.record_id);
    let kept: ChannelDefinition = serde_json::from_slice(&payload).unwrap();
    assert!(kept.routing.is_some());
    provider.close().await;
    cma.close().await;
}

#[tokio::test]
async fn an_unbind_without_a_provider_client_stops_the_audit() {
    let cma = FakeCma::start(false).await;
    let (original, _) = seed(&cma, &key_ring(), routed_to_default(), false);
    let error = audit_routing(
        &records(&cma),
        &key_ring(),
        None,
        &non_human(),
        RoutingAuditMode::Clear {
            allow_default_widening: false,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "provider configuration unavailable");
    unchanged(&cma, &original.record_id);
    cma.close().await;
}

#[tokio::test]
async fn a_record_listed_under_another_owner_is_neither_unbound_nor_rewritten() {
    let cma = FakeCma::start(false).await;
    let provider = provider(&[]).await;
    let keys = key_ring();
    let (original, _) = seed(&cma, &keys, routed_to_default(), false);
    cma.seed(
        original.record_id.as_str(),
        "another-owner",
        seal(&keys, &original),
    );
    let report = clear(&cma, &provider, false).await;
    assert_eq!(report.findings[0].action, RoutingAuditAction::Found);
    assert_eq!((report.cleared, report.remaining), (0, 1));
    assert!(provider.log().is_empty());
    unchanged(&cma, &original.record_id);
    provider.close().await;
    cma.close().await;
}

#[tokio::test]
async fn verified_relay_sources_block_rollback_without_downgrading_trust_or_unbinding() {
    for platform in ["discord", "whatsapp"] {
        for routed in [false, true] {
            let cma = FakeCma::start(false).await;
            let provider = provider(&[]).await;
            let mut value = if routed {
                routed_to_default()
            } else {
                legacy()
            };
            value["sources"] = json!([super::routing_fixture::relay(
                platform,
                super::routing_fixture::ROUTE
            )]);
            value["sources"][0]["verify_raw_event"] = true.into();
            value["partition"] = json!({"version":1,"key":"conversation"});
            value["reply"]["sinks"] = json!([{"type":"source"}]);
            let (original, _) = seed(&cma, &key_ring(), value, false);
            let (before, version) = stored(&cma, &key_ring(), &original.record_id);
            assert!(serde_json::from_slice::<previous_schema::ChannelDefinition>(&before).is_err());
            let report = audit_routing(
                &records(&cma),
                &key_ring(),
                Some(&providers(&provider)),
                &non_human(),
                RoutingAuditMode::Report,
            )
            .await
            .unwrap();
            assert_eq!(report.remaining, 1);
            assert!(report.findings[0].extensions.contains(&"sources"));
            for override_widening in [false, true] {
                let report = clear(&cma, &provider, override_widening).await;
                assert_eq!(
                    (report.cleared, report.blocked(), report.remaining),
                    (0, 1, 1)
                );
                assert_eq!(report.findings[0].action, RoutingAuditAction::Blocked);
                assert_eq!(report.findings[0].reason, Some("sources_need_owner"));
                assert!(!report.complete());
                assert_eq!(
                    stored(&cma, &key_ring(), &original.record_id),
                    (before.clone(), version)
                );
                assert!(provider.requests().is_empty());
            }
            provider.close().await;
            cma.close().await;
        }
    }
}

#[tokio::test]
async fn durable_custom_marker_blocks_routing_clear_before_unbind_or_write_even_with_override() {
    let cma = FakeCma::start(false).await;
    let provider = provider(&[]).await;
    let mut value = routed_to_default();
    value["durable_custom_execution"] = true.into();
    let (original, _) = seed(&cma, &key_ring(), value, true);
    let before = stored(&cma, &key_ring(), &original.record_id);
    for allow in [false, true] {
        let report = clear(&cma, &provider, allow).await;
        assert_eq!(report.blocked(), 1);
        assert_eq!(
            report.findings[0].reason,
            Some("durable_custom_execution_requires_reader")
        );
        assert_eq!(stored(&cma, &key_ring(), &original.record_id), before);
        assert!(provider.requests().is_empty());
    }
    provider.close().await;
    cma.close().await;
}
