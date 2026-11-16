//! Real partition transactions protect supplementary replies and retained parts.
use super::*;
use crate::{
    delivery::intents::{DeliveryOutcome, IntentReceipt},
    replicas::effects::{EffectDisposition, EffectExecutor},
    turns::routed::{
        adapter_test_server::HttpServer,
        custom_http_media_tests::inline_media,
        delivery_tests::command,
        media_delivery::MediaDelivery,
        media_response_store::NativeOutcome,
        replica_test_fixture::*,
        scheduler::{Disposition, TargetExecutor, WorkItem},
        MediaContinuationReceipt, MediaLease, MediaObligationStore, MediaUploadReceipt,
        MediaWorkIndex,
    },
};
use cmaeg_core::media_continuation::{sha256, MediaArtifactClaim, MediaContinuationKind};
use std::{collections::BTreeMap, time::Duration};

struct Fixture {
    h: Harness,
    workers: Vec<Arc<MediaWorker>>,
    http: HttpServer,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_harness(Harness::media().await).await
    }
    async fn with_harness(h: Harness) -> Self {
        let (owner, mut parent) = h.accepted().await;
        if matches!(
            h.plan.activity.source,
            cmaeg_core::activity::SourceKind::Custom
        ) {
            // Seed a preexisting completed parent; only supplementary IO is under test.
            let journal = h.nodes[owner].journal();
            journal
                .transition(
                    &mut parent,
                    super::super::Phase::Completed {
                        response_id: "fixture-response".into(),
                        durable_cursor: None,
                    },
                )
                .await
                .unwrap();
            h.nodes[owner]
                .partitions
                .complete(parent.partition.as_ref().unwrap())
                .await
                .unwrap();
        } else {
            h.finish_parent(owner, &mut parent).await;
        }
        let http = HttpServer::start().await;
        http.memory.lock().unwrap().config.snapshot_body = Some(serde_json::json!({
            "id":"resp_media","object":"response","status":"completed",
            "conversation":{"id":"agt_existing"},"metadata":metadata(),
            "output":[{"type":"message","content":[{"type":"output_text","text":"Media completed."}]}]
        }));
        let workers = h
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let store = MediaObligationStore(node.journal());
                let providers = http.client(true, Duration::from_secs(2));
                Arc::new(MediaWorker {
                    index: MediaWorkIndex::new(node.journal(), node.plans()),
                    plans: node.plans(),
                    store: store.clone(),
                    authority: h.current.clone(),
                    providers: providers.clone(),
                    media: inline_media(),
                    delivery: Arc::new(MediaDelivery {
                        store,
                        authority: h.current.clone(),
                        providers,
                        coordinator: h.tools[index].coordinator.clone(),
                    }),
                    worker: format!("media-node-{index}"),
                })
            })
            .collect();
        Self { h, workers, http }
    }
    async fn admit(&self) {
        let work = WorkItem {
            identity: self.h.plan.identity.clone(),
            target_id: self.h.target().id.clone(),
        };
        for worker in &self.workers {
            assert!(matches!(
                TargetExecutor::execute(worker.as_ref(), &work).await,
                Disposition::Settled
            ));
        }
        assert!(
            self.http.seen().is_empty(),
            "media scanner only admits FIFO work"
        );
    }
    async fn claim(&self) -> (usize, PartitionLease) {
        let until = tokio::time::Instant::now() + Duration::from_secs(25);
        loop {
            for (index, node) in self.h.nodes.iter().enumerate() {
                if node.worker.is_none() {
                    continue;
                }
                match node
                    .claim_effect(
                        &self.h.plan,
                        self.h.target(),
                        &media_work::id(self.h.target()),
                    )
                    .await
                {
                    Ok(token) => return (index, token),
                    Err(crate::replicas::Error::Busy | crate::replicas::Error::Lost) => {}
                    Err(error) => panic!("effect claim failed: {error:?}"),
                }
            }
            assert!(
                tokio::time::Instant::now() < until,
                "media effect ownership unavailable"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn prime(&self, owner: usize, token: &PartitionLease) -> MediaLease {
        let store = &self.workers[owner].store;
        store
            .initialize(&self.h.plan, self.h.target(), 0, None, None)
            .await
            .unwrap();
        let mut lease = store
            .claim_partition(
                &self.h.plan,
                self.h.target(),
                0,
                "media-prime",
                Some(token.clone()),
            )
            .await
            .unwrap();
        let artifact = MediaArtifactClaim {
            attachment_index: 0,
            kind: MediaContinuationKind::Original,
            content_sha256: sha256(b"file"),
            source_sha256: sha256(b"file"),
            name: "file.txt".into(),
            media_type: "text/plain".into(),
            size_bytes: 4,
            text_sha256: None,
        };
        store
            .freeze_original(&mut lease, artifact.clone())
            .await
            .unwrap();
        store
            .record_upload(
                &mut lease,
                MediaUploadReceipt {
                    id: "att_media".into(),
                    agent_id: "agt_existing".into(),
                    name: artifact.name.clone(),
                    path: "/home/attachments/agt_existing/att_media/file.txt".into(),
                    size_bytes: 4,
                    media_type: artifact.media_type.clone(),
                    sha256: artifact.source_sha256.clone(),
                },
            )
            .await
            .unwrap();
        store
            .freeze_supplement(&mut lease, artifact, "")
            .await
            .unwrap();
        store
            .record_submission(
                &mut lease,
                MediaContinuationKind::Original,
                MediaContinuationReceipt {
                    command_id: "cmd_media".into(),
                    response_id: "resp_media".into(),
                    metadata: metadata(),
                },
            )
            .await
            .unwrap();
        let mut renewed = token.clone();
        self.h.nodes[owner]
            .journal()
            .renew_partition(&mut renewed)
            .await
            .unwrap();
        assert!(matches!(
            store
                .claim_partition(
                    &self.h.plan,
                    self.h.target(),
                    0,
                    "same-generation",
                    Some(renewed)
                )
                .await,
            Err(ExecutionError::Busy)
        ));
        store.release(&mut lease).await.unwrap();
        lease
    }
}
fn metadata() -> BTreeMap<String, String> {
    [
        ("cma_parent_command_id", "cmd_parent"),
        ("cma_parent_response_id", "fixture-response"),
        ("cma_parent_fingerprint", "parent_fingerprint"),
        ("cma_artifact_id", "0:original"),
        ("cma_command_fingerprint", "media_fingerprint"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}
fn partial(h: &Harness) -> IntentReceipt {
    let mut receipt = crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
        reason: "fixture_partial_media".into(),
    });
    for index in 0..80 {
        let payload: cmaeg_core::seal::MessageRefPayload = serde_json::from_value(serde_json::json!({
            "channel_id":h.event().channel_id,"event_id":h.event().event_id,"consumer":h.event().consumer,
            "platform_message_id":(800+index).to_string(),"route_id":"telegram:77:42"
        })).unwrap();
        receipt
            .message_parts
            .push(crate::delivery::intents::ConfirmedMessagePart {
                index,
                message_ref: cmaeg_core::ids::MessageRef::seal(&payload, &h.database.keys())
                    .unwrap(),
            });
    }
    receipt
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn media_scanners_only_admit_fifo_and_confirmed_effect_replays_after_completion() {
    let mut f = Fixture::new().await;
    f.admit().await;
    let (owner, mut token) = f.claim().await;
    f.prime(owner, &token).await;
    let id = media_work::id(f.h.target());
    let mut rx = std::mem::replace(&mut f.h.rx, tokio::sync::mpsc::channel(1).1);
    let (result, ()) = tokio::join!(
        EffectExecutor::execute(
            f.workers[owner].as_ref(),
            &f.h.plan,
            f.h.target(),
            &id,
            &mut token
        ),
        async {
            let reply = command(&mut rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply.receipt.send(receipt()).unwrap();
        }
    );
    assert!(matches!(result.unwrap(), EffectDisposition::Completed));
    f.h.nodes[owner].partitions.complete(&token).await.unwrap();
    assert!(f.workers[1 - owner]
        .completed_media(&f.h.plan, f.h.target())
        .await
        .unwrap());
    assert!(f.workers[1 - owner]
        .index
        .load(&f.h.plan, f.h.target())
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        f.workers[1 - owner]
            .run_owned(&f.h.plan, f.h.target(), &mut None)
            .await,
        Err(ExecutionError::LostLease)
    ));
    let ledger = f.workers[1 - owner]
        .store
        .load(&f.h.plan, f.h.target(), 0)
        .await
        .unwrap()
        .unwrap();
    assert!(ledger.supplements[0].settled);
    let response = f.workers[1 - owner]
        .store
        .retained_media_response(&ledger, MediaContinuationKind::Original)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        response.native_receipt.unwrap().delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(rx.try_recv().is_err());
    assert!(f.http.seen().iter().all(|request| request.method == "GET"));
    drop(f.workers);
    f.h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn media_partial_receipt_survives_takeover_and_blocks_later_fifo_work() {
    let mut f = Fixture::new().await;
    f.admit().await;
    let (owner, mut token) = f.claim().await;
    let stale = f.prime(owner, &token).await;
    let id = media_work::id(f.h.target());
    let expected = partial(&f.h);
    let mut rx = std::mem::replace(&mut f.h.rx, tokio::sync::mpsc::channel(1).1);
    let (result, ()) = tokio::join!(
        EffectExecutor::execute(
            f.workers[owner].as_ref(),
            &f.h.plan,
            f.h.target(),
            &id,
            &mut token
        ),
        async {
            let reply = command(&mut rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            reply.receipt.send(expected.clone()).unwrap();
        }
    );
    assert!(matches!(result.unwrap(), EffectDisposition::Retained));
    drop(f.h.nodes[owner].worker.take());
    let (next, mut successor) = f.claim().await;
    assert_ne!(owner, next);
    assert!(matches!(
        EffectExecutor::execute(
            f.workers[next].as_ref(),
            &f.h.plan,
            f.h.target(),
            &id,
            &mut successor
        )
        .await
        .unwrap(),
        EffectDisposition::Retained
    ));
    assert!(f.workers[owner].store.guard(&stale).await.is_err());
    let ledger = f.workers[next]
        .store
        .load(&f.h.plan, f.h.target(), 0)
        .await
        .unwrap()
        .unwrap();
    assert!(!ledger.supplements[0].settled);
    let response = f.workers[next]
        .store
        .retained_media_response(&ledger, MediaContinuationKind::Original)
        .await
        .unwrap()
        .unwrap();
    let restored = f.h.nodes[next]
        .journal()
        .receipts(
            &response.id,
            &f.h.plan.identity.owner,
            f.h.plan.retained_until,
        )
        .load(response.native_receipt.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        crate::turns::routed::storage::bytes(&restored).unwrap(),
        crate::turns::routed::storage::bytes(&expected).unwrap()
    );
    let later = uuid::Uuid::new_v4().to_string();
    f.h.nodes[next]
        .journal()
        .enqueue_effect(&f.h.plan, f.h.target(), &later)
        .await
        .unwrap();
    assert!(f.h.nodes[next]
        .claim_effect(&f.h.plan, f.h.target(), &later)
        .await
        .is_err());
    assert!(rx.try_recv().is_err());
    assert!(!f.workers[next]
        .completed_media(&f.h.plan, f.h.target())
        .await
        .unwrap());
    assert!(f.workers[next]
        .index
        .load(&f.h.plan, f.h.target())
        .await
        .unwrap()
        .is_some());
    drop(f.workers);
    f.h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn cancelled_media_send_cannot_write_or_replay_after_takeover() {
    let mut f = Fixture::new().await;
    f.admit().await;
    let (owner, mut token) = f.claim().await;
    f.prime(owner, &token).await;
    let id = media_work::id(f.h.target());
    let mut rx = std::mem::replace(&mut f.h.rx, tokio::sync::mpsc::channel(1).1);
    let reply = {
        let execution = EffectExecutor::execute(
            f.workers[owner].as_ref(),
            &f.h.plan,
            f.h.target(),
            &id,
            &mut token,
        );
        tokio::pin!(execution);
        let reply = tokio::select! {
            _ = &mut execution => panic!("awaiting native receipt"),
            reply = command(&mut rx) => reply,
        };
        let _permit = reply.gate.attempt().await.unwrap();
        assert!(reply.gate.commit());
        reply
    };
    assert!(!reply.gate.commit());
    drop(f.h.nodes[owner].worker.take());
    let (next, mut successor) = f.claim().await;
    assert!(matches!(
        EffectExecutor::execute(
            f.workers[next].as_ref(),
            &f.h.plan,
            f.h.target(),
            &id,
            &mut successor
        )
        .await
        .unwrap(),
        EffectDisposition::Retained
    ));
    assert!(reply.receipt.send(receipt()).is_err());
    let ledger = f.workers[next]
        .store
        .load(&f.h.plan, f.h.target(), 0)
        .await
        .unwrap()
        .unwrap();
    let response = f.workers[next]
        .store
        .retained_media_response(&ledger, MediaContinuationKind::Original)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(response.native, NativeOutcome::Intent));
    assert!(response.native_receipt.is_none());
    assert!(rx.try_recv().is_err());
    drop(f.workers);
    f.h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn media_send_renewal_failure_disables_late_gate_and_retains_uncertainty() {
    use futures_util::FutureExt;
    let mut f = Fixture::new().await;
    let result = std::panic::AssertUnwindSafe(async {
        f.admit().await;
        let (owner, mut token) = f.claim().await;
        f.prime(owner, &token).await;
        let mut rx = std::mem::replace(&mut f.h.rx, tokio::sync::mpsc::channel(1).1);
        let work_id = media_work::id(f.h.target());
        let (result, reply) = tokio::join!(
            EffectExecutor::execute(
                f.workers[owner].as_ref(),
                &f.h.plan,
                f.h.target(),
                &work_id,
                &mut token
            ),
            async {
                let reply = command(&mut rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                f.h.nodes[owner]
                    .lose_work(&media_work::id(f.h.target()))
                    .await
                    .unwrap();
                reply
            }
        );
        assert!(matches!(result, Err(ExecutionError::LostLease)));
        assert!(!reply.gate.commit());
        assert!(reply.receipt.send(receipt()).is_err());
        assert!(rx.try_recv().is_err());
    })
    .catch_unwind()
    .await;
    drop(f.workers);
    drop(f.http);
    f.h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn media_webhook_renewal_failure_never_retries_or_publishes_a_receipt() {
    use crate::delivery::{
        custom::CustomDelivery, sync::SyncRegistry, webhook_client::WebhookClient,
    };
    use crate::turns::routed::{
        test_support::fixture, tool_effects::mongo_webhook_renewal_tests::HangingWebhook,
    };
    use futures_util::FutureExt;
    let mut activity = fixture::custom(None);
    let cmaeg_core::activity::ActivityKind::Event { attachments, .. } = &mut activity.kind else {
        unreachable!()
    };
    attachments.push(serde_json::from_value(serde_json::json!({"reference":"file","kind":"file","filename":"file.txt","source":{"type":"inline","pointer":"/file"}})).unwrap());
    let request = fixture::request(
        |v| {
            v["routing"]["default_targets"] = serde_json::json!(["default"]);
            v["provider"]["target"] =
                serde_json::json!({"type":"existing_agent","agent_id":"agt_existing"});
            v["sources"] =
                serde_json::json!([{"type":"custom","auth":{"type":"hmac","key_id":"result"}}]);
            v["partition"] = serde_json::json!({"version":1,"key":"per_event"});
            v["reply"]["sinks"] = serde_json::json!([{"type":"result_webhook","url":"https://receiver.example/hook","hmac_key_id":"result"}]);
            v["secrets"]["hmac_secrets"] =
                serde_json::json!([["result", "fixture-webhook-signing"]]);
        },
        activity,
    );
    let mut f = Fixture::with_harness(Harness::with_request(request, true).await).await;
    let result = std::panic::AssertUnwindSafe(async {
        f.admit().await;
        let (owner, mut token) = f.claim().await;
        f.prime(owner, &token).await;
        let receiver = Arc::new(HangingWebhook::default());
        let mut custom = CustomDelivery::new(
            "https://gateway.example".into(),
            Arc::new(SyncRegistry::default()),
        );
        custom.webhooks = WebhookClient::with_transport(receiver.clone());
        let coordinator = f.h.tools[owner]
            .coordinator
            .clone()
            .with_custom_stream(custom);
        let worker = Arc::get_mut(&mut f.workers[owner]).unwrap();
        worker.delivery = Arc::new(MediaDelivery {
            store: worker.store.clone(),
            authority: f.h.current.clone(),
            providers: worker.providers.clone(),
            coordinator,
        });
        let work = media_work::id(f.h.target());
        let (result, ()) = tokio::join!(
            EffectExecutor::execute(
                f.workers[owner].as_ref(),
                &f.h.plan,
                f.h.target(),
                &work,
                &mut token
            ),
            async {
                receiver.started.notified().await;
                f.h.nodes[owner].lose_work(&work).await.unwrap();
            }
        );
        assert!(matches!(result, Err(ExecutionError::LostLease)));
        assert_eq!(receiver.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(f.h.rx.try_recv().is_err());
    })
    .catch_unwind()
    .await;
    drop(f.workers);
    drop(f.http);
    f.h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn operator_media_abandonment_preserves_partial_evidence_and_counts_unexpired_custody() {
    use futures_util::FutureExt;
    let mut f = Fixture::new().await;
    let result = std::panic::AssertUnwindSafe(async {
        f.admit().await;
        let (owner, mut token) = f.claim().await;
        f.prime(owner, &token).await;
        let work = media_work::id(f.h.target());
        let saved_receipt = partial(&f.h);
        let mut rx = std::mem::replace(&mut f.h.rx, tokio::sync::mpsc::channel(1).1);
        let (outcome, ()) = tokio::join!(
            EffectExecutor::execute(
                f.workers[owner].as_ref(),
                &f.h.plan,
                f.h.target(),
                &work,
                &mut token
            ),
            async {
                let reply = command(&mut rx).await;
                let _permit = reply.gate.attempt().await.unwrap();
                assert!(reply.gate.commit());
                reply.receipt.send(saved_receipt).unwrap();
            }
        );
        assert!(matches!(outcome.unwrap(), EffectDisposition::Retained));
        f.h.nodes[owner].partitions.release(&token).await.unwrap();
        let op = crate::replicas::unresolved::Operator::new(
            f.h.nodes[owner].partitions.clone(),
            f.h.nodes[owner].plans(),
            f.h.nodes[owner].journal(),
        );
        let listed = op.inspect().await.unwrap();
        assert_eq!(listed.candidates[0].state, "media_effect_unknown");
        let mut request = crate::replicas::unresolved::Request {
            binding: listed.candidates[0].binding.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            reason: "reviewed uncertain media fixture".into(),
            plan_digest: None,
        };
        request.plan_digest = Some(op.preview(&request).await.unwrap().plan_digest);
        let provider_calls = f.http.seen().len();
        op.apply(&request, 1).await.unwrap();
        assert_eq!(
            f.http.seen().len(),
            provider_calls,
            "operator must never replay provider output"
        );
        assert!(rx.try_recv().is_err());
        let listed = op.inspect().await.unwrap();
        assert!(listed.candidates.is_empty());
        assert_eq!(
            listed.outstanding_quarantined_effects, 1,
            "unexpired uncertain media is not a saved refusal"
        );
    })
    .catch_unwind()
    .await;
    drop(f.workers);
    drop(f.http);
    f.h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
