use super::{
    super::replica_test_fixture::*,
    mongo_progress_fixture::*,
    mongo_rollback_tests::{operator, request},
    *,
};
use crate::{
    replicas::unresolved::Request,
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
            let mut req = Request {
                binding: h.nodes[owner]
                    .work_binding(lease.partition.as_ref().unwrap())
                    .await,
                request_id: uuid::Uuid::new_v4().to_string(),
                reason: "live lease fixture".into(),
                plan_digest: None,
            };
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            assert!(op.preview(&req).await.unwrap().candidate.lease_live);
            assert_eq!(op.apply(&req, 1).await.err().unwrap().code(), "live_lease");
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
                assert_eq!(
                    op.apply(&changed, 1).await.err().unwrap().code(),
                    if kind == 3 {
                        "plan_digest_mismatch"
                    } else {
                        "binding_version_mismatch"
                    }
                );
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
            let refusal = op.apply(&req, 1).await.err().unwrap();
            assert_eq!(refusal.code(), "cohort_incompatible");
            assert!(refusal.document(true)["observed"]["members"]
                .as_array()
                .unwrap()
                .iter()
                .any(|member| member["quarantine"] == false));
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
            assert_eq!(
                op.apply(&changed, 1).await.err().unwrap().code(),
                "request_id_reuse"
            );
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
            assert_eq!(op.apply(&req, 1).await.err().unwrap().code(), "live_lease");
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

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn renewing_partition_does_not_hide_an_eligible_neighbor() {
    with_partitioned_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            h.nodes[owner]
                .journal()
                .transition(&mut lease, Phase::Unknown)
                .await
                .unwrap();
            h.nodes[owner].journal().release(&mut lease).await.unwrap();
            let other = plan_for_conversation(h, "busy-neighbor", "43").await;
            let mut held = None;
            for node in &h.nodes {
                if let Ok(lease) = node
                    .journal()
                    .claim(&other, &other.targets[0], "busy-neighbor")
                    .await
                {
                    held = Some((node.journal(), lease));
                    break;
                }
            }
            let (journal, mut lease) = held.expect("claim independent partition");
            let (stop, stopped) = tokio::sync::watch::channel(false);
            let renewer = Task::new(tokio::spawn(async move {
                while !*stopped.borrow() {
                    journal.renew(&mut lease).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                journal.release(&mut lease).await.unwrap();
            }));
            let op = operator(h);
            let limited = op.inspect_with_limit(1).await.unwrap();
            assert!(limited.truncated);
            assert!(limited.candidates.len() + limited.skipped <= 1);
            for _ in 0..10 {
                let list = op.inspect().await.unwrap();
                assert_eq!(list.candidates.len(), 1);
                assert_eq!(list.candidates[0].target, h.target().id);
                assert!(list.skipped >= 1);
                assert!(!list.truncated);
                op.observe_heads().await.unwrap();
            }
            stop.send(true).unwrap();
            renewer.join().await;
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn operator_refusals_expose_ineligible_live_guard_and_quarantine_states() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            journal.release(&mut lease).await.unwrap();
            let op = operator(h);
            let mut req = Request {
                binding: h.nodes[owner].head_binding(&h.plan, h.target()).await,
                request_id: uuid::Uuid::new_v4().to_string(),
                reason: "refusal fixture".into(),
                plan_digest: None,
            };
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            let refusal = op.apply(&req, 1).await.err().unwrap();
            assert_eq!(refusal.code(), "ineligible_state");
            assert_eq!(refusal.document(true)["observed"]["state"], "accepted");
            let (owner, mut lease) = h.acquire().await;
            let journal = h.nodes[owner].journal();
            assert!(matches!(
                ReplyStore(journal.clone(), lease.partition.clone())
                    .admit(&h.plan, h.target(), Kind::Tool, "c".repeat(64))
                    .await
                    .unwrap(),
                Admission::Claimed(_)
            ));
            journal.release(&mut lease).await.unwrap();
            req.binding = h.nodes[owner].head_binding(&h.plan, h.target()).await;
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            let refusal = op.apply(&req, 1).await.err().unwrap();
            assert_eq!(refusal.code(), "live_guard_attempt");
            assert_eq!(refusal.document(true)["observed"]["guard_live"], true);
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            h.nodes[owner]
                .journal()
                .transition(&mut lease, Phase::Unknown)
                .await
                .unwrap();
            h.nodes[owner].journal().release(&mut lease).await.unwrap();
            let mut capability = h.capability();
            capability.issued_at -= 5;
            let root = Store(h.nodes[owner].journal())
                .admit(
                    &h.plan,
                    h.target(),
                    "quarantined",
                    &Payload {
                        capability,
                        caller: caller().key_hash,
                        message: message(),
                    },
                )
                .await
                .unwrap();
            h.nodes[owner]
                .journal()
                .enqueue_effect(&h.plan, h.target(), &root.id)
                .await
                .unwrap();
            let op = operator(h);
            let first = request(h).await;
            op.apply(&first, 1).await.unwrap();
            let mut req = Request {
                binding: h.nodes[owner].head_binding(&h.plan, h.target()).await,
                request_id: uuid::Uuid::new_v4().to_string(),
                reason: "second operation".into(),
                plan_digest: None,
            };
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            let refusal = op.apply(&req, 1).await.err().unwrap();
            assert_eq!(refusal.code(), "already_quarantined");
            assert_eq!(refusal.document(true)["observed"]["target"], h.target().id);
        })
    })
    .await;
}
