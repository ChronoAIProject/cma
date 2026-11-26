//! The encrypted HTTP/CAS fixture controls acknowledgement loss independently
//! from the real-Mongo partition-authority tests. No external sends occur here.
use super::*;
use crate::turns::routed::test_support::{supported, Setup};
fn receipt() -> IntentReceipt {
    crate::delivery::durable::receipt_for(DeliveryOutcome::Confirmed {
        platform_message_id: Some("legacy-original".into()),
    })
}
#[tokio::test]
async fn legacy_archive_and_guard_acknowledgement_failures_never_erase_the_original() {
    for archive in [true, false] {
        for committed in [false, true] {
            let setup = Setup::new(supported()).await;
            let store = ReplyStore(setup.journal().clone(), None);
            let a = "a".repeat(64);
            let b = "b".repeat(64);
            let hash = "c".repeat(64);
            let Admission::Claimed(claim) = store
                .admit(&setup.plan, setup.target(), Kind::Tool, a.clone())
                .await
                .unwrap()
            else {
                panic!("initial legacy claim");
            };
            store.finish(&claim, &receipt()).await.unwrap();
            let guard = storage::identifier(&("ordinary-route-reply-guard-v1", &setup.target().id));
            let saved = storage::identifier(&(
                "ordinary_route_legacy_tool_confirmation_v1",
                &setup.target().id,
                &a,
            ));
            setup
                .server
                .fault(if archive { saved } else { guard }, committed, committed);
            // Select the replica preservation branch on the controllable HTTP
            // persistence adapter; this test does not simulate partition authority.
            let result = store
                .admit_keyed_inner(
                    &setup.plan,
                    setup.target(),
                    (b.clone(), hash.clone(), None),
                    Kind::Tool,
                    true,
                    false,
                )
                .await;
            assert!(result.is_err() || (archive && committed));
            let fresh = ReplyStore(setup.journal().clone(), None);
            let Some(Admission::Replay(replayed)) = fresh
                .replay(&setup.plan, setup.target(), &a, Some(&hash))
                .await
                .unwrap()
            else {
                panic!("interrupted preservation retains exact A");
            };
            assert_eq!(
                storage::bytes(&replayed).unwrap(),
                storage::bytes(&receipt()).unwrap()
            );
            if !committed {
                assert!(matches!(
                    fresh
                        .admit_keyed_inner(
                            &setup.plan,
                            setup.target(),
                            (b.clone(), hash.clone(), None),
                            Kind::Tool,
                            true,
                            false,
                        )
                        .await
                        .unwrap(),
                    Admission::Claimed(_)
                ));
            } else {
                assert!(matches!(
                    fresh
                        .replay(&setup.plan, setup.target(), &b, Some(&hash))
                        .await
                        .unwrap(),
                    Some(Admission::Pending)
                ));
            }
            setup
                .server
                .memory
                .clock
                .fetch_add(31, std::sync::atomic::Ordering::SeqCst);
            assert!(matches!(
                fresh
                    .replay(&setup.plan, setup.target(), &b, Some(&hash))
                    .await
                    .unwrap(),
                Some(Admission::Unknown)
            ));
            assert!(matches!(
                fresh
                    .replay(&setup.plan, setup.target(), &a, Some(&hash))
                    .await
                    .unwrap(),
                Some(Admission::Replay(_))
            ));
        }
    }
}
