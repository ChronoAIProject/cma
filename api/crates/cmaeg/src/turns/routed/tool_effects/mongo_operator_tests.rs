use super::{
    super::replica_test_fixture::*,
    mongo_progress_fixture::*,
    mongo_rollback_tests::{operator, request},
    *,
};
use crate::{
    replicas::{unresolved::Request, Error},
    turns::routed::tool_reply_store::{Admission, Kind, ReplyStore},
};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn operator_refuses_live_leases_wrong_bindings_and_missing_member_capability() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            let op = operator(h);
            assert!(
                op.inspect().await.unwrap().candidates.is_empty(),
                "healthy Accepted is not eligible"
            );
            journal
                .transition(&mut lease, Phase::Unknown)
                .await
                .unwrap();
            let req = request(h).await;
            assert!(op.preview(&req).await.unwrap().candidate.lease_live);
            assert!(matches!(op.apply(&req, 1).await, Err(Error::Busy)));
            journal.release(&mut lease).await.unwrap();
            let req = request(h).await;
            for kind in 0..4 {
                let mut changed = req.clone();
                match kind {
                    0 => changed.binding.head_version += 1,
                    1 => changed.binding.head_work = uuid::Uuid::new_v4().to_string(),
                    2 => changed.binding.partition = uuid::Uuid::new_v4().to_string(),
                    _ => changed.plan_digest = Some("incorrect".into()),
                }
                assert!(op.apply(&changed, 1).await.is_err());
            }
            h.nodes[1].hide_capability().await;
            tokio::time::sleep(Duration::from_millis(3200)).await;
            assert!(op
                .preview(&req)
                .await
                .unwrap()
                .members
                .iter()
                .any(|member| !member.quarantine));
            assert!(matches!(op.apply(&req, 1).await, Err(Error::Lost)));
            assert_eq!(
                op.inspect().await.unwrap().candidates[0]
                    .binding
                    .head_version,
                req.binding.head_version
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn operator_releases_accepted_guard_unknown_without_changing_acceptance_evidence() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            // A retained historical guard, admitted 31 seconds before this fixture.
            // Partition fencing still uses real Mongo time, not this guard timestamp.
            let mut past = journal.clone();
            let now = (journal.clock)();
            past.clock = Arc::new(move || now - 31);
            assert!(matches!(
                ReplyStore(past, lease.partition.clone())
                    .admit(&h.plan, h.target(), Kind::Automatic, "b".repeat(64))
                    .await
                    .unwrap(),
                Admission::Claimed(_)
            ));
            journal.release(&mut lease).await.unwrap();
            let op = operator(h);
            let req = request(h).await;
            assert_eq!(
                op.preview(&req).await.unwrap().candidate.state,
                "accepted_guard_unknown"
            );
            let before = journal
                .read::<TargetState>(&h.target().id)
                .await
                .unwrap()
                .unwrap();
            let receipt = op.apply(&req, 1).await.unwrap();
            let after = journal
                .read::<TargetState>(&h.target().id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                super::super::storage::bytes(&before.0).unwrap(),
                super::super::storage::bytes(&after.0).unwrap()
            );
            assert_eq!(before.1, after.1);
            assert_eq!(receipt.prior_state, "accepted_guard_unknown");
            assert!(op.inspect().await.unwrap().candidates.is_empty());
            let mut changed = req;
            changed.reason = "another reason".into();
            assert!(op.apply(&changed, 1).await.is_err());
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn operator_retires_abandoned_sending_effect_without_replaying_it() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            let mut capability = h.capability();
            capability.issued_at -= 5;
            let payload = Payload {
                capability,
                caller: caller().key_hash,
                message: message(),
            };
            let mut root = Store(h.nodes[0].journal())
                .admit(&h.plan, h.target(), "sending", &payload)
                .await
                .unwrap();
            h.nodes[0]
                .journal()
                .enqueue_effect(&h.plan, h.target(), &root.id)
                .await
                .unwrap();
            let mut held = None;
            for (index, node) in h.nodes.iter().enumerate() {
                if let Ok(token) = node.claim_effect(&h.plan, h.target(), &root.id).await {
                    held = Some((index, token));
                    break;
                }
            }
            let (owner, token) = held.unwrap();
            Store(h.nodes[owner].journal())
                .transition(&mut root, &token, model::State::Sending)
                .await
                .unwrap();
            let op = operator(h);
            let mut req = Request {
                binding: h.nodes[owner].work_binding(&token).await,
                request_id: uuid::Uuid::new_v4().to_string(),
                reason: "live effect fixture".into(),
                plan_digest: None,
            };
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            assert!(!op.preview(&req).await.unwrap().candidate.eligible);
            assert!(matches!(op.apply(&req, 1).await, Err(Error::Busy)));
            h.nodes[owner].partitions.release(&token).await.unwrap();
            let req: Request = request(h).await;
            assert_eq!(
                op.preview(&req).await.unwrap().candidate.state,
                "effect_abandoned_sending"
            );
            op.apply(&req, 1).await.unwrap();
            assert!(h.nodes[0]
                .journal()
                .effect_roots(None, 32)
                .await
                .unwrap()
                .is_empty());
            assert_eq!(
                op.inspect().await.unwrap().outstanding_quarantined_effects,
                1,
                "unknown send is not a fabricated saved refusal"
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn outstanding_quarantine_excludes_prior_confirmed_retired_occurrences() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut parent) = h.accepted().await;
            h.finish_parent(owner, &mut parent).await;
            let mut capability = h.capability();
            capability.issued_at -= 5;
            let payload = Payload {
                capability: capability.clone(),
                caller: caller().key_hash,
                message: message(),
            };
            let store = Store(h.nodes[0].journal());
            let first = store
                .admit(&h.plan, h.target(), "already-confirmed", &payload)
                .await
                .unwrap();
            h.nodes[0]
                .journal()
                .enqueue_effect(&h.plan, h.target(), &first.id)
                .await
                .unwrap();
            let (sink, sends) = transport(h, Duration::ZERO);
            super::mongo_tests::execute(h, &first.id).await;
            assert_eq!(sends.load(std::sync::atomic::Ordering::SeqCst), 1);
            sink.abort();
            let mut unknown = store
                .admit(&h.plan, h.target(), "uncertain", &payload)
                .await
                .unwrap();
            h.nodes[0]
                .journal()
                .enqueue_effect(&h.plan, h.target(), &unknown.id)
                .await
                .unwrap();
            let mut held = None;
            for (index, node) in h.nodes.iter().enumerate() {
                if let Ok(token) = node.claim_effect(&h.plan, h.target(), &unknown.id).await {
                    held = Some((index, token));
                    break;
                }
            }
            let (owner, token) = held.unwrap();
            Store(h.nodes[owner].journal())
                .transition(
                    &mut unknown,
                    &token,
                    model::State::Unknown { receipt: None },
                )
                .await
                .unwrap();
            h.nodes[owner].partitions.release(&token).await.unwrap();
            let op = operator(h);
            let req = request(h).await;
            op.apply(&req, 1).await.unwrap();
            assert_eq!(
                op.inspect().await.unwrap().outstanding_quarantined_effects,
                1
            );
            let original = store
                .read(&first.id, &h.plan, h.target())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                store.receipt(&original).await.unwrap().unwrap().delivery,
                crate::delivery::intents::DeliveryOutcome::Confirmed { .. }
            ));
        })
    })
    .await;
}
