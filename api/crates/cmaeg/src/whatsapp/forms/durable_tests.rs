use super::{test_fixture::*, *};
use cmaeg_core::{activity::ActivityKind, seal::event_ref::activity_digest};
use serde_json::json;

#[tokio::test]
async fn oversized_original_event_is_rejected_before_reserving_asset_capacity() {
    let mut f = Fixture::new(false).await;
    if let ActivityKind::Message { text, .. } = &mut f.base.context.activity.kind {
        *text = "x".repeat(32 * 1024);
    }
    f.base.context.activity.validate().unwrap();
    f.base.event.activity_sha256 = Some(activity_digest(&f.base.context.activity));
    assert!(f
        .base
        .store
        .prepare_whatsapp_flow(
            &f.base.channel,
            &f.base.event,
            &message(),
            f.base.context.activity.clone(),
            "key",
            &f.forms.provisioner.store
        )
        .await
        .is_err());
    assert!(f.base.server.memory.flows.lock().unwrap().is_empty());
}
#[tokio::test]
async fn maximum_bounded_completion_fits_one_sealed_record_and_reconstructs_exact_values() {
    let fields:Vec<_>=(0..12).map(|i|json!({"id":format!("business/{i}"),"label":format!("Field {i}"),"required":i<4,"kind":{"type":"text"}})).collect();
    let message =
        serde_json::from_value(json!({"blocks":[{"type":"form","id":"maximum","fields":fields}]}))
            .unwrap();
    let mut f = Fixture::with_message(true, &message).await;
    if let ActivityKind::Message { text, .. } = &mut f.base.context.activity.kind {
        *text = "x".repeat(9000);
    }
    f.base.event.activity_sha256 = Some(activity_digest(&f.base.context.activity));
    let card = f
        .base
        .store
        .prepare_whatsapp_flow(
            &f.base.channel,
            &f.base.event,
            &message,
            f.base.context.activity.clone(),
            "key",
            &f.forms.provisioner.store,
        )
        .await
        .unwrap();
    let a = card.native_whatsapp_flow.as_ref().unwrap();
    for _ in 0..5 {
        f.forms
            .provisioner
            .advance(&a.asset_registry)
            .await
            .unwrap();
    }
    f.forms
        .commit(
            &f.base.channel,
            &f.base.event,
            &message,
            &a.preparation_ref,
            "send",
            tokio::time::Instant::now() + std::time::Duration::from_secs(8),
        )
        .await
        .unwrap();
    let mut callback = f.callback(&card);
    callback.fields = (0..4)
        .map(|i| (format!("f{i}"), json!("😀".repeat(990))))
        .collect();
    assert!(callback.valid());
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
    let ActivityKind::Interaction(result) = pending.activity.kind else {
        panic!("interaction")
    };
    assert_eq!(result.values["fields"]["business/3"], "😀".repeat(990));
    let (saved, _) = f.base.store.load_whatsapp_flow(&card.id).await.unwrap();
    let value = serde_json::to_value(saved).unwrap();
    assert!(value["native_whatsapp_flow"]["pending"]
        .get("activity")
        .is_none());
    assert!(value["consumed"].get("submitted").is_none());
    assert!(
        f.base.server.memory.flows.lock().unwrap()[&card.id]
            .blob
            .len()
            <= 65536
    );
    assert_eq!(
        f.base
            .store
            .consume_whatsapp_flow(&f.base.channel, &callback)
            .await
            .unwrap(),
        card.id
    );
}
#[tokio::test]
async fn signed_webhook_rebatching_keeps_exact_completion_identity_and_removes_token() {
    use crate::whatsapp::{
        tests::{credentials, envelope, source, text},
        webhook,
    };
    let f = Fixture::new(false).await;
    let card = f.ready().await;
    f.commit(&card, "send").await.unwrap();
    let mut response = serde_json::to_value(values()).unwrap();
    response["flow_token"] = json!(crate::interactions::whatsapp_flow::token(&card));
    let raw = json!({"from":f.base.context.activity.actor.id,"id":"wamid.signed-completion","timestamp":crate::interactions::whatsapp_native_fixture::now().to_string(),"type":"interactive","context":{"id":"wamid.flow","from":"untrusted-context-from"},"interactive":{"type":"nfm_reply","nfm_reply":{"name":"flow","response_json":response.to_string()}}});
    let original = envelope(vec![raw.clone(), raw.clone()]);
    let secret = credentials();
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(
            ring::hmac::HMAC_SHA256,
            secret.app_secret.as_str().as_bytes(),
        ),
        &original,
    );
    let signature = format!("sha256={}", webhook::hex(tag.as_ref()));
    webhook::verify(secret.app_secret.as_str(), &signature, &original).unwrap();
    let extracted = webhook::extract(&source(), &original).unwrap();
    assert_eq!(extracted.len(), 1);
    let callback = callback::Callback::from_message(&source(), &extracted[0]).unwrap();
    assert!(!serde_json::to_string(&callback)
        .unwrap()
        .contains("flow_token"));
    f.base
        .store
        .consume_whatsapp_flow(&f.base.channel, &callback)
        .await
        .unwrap();
    let rebatch = envelope(vec![text("wamid.other", "Hi"), raw.clone()]);
    assert!(webhook::verify(secret.app_secret.as_str(), &signature, &rebatch).is_err());
    let rebatch = webhook::extract(&source(), &rebatch).unwrap();
    let duplicate = callback::Callback::from_message(&source(), &rebatch[1]).unwrap();
    assert!(callback == duplicate);
    assert_eq!(
        f.base
            .store
            .consume_whatsapp_flow(&f.base.channel, &duplicate)
            .await
            .unwrap(),
        card.id
    );
    let mut missing = raw.clone();
    missing.as_object_mut().unwrap().remove("context");
    assert!(webhook::extract(&source(), &envelope(vec![missing])).is_err());
    let mut conflicting = raw.clone();
    conflicting["interactive"]["nfm_reply"]["response_json"] = json!("{}");
    assert!(webhook::extract(&source(), &envelope(vec![raw, conflicting])).is_err());
}

#[tokio::test]
async fn card_namespace_and_default_named_seal_kinds_cannot_be_substituted() {
    use cmaeg_core::seal::{seal_consumer_interaction, seal_interaction};
    for named in [false, true] {
        let f = Fixture::new(named).await;
        let card = f.prepare().await;
        let bytes = serde_json::to_vec(&card).unwrap();
        let keys = crate::ingress::routing_plan::fixture::keys();
        let seal = if named {
            seal_interaction
        } else {
            seal_consumer_interaction
        };
        f.base
            .server
            .memory
            .flows
            .lock()
            .unwrap()
            .get_mut(&card.id)
            .unwrap()
            .blob = seal(&keys, &card.id, &bytes).unwrap();
        assert!(f.base.store.load_whatsapp_flow(&card.id).await.is_err());
        let seal = if named {
            seal_consumer_interaction
        } else {
            seal_interaction
        };
        f.base.server.memory.rows.lock().unwrap().insert(
            card.id.clone(),
            crate::ingress::routing_store::test_server::Row {
                owner: f.base.channel.definition.owner.subject.clone(),
                blob: seal(&keys, &card.id, &bytes).unwrap(),
                expiry: card.expires_at,
                version: 1,
            },
        );
        assert!(f.base.store.load(&card.id).await.is_err());
    }
}

// Called by the authenticated Mongo contract after the optional migration. This
// runs the composed card, asset, receipt and completion path under real validators.
pub(crate) async fn exercise_mongo(database: crate::records::mongo::MongoRecords) {
    use std::{sync::Arc, time::Duration};
    for named in [false, true] {
        let mut f = Fixture::new(named).await;
        let records = f
            .base
            .server
            .records
            .clone()
            .with_database(database.clone());
        let row = f.base.server.memory.channels.lock().unwrap()
            [f.base.channel.pointer.record_id.as_str()]
        .clone();
        records
            .put(
                &f.base.channel.pointer.record_id,
                &row.owner,
                &row.blob,
                None,
            )
            .await
            .unwrap();
        let keys = Arc::new(crate::ingress::routing_plan::fixture::keys());
        let resolver = crate::channels::resolver::ChannelResolver::new(
            Arc::new(records.clone()),
            keys.clone(),
            Default::default(),
        );
        f.base.store = Arc::new(crate::interactions::InteractionStore::new(
            records.clone(),
            keys.clone(),
            f.base.provider.client(true, Duration::from_secs(2)),
        ));
        f.forms = service::Forms::new(
            records.clone(),
            keys,
            resolver,
            f.base.store.as_ref().clone(),
            crate::whatsapp::Client::production()
                .unwrap()
                .with_transport(f.graph.clone()),
            true,
            true,
        );
        f.forms.provisioner.manager = f.management.clone();
        let card = f.ready().await;
        f.commit(&card, "mongo-send").await.unwrap();
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
        crate::interactions::whatsapp_flow::current_dispatch(
            &records,
            &Arc::new(crate::ingress::routing_plan::fixture::keys()),
            &f.base.channel,
            &pending.activity,
            &pending.event,
        )
        .await
        .unwrap();
        let stored = database
            .for_kind(crate::records::RecordKind::WhatsappFlow)
            .get(&card.id)
            .await
            .unwrap();
        assert!(stored.version >= 4);
        assert!(database
            .for_kind(crate::records::RecordKind::Interaction)
            .get(&card.id)
            .await
            .is_err());
        assert_eq!(f.graph.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn slow_remote_proof_cannot_cross_preparation_or_send_admission_expiry() {
    use std::sync::atomic::Ordering;
    for commit in [false, true] {
        let mut f = Fixture::new(false).await;
        f.base.event.expires_at = crate::interactions::whatsapp_native_fixture::now() + 2;
        let card = if commit {
            f.ready().await
        } else {
            f.prepare().await
        };
        f.management.verify_delay.store(true, Ordering::SeqCst);
        let result = if commit {
            f.commit(&card, "late-send").await.map(|_| ())
        } else {
            f.forms
                .provisioner
                .advance(&card.native_whatsapp_flow.as_ref().unwrap().asset_registry)
                .await
        };
        assert_eq!(result, Err(Error::Revoked));
        assert_eq!(f.graph.calls.load(Ordering::SeqCst), 0);
        if !commit {
            assert_eq!(f.management.creates.load(Ordering::SeqCst), 0);
        }
    }
}
