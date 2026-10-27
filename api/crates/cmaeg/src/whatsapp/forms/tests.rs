use super::{compile, service::FormPreparationState, test_fixture::*, values, Error};
use crate::delivery::intents::DeliveryOutcome;
use serde_json::json;
use std::sync::atomic::Ordering;

#[test]
fn all_four_fields_are_static_typed_and_do_not_expose_business_ids() {
    let compiled = compile::compile(&message()).unwrap();
    assert_eq!(compiled.json["version"], "7.3");
    let raw = compiled.json.to_string();
    assert!(!raw.contains("business/name"));
    assert!(!raw.contains("endpoint"));
    assert!(raw.contains("CalendarPicker"));
    assert!(raw.contains("init-values"));
    let got = values::submitted(&message(), &values()).unwrap();
    assert_eq!(got["amount"], json!(-250.0));
    assert_eq!(got["when"], "2028-02-29");
    assert_eq!(got["choice"], "yes");
    let mut fields = values();
    fields.remove("f3");
    assert!(!values::submitted(&message(), &fields)
        .unwrap()
        .contains_key("choice"));
    fields.insert("f3".into(), json!(null));
    assert!(!values::submitted(&message(), &fields)
        .unwrap()
        .contains_key("choice"));
}
#[test]
fn malformed_extra_duplicate_and_oversize_completions_are_refused() {
    for raw in [
        r#"{"flow_token":"a","f0":"x","f0":"y"}"#,
        r#"{"flow_token":"a","f0":{}}"#,
        r#"{"flow_token":"a","flow_token":"b"}"#,
    ] {
        assert!(values::decode(raw).is_err());
    }
    for (key, value) in [
        ("f1", json!("01")),
        ("f1", json!(" NaN")),
        ("f1", json!("1e9999")),
        ("f2", json!("2025-02-29")),
        ("f2", json!("2026-01-01+08:00")),
        ("f3", json!("01")),
        ("f3", json!("2")),
        ("f4", json!("x")),
        ("f00", json!("x")),
        ("f0", json!("x".repeat(1001))),
    ] {
        let mut fields = values();
        fields.insert(key.into(), value);
        assert!(values::submitted(&message(), &fields).is_err(), "{key}");
    }
    let mut fields = values();
    fields.remove("f0");
    assert!(values::submitted(&message(), &fields).is_err());
    assert!(super::unique_json::decode(br#"{"a":{"b":1,"b":2}}"#).is_err());
}
#[test]
fn mixed_or_oversize_messages_are_refused_before_preparation() {
    let mut msg = message();
    msg.text = Some("x".repeat(1025));
    assert!(compile::compile(&msg).is_err());
    let mut msg = message();
    msg.thread_reply = true;
    assert!(compile::compile(&msg).is_err());
    let mut msg = message();
    msg.blocks.push(msg.blocks[0].clone());
    assert!(compile::compile(&msg).is_err());
    let mut msg = message();
    msg.blocks.push(
        serde_json::from_value(
            json!({"type":"buttons","id":"b","options":[{"id":"x","label":"X"}]}),
        )
        .unwrap(),
    );
    assert!(compile::compile(&msg).is_err());
}
#[tokio::test]
async fn preparation_reuses_one_slot_and_management_never_sends() {
    let f = Fixture::new(false).await;
    let card = f.prepare().await;
    assert_eq!(f.prepare().await.id, card.id);
    assert!(f
        .base
        .store
        .prepare_whatsapp_flow(
            &f.base.channel,
            &f.base.event,
            &message(),
            f.base.context.activity.clone(),
            "new-key",
            &f.forms.provisioner.store
        )
        .await
        .is_err());
    let pending = f
        .forms
        .status(&f.base.channel, &f.base.event)
        .await
        .unwrap();
    assert!(matches!(
        pending.state,
        FormPreparationState::Pending { .. }
    ));
    let card = f.ready().await;
    assert_eq!(f.management.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.management.publishes.load(Ordering::SeqCst), 1);
    assert_eq!(f.graph.calls.load(Ordering::SeqCst), 0);
    assert!(card.expires_at > f.base.event.expires_at);
}
#[tokio::test]
async fn concurrent_create_and_publish_attempts_are_fenced() {
    let f = Fixture::new(false).await;
    let card = f.prepare().await;
    let id = &card.native_whatsapp_flow.as_ref().unwrap().asset_registry;
    let _ = tokio::join!(
        f.forms.provisioner.advance(id),
        f.forms.provisioner.advance(id)
    );
    for _ in 0..5 {
        let _ = tokio::join!(
            f.forms.provisioner.advance(id),
            f.forms.provisioner.advance(id)
        );
    }
    assert_eq!(f.management.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.management.publishes.load(Ordering::SeqCst), 1);
    assert_eq!(f.graph.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn unknown_create_and_publish_reconcile_without_repeating_mutations() {
    let f = Fixture::new(false).await;
    f.management.unknown_create.store(true, Ordering::SeqCst);
    f.management.unknown_publish.store(true, Ordering::SeqCst);
    let card = f.prepare().await;
    let id = &card.native_whatsapp_flow.as_ref().unwrap().asset_registry;
    f.forms.provisioner.advance(id).await.unwrap();
    f.management.ambiguous.store(true, Ordering::SeqCst);
    f.forms.provisioner.advance(id).await.unwrap();
    assert!(matches!(
        f.forms
            .status(&f.base.channel, &f.base.event)
            .await
            .unwrap()
            .state,
        FormPreparationState::Uncertain { .. }
    ));
    f.management.ambiguous.store(false, Ordering::SeqCst);
    for _ in 0..6 {
        f.forms.provisioner.advance(id).await.unwrap();
    }
    assert!(matches!(
        f.forms
            .status(&f.base.channel, &f.base.event)
            .await
            .unwrap()
            .state,
        FormPreparationState::Ready { .. }
    ));
    assert_eq!(f.management.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.management.publishes.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn explicit_commit_is_one_send_across_retries_and_other_keys() {
    let f = Fixture::new(true).await;
    let card = f.ready().await;
    let (left, right) = tokio::join!(f.commit(&card, "commit"), f.commit(&card, "commit"));
    assert!(left.is_ok() || right.is_ok());
    let receipt = f.commit(&card, "commit").await.unwrap();
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(receipt.message_ref.is_some());
    assert_eq!(
        f.commit(&card, "new-key").await.unwrap_err(),
        Error::Conflict
    );
    assert_eq!(f.graph.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn unknown_send_remains_fenced_and_does_not_issue_a_reference() {
    let f = Fixture::new(false).await;
    let card = f.ready().await;
    f.graph.unknown.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        let receipt = f.commit(&card, "commit").await.unwrap();
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(receipt.message_ref.is_none());
    }
    assert_eq!(
        f.commit(&card, "new-key").await.unwrap_err(),
        Error::Conflict
    );
    assert_eq!(f.graph.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn callbacks_bind_sender_wamid_source_schema_and_one_durable_winner() {
    let f = Fixture::new(true).await;
    let card = f.ready().await;
    f.commit(&card, "commit").await.unwrap();
    let callback = f.callback(&card);
    for index in 0..6 {
        let mut bad = callback.clone();
        match index{0=>bad.sender="9999".into(),1=>bad.context="wamid.other".into(),2=>bad.revision=2,3=>bad.fields.insert("f4".into(),json!("bad")).map(|_|()).unwrap_or(()),4=>bad.timestamp=1,_=>bad.source=serde_json::from_value(json!({"type":"whatsapp","app_id":"99999","waba_id":"67890","phone_number_id":"11111"})).unwrap()};
        assert!(f
            .base
            .store
            .consume_whatsapp_flow(&f.base.channel, &bad)
            .await
            .is_err());
    }
    let id = f
        .base
        .store
        .consume_whatsapp_flow(&f.base.channel, &callback)
        .await
        .unwrap();
    assert_eq!(id, card.id);
    assert_eq!(
        f.base
            .store
            .consume_whatsapp_flow(&f.base.channel, &callback)
            .await
            .unwrap(),
        id
    );
    let mut different = callback.clone();
    different.id = "wamid.other-completion".into();
    assert!(f
        .base
        .store
        .consume_whatsapp_flow(&f.base.channel, &different)
        .await
        .is_err());
    let pending = f
        .base
        .store
        .whatsapp_flow_pending(&id, &f.base.channel)
        .await
        .unwrap();
    assert!(!pending.admitted);
    assert!(!serde_json::to_string(&pending)
        .unwrap()
        .contains("flow_token"));
    let page = f
        .base
        .store
        .whatsapp_flow_recovery_page(None)
        .await
        .unwrap();
    assert!(page.cards.iter().any(|(id, _)| id == &card.id));
    crate::interactions::whatsapp_flow::current_dispatch(
        &f.base.server.records,
        &std::sync::Arc::new(crate::ingress::routing_plan::fixture::keys()),
        &f.base.channel,
        &pending.activity,
        &pending.event,
    )
    .await
    .unwrap();
    f.base.store.whatsapp_flow_admitted(&id).await.unwrap();
    assert!(
        f.base
            .store
            .whatsapp_flow_pending(&id, &f.base.channel)
            .await
            .unwrap()
            .admitted
    );
}
#[tokio::test]
async fn gate_off_keeps_status_and_bound_callbacks_but_consent_withdrawal_revokes() {
    let mut f = Fixture::new(false).await;
    let card = f.ready().await;
    f.commit(&card, "commit").await.unwrap();
    f.forms.issuance_enabled = false;
    f.forms.provisioner.writes_enabled = false;
    assert!(matches!(
        f.forms
            .status(&f.base.channel, &f.base.event)
            .await
            .unwrap()
            .state,
        FormPreparationState::Delivery { .. }
    ));
    f.base
        .store
        .consume_whatsapp_flow(&f.base.channel, &f.callback(&card))
        .await
        .unwrap();
    if let cmaeg_core::channel::Source::Whatsapp { forms_enabled, .. } =
        &mut std::sync::Arc::make_mut(&mut f.base.channel)
            .definition
            .sources[0]
    {
        *forms_enabled = false;
    }
    f.base.save_channel();
    assert!(f
        .forms
        .status(&f.base.channel, &f.base.event)
        .await
        .is_err());
}

#[tokio::test]
async fn confirmed_unbound_lost_ack_recovers_locally_without_a_second_send() {
    let f = Fixture::new(false).await;
    let card = f.ready().await;
    f.base.server.fault_after(card.id.clone(), true, true, 1);
    assert!(f.commit(&card, "commit").await.is_err());
    let page = f
        .base
        .store
        .whatsapp_flow_recovery_page(None)
        .await
        .unwrap();
    assert!(page.cards.iter().any(|(id, _)| id == &card.id));
    f.base
        .store
        .recover_whatsapp_flow_binding(&card.id)
        .await
        .unwrap();
    assert!(matches!(
        f.commit(&card, "commit").await.unwrap().delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(f.graph.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn expired_commit_budget_cannot_cross_the_attempt_boundary() {
    let f = Fixture::new(false).await;
    let card = f.ready().await;
    assert_eq!(
        f.forms
            .commit(
                &f.base.channel,
                &f.base.event,
                &message(),
                &card.native_whatsapp_flow.as_ref().unwrap().preparation_ref,
                "commit",
                tokio::time::Instant::now()
            )
            .await
            .unwrap_err(),
        Error::Unavailable
    );
    assert_eq!(f.graph.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        f.forms
            .status(&f.base.channel, &f.base.event)
            .await
            .unwrap()
            .state,
        FormPreparationState::Ready { .. }
    ));
}
#[tokio::test]
async fn completion_retention_survives_original_event_expiry_and_admission_marker_gap() {
    use crate::ingress::{launch::LaunchRequest, routing_store::Candidate};
    let mut f = Fixture::new(false).await;
    f.base.event.expires_at = crate::interactions::whatsapp_native_fixture::now() + 2;
    f.base.context.event_ref = cmaeg_core::seal::EventRef::seal(
        &f.base.event,
        &crate::ingress::routing_plan::fixture::keys(),
    )
    .unwrap();
    let card = f.ready().await;
    f.commit(&card, "commit").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let callback = f.callback(&card);
    f.base
        .store
        .consume_whatsapp_flow(&f.base.channel, &callback)
        .await
        .unwrap();
    let pending = f
        .base
        .store
        .whatsapp_flow_pending(&card.id, &f.base.channel)
        .await
        .unwrap();
    let request = LaunchRequest {
        channel: f.base.channel.clone(),
        activity: pending.activity.clone(),
        conversation_id: pending.event.conversation_id.clone(),
        event_ref: pending.reference.clone(),
        consumer: pending.event.consumer.clone(),
        idempotency_key: cmaeg_core::ids::IdempotencyKey::from_event(&pending.activity.event_id),
        received_at: std::time::Instant::now(),
        trace: tracing::Span::none(),
        timing: None,
    };
    let keys = crate::ingress::routing_plan::fixture::keys();
    let now = crate::interactions::whatsapp_native_fixture::now();
    let plan = crate::ingress::native_interaction::plan(&request, &keys, now).unwrap();
    let bytes = serde_json::to_vec(&callback).unwrap();
    let candidate =
        Candidate::prepare_native_interaction(&request, &plan, &bytes, &keys, now).unwrap();
    let admitted = f.base.server.store().admit(&candidate).await.unwrap();
    assert_eq!(
        serde_json::to_value(&admitted).unwrap()["kind"],
        "native_whatsapp_flow_interaction_v1"
    );
    assert_eq!(
        f.base.server.store().admit(&candidate).await.unwrap(),
        admitted
    );
    f.base.server.fault(card.id.clone(), true, true);
    assert!(f.base.store.whatsapp_flow_admitted(&card.id).await.is_err());
    f.base.store.whatsapp_flow_admitted(&card.id).await.unwrap();
    assert!(
        f.base
            .store
            .whatsapp_flow_pending(&card.id, &f.base.channel)
            .await
            .unwrap()
            .admitted
    );
}

#[test]
fn forms_consent_does_not_change_existing_group_or_control_source_hashes() {
    let source = crate::whatsapp::tests::source();
    let credentials = crate::whatsapp::tests::credentials();
    let mut consented = source.clone();
    if let cmaeg_core::channel::Source::Whatsapp { forms_enabled, .. } = &mut consented {
        *forms_enabled = true;
    }
    assert_eq!(
        crate::whatsapp::legacy_source(&source),
        crate::whatsapp::legacy_source(&consented)
    );
    assert_eq!(
        crate::whatsapp::group_revision(&source, &credentials),
        crate::whatsapp::group_revision(&consented, &credentials)
    );
}
#[tokio::test]
async fn real_preparation_reopens_only_the_exact_selected_durable_event() {
    use crate::ingress::{
        launch::LaunchRequest,
        routing_plan::{self, OrdinaryPlan, RoutingOptions},
        routing_store::Candidate,
    };
    let mut f = Fixture::new(false).await;
    let keys = crate::ingress::routing_plan::fixture::keys();
    let now = crate::interactions::whatsapp_native_fixture::now();
    let request = LaunchRequest {
        channel: f.base.channel.clone(),
        activity: f.base.context.activity.clone(),
        conversation_id: f.base.event.conversation_id.clone(),
        event_ref: f.base.context.event_ref.clone(),
        consumer: None,
        idempotency_key: cmaeg_core::ids::IdempotencyKey::from_event(&f.base.event.event_id),
        received_at: std::time::Instant::now(),
        trace: tracing::Span::none(),
        timing: None,
    };
    let OrdinaryPlan::Routed(plan) = routing_plan::plan(
        &request,
        &keys,
        now,
        RoutingOptions {
            enabled: true,
            attributes_signed: true,
            non_human_providers: &Default::default(),
        },
    )
    .unwrap() else {
        panic!("durable plan required")
    };
    let candidate =
        Candidate::prepare(&request, &plan, b"verified fixture body", &keys, now).unwrap();
    f.base.server.store().admit(&candidate).await.unwrap();
    f.base.event = plan
        .targets
        .iter()
        .find(|t| t.consumer.is_default())
        .unwrap()
        .event
        .clone();
    let receipt = f
        .forms
        .prepare(&f.base.channel, &f.base.event, &message(), "key")
        .await
        .unwrap();
    assert!(matches!(
        receipt.state,
        FormPreparationState::Pending { .. }
    ));
    assert_eq!(
        f.forms
            .prepare(&f.base.channel, &f.base.event, &message(), "key")
            .await
            .unwrap()
            .preparation_id,
        receipt.preparation_id
    );
    assert_eq!(
        f.forms
            .prepare(&f.base.channel, &f.base.event, &message(), "other")
            .await
            .unwrap_err(),
        Error::Conflict
    );
    let named = plan
        .targets
        .iter()
        .find(|t| !t.consumer.is_default())
        .unwrap()
        .event
        .clone();
    let other = f
        .forms
        .prepare(&f.base.channel, &named, &message(), "key")
        .await
        .unwrap();
    assert_ne!(other.preparation_id, receipt.preparation_id);
    assert_eq!(f.graph.calls.load(Ordering::SeqCst), 0);
}
