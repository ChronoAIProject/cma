use super::{
    super::replica_test_fixture::*, mongo_progress_fixture::*, mongo_rollback_tests::operator, *,
};
use crate::{
    ingress::routing_plan::fixture,
    replicas::unresolved::{utc, Request},
    turns::routed::{
        adapter_test_server::HttpServer,
        expiry::EXPIRED_BEFORE_START,
        tool_reply_store::{Admission, Kind, ReplyStore},
        DispatchOutcome, HttpExecutionPort,
    },
};
use std::sync::atomic::{AtomicI64, Ordering};

fn horizon(h: &Harness) -> i64 {
    h.event().expires_at
        + cmaeg_core::seal::REPLY_TARGET_TTL
        + crate::replicas::config::LEASE.as_secs() as i64
}
fn relay_request() -> crate::ingress::launch::LaunchRequest {
    let mut activity = fixture::message("retained relay chat");
    activity.source = cmaeg_core::activity::SourceKind::NyxidRelay {
        platform: cmaeg_core::channel::Platform::Lark,
        route_id: fixture::ROUTE.into(),
    };
    fixture::request(
        |v| {
            v["routing"]["default_targets"] = serde_json::json!(["default"]);
            v["sources"][1] = crate::channels::routing_fixture::relay("lark", fixture::ROUTE);
        },
        activity,
    )
}
fn request(binding: crate::replicas::unresolved::Binding) -> Request {
    Request {
        binding,
        request_id: uuid::Uuid::new_v4().to_string(),
        reason: "review retained expired execution".into(),
        plan_digest: None,
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn accepted_relay_expiry_lists_stalled_then_releases_without_sends() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recorder = metrics::set_default_local_recorder(&recorder);
    with_request_harness(relay_request(), |h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            journal.release(&mut lease).await.unwrap();
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            let later = plan_with_expiry(h, "later-expiring", "42", now + 3).await;
            let clock = Arc::new(AtomicI64::new(horizon(h) - 1));
            let mut op = operator(h);
            let time = clock.clone();
            op.journal.clock = Arc::new(move || time.load(Ordering::SeqCst));
            let listed = op.inspect().await.unwrap();
            assert!(listed.candidates.is_empty());
            assert!(listed.unreadable.is_empty());
            assert_eq!(listed.stalled.len(), 1);
            let row = &listed.stalled[0];
            assert_eq!(row.state, "accepted");
            assert_eq!(row.binding.head_work, h.target().id);
            assert_eq!(row.event_expires_at, utc(h.event().expires_at).unwrap());
            assert_eq!(row.eligible_after, Some(utc(horizon(h)).unwrap()));
            assert_eq!(row.retained_until, utc(h.plan.retained_until).unwrap());
            assert!(
                row.age_seconds >= 0 && row.age_seconds <= (journal.clock)() - h.plan.admitted_at
            );
            op.observe_heads().await.unwrap();
            assert!(handle.render().contains("cmaeg_replica_stalled_heads 1"));
            assert!(handle
                .render()
                .contains("cmaeg_replica_retained_uncertain_heads 0"));
            let mut req = request(row.binding.clone());
            let preview = op.preview(&req).await.unwrap();
            assert_eq!(preview.candidate.state, "accepted");
            assert!(!preview.candidate.eligible);
            req.plan_digest = Some(preview.plan_digest);
            assert_eq!(
                op.apply(&req, 1).await.err().unwrap().code(),
                "ineligible_state"
            );
            let body = journal.retained_body(&lease.state).await.unwrap();
            assert!(matches!(
                body.settlement_event(&h.plan, h.target(), &h.database.keys(), horizon(h) - 1),
                Err(ExecutionError::Expired)
            ));
            let before = journal
                .read::<TargetState>(&h.target().id)
                .await
                .unwrap()
                .unwrap();

            clock.store(horizon(h), Ordering::SeqCst);
            let listed = op.inspect().await.unwrap();
            assert!(listed.stalled.is_empty());
            assert_eq!(listed.candidates[0].state, "accepted_expired");
            assert!(listed.candidates[0].eligible);
            op.observe_heads().await.unwrap();
            assert!(handle.render().contains("cmaeg_replica_stalled_heads 0"));
            assert!(handle
                .render()
                .contains("cmaeg_replica_retained_uncertain_heads 1"));
            let preview = op.preview(&req).await.unwrap();
            assert_eq!(preview.plan_digest.len(), 64);
            req.plan_digest = Some("stale".into());
            assert_eq!(
                op.apply(&req, 1).await.err().unwrap().code(),
                "plan_digest_mismatch"
            );
            req.plan_digest = Some(preview.plan_digest);
            let receipt = op.apply(&req, 1).await.unwrap();
            assert_eq!(receipt.kind, "replica_unknown_abandonment_v1");
            assert_eq!(receipt.disposition, "unknown_abandoned");
            assert_eq!(receipt.prior_state, "accepted_expired");
            assert_eq!(
                op.audit(&req.request_id).await.unwrap().prior_state,
                "accepted_expired"
            );
            assert_eq!(
                op.apply(&req, 1).await.unwrap().evidence_sha256,
                receipt.evidence_sha256
            );
            let after = journal
                .read::<TargetState>(&h.target().id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(before.1, after.1);
            assert_eq!(
                super::super::storage::bytes(&before.0).unwrap(),
                super::super::storage::bytes(&after.0).unwrap()
            );
            assert!(journal.quarantined(&h.plan, h.target()).await.unwrap());
            let listed = op.inspect().await.unwrap();
            assert_eq!(listed.stalled[0].binding.head_work, later.targets[0].id);
            assert!(listed.stalled[0].eligible_after.is_none());
            assert_eq!(
                h.tools[1].authorize_reply_target_mint(&h.event()).await,
                Err(ExecutionError::Abandoned)
            );
            let mut capability = h.capability();
            capability.issued_at -= 5;
            // Delayed relay capabilities omit the original single-use reply token.
            if let cmaeg_core::seal::ReplyAddress::NyxidRelay {
                reply_token,
                reply_token_expires_at,
                ..
            } = &mut capability.event.reply
            {
                *reply_token = cmaeg_core::channel::Secret::new("");
                *reply_token_expires_at = 1;
            }
            assert!(matches!(
                h.tools[1]
                    .reply_delayed(caller(), &capability, "late-reply", message())
                    .await,
                Err(ExecutionError::Abandoned)
            ));
            let channel = h.current.channel.lock().unwrap().clone();
            assert!(matches!(
                h.tools[1]
                    .reply(caller(), &channel, &h.event(), message(), false)
                    .await,
                Err(ExecutionError::Abandoned)
            ));

            // The later item has its own short deadline; use real lease time for dispatch.
            let wait = (later.targets[0].launch.event.expires_at - (journal.clock)()).max(0);
            tokio::time::sleep(Duration::from_secs(wait as u64 + 1)).await;
            let http = HttpServer::start().await;
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    for node in &h.nodes {
                        let port = Arc::new(HttpExecutionPort::new(
                            http.client(true, Duration::from_secs(5)),
                            Arc::new(Builder(h.database.keys())),
                            node.plans(),
                            node.journal(),
                            h.current.clone(),
                        ));
                        match port
                            .dispatcher()
                            .dispatch(&later.identity, &later.targets[0].id, "later-expired")
                            .await
                        {
                            Ok(DispatchOutcome::Terminal(Phase::Refused { code })) => {
                                assert_eq!(code, EXPIRED_BEFORE_START);
                                return;
                            }
                            Err(ExecutionError::Busy | ExecutionError::LostLease) => {}
                            other => panic!("later item did not expire safely: {:?}", other.err()),
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .unwrap();
            assert!(
                http.seen().is_empty(),
                "expiry must not POST or replay old content"
            );
            assert!(
                h.rx.try_recv().is_err(),
                "inspection, release and expiry must send nothing"
            );
            let listed = op.inspect().await.unwrap();
            assert!(listed.candidates.is_empty() && listed.stalled.is_empty());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn expired_starting_and_prepared_start_evidence_are_releasable() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            let mut op = operator(h);
            let now = horizon(h);
            op.journal.clock = Arc::new(move || now);
            let body = lease.state.body.clone();
            for phase in [Phase::Starting, Phase::Pending, Phase::Prepared] {
                let mut state = lease.state.clone();
                state.phase = phase.clone();
                state.projection = serde_json::from_value(serde_json::json!({
                    "slot":0,"hash":"a".repeat(64),"len":1,"versions":[1]
                }))
                .unwrap();
                state.body = if phase == Phase::Pending {
                    None
                } else {
                    body.clone()
                };
                journal.commit(&mut lease, state).await.unwrap();
                let mut before = journal.clone();
                before.clock = Arc::new(move || now - 1);
                let evidence = super::super::operator::inspect(&before, &h.plan, h.target())
                    .await
                    .unwrap();
                assert!(!evidence.eligible);
                assert_eq!(
                    evidence.state,
                    if phase == Phase::Starting {
                        "starting"
                    } else {
                        "not_uncertain"
                    }
                );
                assert_eq!(evidence.eligible_after, Some(now));
                let evidence = super::super::operator::inspect(&op.journal, &h.plan, h.target())
                    .await
                    .unwrap();
                assert!(evidence.eligible);
                assert_eq!(
                    evidence.state,
                    if phase == Phase::Starting {
                        "starting_expired"
                    } else {
                        "prepared_started_expired"
                    }
                );
            }
            journal.release(&mut lease).await.unwrap();
            let list = op.inspect().await.unwrap();
            let mut req = request(list.candidates[0].binding.clone());
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            assert_eq!(
                op.apply(&req, 1).await.unwrap().prior_state,
                "prepared_started_expired"
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn accepted_custom_stays_stalled_without_an_expiry_release_deadline() {
    let launch = fixture::request(
        |v| v["routing"]["default_targets"] = serde_json::json!(["default"]),
        fixture::custom(None),
    );
    with_request_harness(launch, |h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            h.nodes[owner].journal().release(&mut lease).await.unwrap();
            let mut op = operator(h);
            let now = horizon(h);
            op.journal.clock = Arc::new(move || now);
            let view = serde_json::to_value(op.inspect().await.unwrap()).unwrap();
            assert_eq!(view["stalled"][0]["state"], "accepted");
            assert!(view["stalled"][0]["eligible_after"].is_null());
            assert_eq!(
                view["stalled"][0]["retained_until"],
                utc(h.plan.retained_until).unwrap()
            );
            let list = op.inspect().await.unwrap();
            assert!(list.candidates.is_empty());
            let mut req = request(list.stalled[0].binding.clone());
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            assert_eq!(
                op.apply(&req, 1).await.err().unwrap().code(),
                "ineligible_state"
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn expired_release_still_refuses_live_lease_and_guard() {
    with_harness(|h| {
        Box::pin(async move {
            let (owner, mut lease) = h.accepted().await;
            let journal = h.nodes[owner].journal();
            journal
                .transition(&mut lease, Phase::Starting)
                .await
                .unwrap();
            assert!(matches!(
                ReplyStore(journal.clone(), lease.partition.clone())
                    .admit(&h.plan, h.target(), Kind::Automatic, "c".repeat(64))
                    .await
                    .unwrap(),
                Admission::Claimed(_)
            ));
            let mut op = operator(h);
            let now = horizon(h);
            op.journal.clock = Arc::new(move || now);
            let mut req = request(
                h.nodes[owner]
                    .work_binding(lease.partition.as_ref().unwrap())
                    .await,
            );
            let preview = op.preview(&req).await.unwrap();
            assert_eq!(preview.candidate.state, "starting_expired");
            assert!(
                preview.candidate.eligible
                    && preview.candidate.lease_live
                    && preview.candidate.guard_live
            );
            req.plan_digest = Some(preview.plan_digest);
            assert_eq!(op.apply(&req, 1).await.err().unwrap().code(), "live_lease");
            journal.release(&mut lease).await.unwrap();
            req.binding = h.nodes[owner].head_binding(&h.plan, h.target()).await;
            req.plan_digest = Some(op.preview(&req).await.unwrap().plan_digest);
            assert_eq!(
                op.apply(&req, 1).await.err().unwrap().code(),
                "live_guard_attempt"
            );
            assert!(h.rx.try_recv().is_err());
        })
    })
    .await;
}
