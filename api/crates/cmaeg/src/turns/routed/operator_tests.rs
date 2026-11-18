use super::{
    delivery_tests::accepted,
    operator::inspect,
    test_support::*,
    tool_reply_store::{Admission, Kind, ReplyStore},
    *,
};
use cmaeg_core::activity::SourceKind;
use std::sync::{atomic::Ordering, Arc};

#[tokio::test]
async fn expiry_release_uses_exact_horizon_and_preserves_uncertainty_labels() {
    for phase in [
        Phase::Starting,
        Phase::Unknown,
        Phase::Accepted {
            response_id: "resp_1".into(),
            durable_cursor: None,
        },
    ] {
        let setup = Setup::new(supported()).await;
        let mut lease = setup.prepared().await;
        setup
            .journal()
            .transition(&mut lease, phase.clone())
            .await
            .unwrap();
        let horizon = lease.state.event_expires_at
            + cmaeg_core::seal::REPLY_TARGET_TTL
            + crate::replicas::config::LEASE.as_secs() as i64;
        for now in [horizon - 1, horizon, horizon + 1] {
            setup.server.memory.clock.store(now, Ordering::SeqCst);
            let evidence = inspect(setup.journal(), &setup.plan, setup.target())
                .await
                .unwrap();
            let expected = match phase {
                Phase::Unknown => "dispatch_unknown",
                Phase::Starting if now < horizon => "starting",
                Phase::Starting => "starting_expired",
                _ if now < horizon => "accepted",
                _ => "accepted_expired",
            };
            assert_eq!(evidence.state, expected);
            assert_eq!(evidence.eligible, phase == Phase::Unknown || now >= horizon);
        }
    }
}

#[tokio::test]
async fn accepted_automation_remains_ineligible_after_horizon() {
    for source in [
        SourceKind::Custom,
        SourceKind::Github {
            app_ref: "integration".into(),
            installation_id: 99,
            repository_id: 1001,
        },
    ] {
        let mut activity = fixture::custom(None);
        activity.source = source.clone();
        let setup = Setup::with_request(
            supported(),
            fixture::request(
                |v| {
                    fixture::targets(v);
                    if matches!(source, SourceKind::Github { .. }) {
                        v["sources"]
                            .as_array_mut()
                            .unwrap()
                            .push(serde_json::json!({
                                "type":"github","app_ref":"integration","installation_ids":[99],
                                "repository_ids":[1001],"events":["issues"],"actions":["opened"]
                            }));
                    }
                },
                activity,
            ),
        )
        .await;
        let lease = accepted(&setup).await;
        setup.server.memory.clock.store(
            lease.state.event_expires_at + cmaeg_core::seal::REPLY_TARGET_TTL + 60,
            Ordering::SeqCst,
        );
        let evidence = inspect(setup.journal(), &setup.plan, setup.target())
            .await
            .unwrap();
        assert_eq!(evidence.state, "accepted");
        assert!(!evidence.eligible);
        assert_eq!(evidence.eligible_after, None);
        let body = setup.journal().retained_body(&lease.state).await.unwrap();
        assert!(body
            .settlement_event(
                &setup.plan,
                setup.target(),
                &fixture::keys(),
                (setup.journal().clock)()
            )
            .is_ok());
    }
}

#[tokio::test]
async fn proven_unstarted_pending_and_prepared_have_no_release_horizon() {
    for prepared in [false, true] {
        let setup = Setup::new(supported()).await;
        if prepared {
            setup.prepared().await;
        } else {
            setup
                .journal()
                .claim(&setup.plan, setup.target(), "fixture")
                .await
                .unwrap();
        }
        setup.server.memory.clock.store(
            setup.target().launch.event.expires_at + cmaeg_core::seal::REPLY_TARGET_TTL + 60,
            Ordering::SeqCst,
        );
        let evidence = inspect(setup.journal(), &setup.plan, setup.target())
            .await
            .unwrap();
        assert_eq!(evidence.state, "not_uncertain");
        assert!(!evidence.eligible);
        assert_eq!(evidence.eligible_after, None);
    }
}

#[tokio::test]
async fn expired_accepted_keeps_delivery_and_guard_uncertainty_precedence() {
    for guard in [false, true] {
        let setup = Setup::new(supported()).await;
        let mut lease = accepted(&setup).await;
        if guard {
            assert!(matches!(
                ReplyStore(setup.journal().clone(), None)
                    .admit(&setup.plan, setup.target(), Kind::Automatic, "b".repeat(64))
                    .await
                    .unwrap(),
                Admission::Claimed(_)
            ));
        } else {
            let mut state = lease.state.clone();
            state.delivery = Some(DeliveryCheckpoint {
                trace_contract: false,
                response_id: "resp_1".into(),
                cursor: 0,
                frame_hash: "a".repeat(64),
                effect_hash: "b".repeat(64),
                terminal: false,
                status: DeliveryStatus::Unknown,
                custom: None,
            });
            setup.journal().commit(&mut lease, state).await.unwrap();
        }
        let mut journal = setup.journal().clone();
        let now = lease.state.event_expires_at + cmaeg_core::seal::REPLY_TARGET_TTL + 60;
        journal.clock = Arc::new(move || now);
        let evidence = inspect(&journal, &setup.plan, setup.target())
            .await
            .unwrap();
        assert_eq!(
            evidence.state,
            if guard {
                "accepted_guard_unknown"
            } else {
                "accepted_delivery_unknown"
            }
        );
        assert!(evidence.eligible);
    }
}
