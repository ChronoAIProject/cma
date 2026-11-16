//! Quarantine must fence fresh keys and minting, not just already queued effects.
use super::{super::replica_test_fixture::*, mongo_progress_fixture::*, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    replicas::unresolved::{Operator, Request},
    turns::routed::{test_support::recovery_frame, ReplayEffect, RoutedDelivery},
};

pub(super) fn operator(h: &Harness) -> Operator {
    Operator::new(
        h.nodes[0].partitions.clone(),
        h.nodes[0].plans(),
        h.nodes[0].journal(),
    )
}
pub(super) async fn request(h: &Harness) -> Request {
    let operator = operator(h);
    let listed = operator.inspect().await.unwrap();
    let mut req = Request {
        binding: listed.candidates[0].binding.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
        reason: "fixture reviewed abandoned turn".into(),
        plan_digest: None,
    };
    req.plan_digest = Some(operator.preview(&req).await.unwrap().plan_digest);
    req
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn abandonment_refuses_fresh_keys_minting_immediate_and_queued_replies_without_io() {
    with_harness(|h| Box::pin(async move {
        let (owner, mut lease) = h.accepted().await;
        let journal = h.nodes[owner].journal();
        journal.transition(&mut lease, Phase::Unknown).await.unwrap();
        journal.release(&mut lease).await.unwrap();
        let mut capability = h.capability(); capability.issued_at -= 5;
        let key = "queued-before-abandonment";
        assert!(matches!(h.tools[0].reply_delayed(caller(), &capability, key, message()).await, Err(ExecutionError::Busy)));
        let id = effect_id(h, &capability, key);
        let operator = operator(h);
        let req = request(h).await;
        let before = operator.inspect().await.unwrap();
        let preview = operator.preview(&req).await.unwrap();
        assert!(preview.candidate.eligible);
        assert_eq!(operator.inspect().await.unwrap().candidates[0].binding.head_version, before.candidates[0].binding.head_version);
        assert!(h.rx.try_recv().is_err());
        let receipt = operator.apply(&req, 1).await.unwrap();
        assert_eq!(receipt.disposition, "unknown_abandoned");
        assert_eq!(receipt.rollback_safe_after, time::OffsetDateTime::from_unix_timestamp(h.event().expires_at + cmaeg_core::seal::REPLY_TARGET_TTL).unwrap().format(&time::format_description::well_known::Rfc3339).unwrap());
        assert_eq!(operator.inspect().await.unwrap().outstanding_quarantined_effects, 1);
        assert_eq!(operator.apply(&req, 1).await.unwrap().evidence_sha256, receipt.evidence_sha256);
        assert_eq!(operator.audit(&req.request_id).await.unwrap().disposition, "unknown_abandoned");
        let (parent, _) = journal.read::<TargetState>(&h.target().id).await.unwrap().unwrap();
        assert_eq!(parent.phase, Phase::Unknown, "acceptance evidence cannot be relabeled");
        for key in ["queued-before-abandonment", "fresh-key"] {
            assert!(matches!(h.tools[1].reply_delayed(caller(), &capability, key, message()).await, Err(ExecutionError::Abandoned)));
        }
        assert_eq!(h.tools[1].authorize_reply_target_mint(&h.event()).await, Err(ExecutionError::Abandoned));
        let channel = h.current.channel.lock().unwrap().clone();
        assert!(matches!(h.tools[1].reply(caller(), &channel, &h.event(), message(), false).await, Err(ExecutionError::Abandoned)));
        sweep(h, &id).await;
        let root = Store(journal.clone()).read(&id, &h.plan, h.target()).await.unwrap().unwrap();
        assert!(matches!(Store(journal.clone()).receipt(&root).await.unwrap().unwrap().delivery, DeliveryOutcome::Rejected { ref reason } if reason == "unknown_abandoned"));
        let listing = operator.inspect().await.unwrap();
        assert_eq!(listing.outstanding_quarantined_effects, 0);
        assert_eq!(listing.rollback_safe_after, Some(receipt.rollback_safe_after));
        assert!(h.rx.try_recv().is_err(), "dry-run/apply/recovery may never send");
        // A stale automatic invocation has neither target authority nor a send.
        let delivery = RoutedDelivery::new(h.nodes[owner].plans(), journal, h.current.clone(), h.tools[owner].coordinator.clone());
        assert!(delivery.deliver_frame(&h.plan.identity, &h.target().id, &mut lease, &recovery_frame("fixture-response", 0, true), ReplayEffect::Reply(Box::new(message()))).await.is_err());
        assert!(h.rx.try_recv().is_err());
    })).await;
}
