//! A stopped multi-binding audit resumes only its durable completed steps.
use super::{
    audit::AuditReport,
    audit_routing_clear::{clear, Clearing},
    management,
    mutation_fixture::{owner, Fixture},
};
use cmaeg_core::channel::Secret;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
#[tokio::test]
async fn audit_resumes_after_known_unsent_second_binding_without_repeating_first() {
    let fixture = Fixture::new(false, true).await;
    let state = fixture.state().await;
    let creator = owner("alice", false);
    let created = management::create(
        &state,
        &creator,
        serde_json::from_value(fixture.body()).unwrap(),
    )
    .await
    .unwrap();
    let mut definition = created.definition;
    let routed = super::routing_fixture::definition(super::routing_fixture::routed());
    definition.routing = routed.routing;
    definition.secrets.consumer_keys = routed.secrets.consumer_keys;
    definition.validate().unwrap();
    let blob = management::seal_definition(&fixture.keys, &definition).unwrap();
    let version = state
        .records
        .put(
            &definition.record_id,
            &creator.subject,
            &blob,
            Some(created.version),
        )
        .await
        .unwrap();
    let non_human = BTreeSet::from(["cma_codex".into()]);
    let head = crate::records::RecordHead {
        record_id: definition.record_id.to_string(),
        owner: creator.subject.clone(),
        version,
    };
    let clearing = Clearing {
        records: &state.records,
        keys: &fixture.keys,
        providers: Some(&state.providers),
        non_human: &non_human,
        allow_default_widening: true,
    };
    let outcome = clear(
        &clearing,
        &head,
        definition,
        version,
        &mut AuditReport::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome.action,
        super::audit_routing::RoutingAuditAction::Blocked
    );
    assert_eq!(
        outcome.reason,
        Some(super::audit_routing_clear::UNBIND_FAILED)
    );
    let retained = management::load(&state, &creator, &created.channel_id)
        .await
        .unwrap();
    assert_eq!(
        retained
            .definition
            .mutation
            .as_ref()
            .unwrap()
            .completed
            .len(),
        1
    );
    assert!(retained
        .definition
        .mutation
        .as_ref()
        .unwrap()
        .effect
        .is_none());
    let mut other = fixture.state().await;
    let registry=crate::providers::ProviderRegistry::parse(&serde_json::json!(["cma_codex","nyxbot"].map(|slug|serde_json::json!({"slug":slug,"base_url":fixture.base,"kind":"responses_http","paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/{conversation_id}","responses":"/responses"},"max_inflight":8}))).to_string()).unwrap();
    other.providers = Arc::new(
        crate::providers::ProviderClient::new(
            registry,
            Duration::from_secs(1),
            Duration::from_secs(1),
            8,
        )
        .unwrap(),
    );
    let clearing = Clearing {
        records: &other.records,
        keys: &fixture.keys,
        providers: Some(&other.providers),
        non_human: &non_human,
        allow_default_widening: true,
    };
    let outcome = clear(
        &clearing,
        &head,
        retained.definition,
        retained.record.version,
        &mut AuditReport::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome.action,
        super::audit_routing::RoutingAuditAction::Cleared
    );
    assert_eq!(
        *fixture.peer.calls.lock().unwrap(),
        ["PUT", "DELETE", "DELETE"]
    );
    let result = management::load(&other, &creator, &created.channel_id)
        .await
        .unwrap();
    assert!(result.definition.routing.is_none());
    assert!(result.definition.mutation.is_none());
    // A source credential remains private and untouched by topology cleanup.
    assert_eq!(
        result.definition.secrets.channel_key,
        Some(Secret::new("fixture-channel-key"))
    );
    fixture.cleanup().await;
}
