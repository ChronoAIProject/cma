//! Compatibility counterexample for the proposed early rollback predicate.
//! Exercise the unchanged pre-quarantine reader against the proposed post-release
//! record state. This does not implement or invoke the operator command.
use super::{super::replica_test_fixture::*, mongo_progress_fixture::*, *};
use crate::{
    delivery::{durable::receipt_for, intents::DeliveryOutcome},
    turns::routed::{storage, tool_reply_store::ReplyStore},
};
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn pre_quarantine_reader_sends_fresh_key_after_all_queued_effects_are_refused_and_retired() {
    with_harness(|h| Box::pin(async move {
        let (owner, mut lease) = h.accepted().await;
        let journal = h.nodes[owner].journal();
        journal.transition(&mut lease, Phase::Unknown).await.unwrap();
        let mut capability = h.capability();
        capability.issued_at -= 5;
        let payload = Payload {
            capability: capability.clone(), caller: caller().key_hash, message: message(),
        };
        let store = Store(journal.clone());
        let mut queued = store.admit(&h.plan, h.target(), "queued-before-abandonment", &payload).await.unwrap();
        journal.enqueue_effect(&h.plan, h.target(), &queued.id).await.unwrap();

        // Seed the approved release's intended durable result. The new sidecar
        // is never selected by this reader; leave the original Unknown evidence.
        let quarantine_id = storage::identifier(&("replica-target-quarantine-v1", &h.target().id));
        journal.write(&quarantine_id, &h.plan.identity.owner, h.plan.retained_until,
            &serde_json::json!({"kind":"replica_target_quarantine_v1", "target":h.target().id,
                "disposition":"unknown_abandoned"}), None).await.unwrap();
        ReplyStore(journal.clone(), lease.partition.clone()).close(&h.plan, h.target()).await.unwrap();
        h.nodes[owner].partitions.complete(lease.partition.as_ref().unwrap()).await.unwrap();

        // Every existing quarantined effect now has its saved refusal and a
        // retired FIFO entry: condition D's proposed early rollback criterion.
        let mut claimed = None;
        for (index, node) in h.nodes.iter().enumerate() {
            if let Ok(token) = node.claim_effect(&h.plan, h.target(), &queued.id).await {
                claimed = Some((index, token)); break;
            }
        }
        let (effect_owner, token) = claimed.expect("queued effect must own the next head");
        let effect_store = Store(h.nodes[effect_owner].journal());
        let refusal = receipt_for(DeliveryOutcome::Rejected { reason: "unknown_abandoned".into() });
        let stored = effect_store.0.receipts(&queued.operation, &h.plan.identity.owner, h.plan.retained_until)
            .retain(&refusal).await.unwrap();
        effect_store.transition(&mut queued, &token, model::State::Settled { receipt: stored }).await.unwrap();
        h.nodes[effect_owner].partitions.complete(&token).await.unwrap();
        assert!(journal.effect_roots(None, 32).await.unwrap().is_empty());
        let replay = h.tools[1].reply_delayed(caller(), &capability, "queued-before-abandonment", message())
            .await.unwrap().unwrap();
        assert!(matches!(replay.delivery, DeliveryOutcome::Rejected { .. }));
        let guard_id = storage::identifier(&("ordinary-route-reply-guard-v1", &h.target().id));
        let (guard, _) = journal.read::<serde_json::Value>(&guard_id).await.unwrap().unwrap();
        assert_eq!(guard["closed"], true);
        assert!(capability.expires_at > (journal.clock)());

        // A rollback binary still accepts another occurrence for this same
        // abandoned target; closing the ordinary guard does not fence this path.
        let key = "fresh-after-rollback";
        assert!(matches!(h.tools[1].reply_delayed(caller(), &capability, key, message()).await,
            Err(ExecutionError::Busy)));
        let id = effect_id(h, &capability, key);
        let (sink, attempts) = transport(h, Duration::ZERO);
        sweep(h, &id).await;
        let result = h.tools[0].reply_delayed(caller(), &capability, key, message()).await.unwrap().unwrap();
        assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        sink.abort();
        println!("rollback_counterexample prior_effects=refused_and_retired guard=closed fresh_key=confirmed sends=1");
    })).await;
}
