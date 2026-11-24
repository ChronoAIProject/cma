use super::*;
use crate::agent_ownership::RetirementIntent;
use layer3_adapter::owner_protocol::TransportAuthority;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transition {
    Active,
    Retire,
    CancelReceiver,
}

#[tokio::test(start_paused = true)]
async fn held_renewal_preserves_reply_validation_across_retirement_and_receiver_cancellation() {
    let mut violations = Vec::new();
    let mut cases = 0;
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        for transition in [
            Transition::Active,
            Transition::Retire,
            Transition::CancelReceiver,
        ] {
            for reply in ["valid", "stale", "agent", "instance", "epoch", "protocol"] {
                let fixture = Fixture::with_policy(2, policy).await;
                let old = &fixture.owners[0];
                *fixture.store.reply.lock().unwrap() = match reply {
                    "valid" => Reply::Renew,
                    "stale" => Reply::Error(OwnershipError::Stale),
                    field => Reply::DifferentToken(field),
                };
                fixture.store.gate.store(true, Ordering::SeqCst);
                let running = tokio::spawn({
                    let manager = fixture.manager.clone();
                    async move { manager.maintain().await }
                });
                fixture.store.entered.notified().await;
                assert!(old.is_current());
                match transition {
                    Transition::Active => (),
                    Transition::Retire => {
                        old.begin_retirement(RetirementIntent::Cooperative).unwrap()
                    }
                    Transition::CancelReceiver => TransportAuthority::revoke(old.as_ref()),
                }
                fixture.store.resume.notify_one();
                let result = running.await.unwrap().map(|hints| hints.len());
                let expected_ok =
                    reply == "valid" || (reply == "stale" && transition != Transition::Active);
                let ready = fixture.manager.is_ready();
                let failed = *fixture.manager.failure_signal().borrow();
                let matches =
                    result.is_ok() == expected_ok && ready == expected_ok && failed != expected_ok;
                let case = format!("policy={policy:?} transition={transition:?} reply={reply} result={result:?} ready={ready} failed={failed}");
                println!("{case}");
                if !matches {
                    violations.push(case);
                }
                if expected_ok {
                    assert!(fixture.owners[1].is_current());
                    assert_eq!(fixture.owners[1].run(async { 42 }).await.unwrap(), 42);
                }
                if transition != Transition::Active || result.is_err() {
                    assert!(!old.is_current());
                    assert!(old
                        .run(async { panic!("obsolete authority executed") })
                        .await
                        .is_err());
                }
                let retained = fixture.manager.owned().await;
                assert_eq!(retained.len(), 2);
                assert!(retained.iter().any(|authority| Arc::ptr_eq(authority, old)));
                assert_eq!(fixture.store.releases.load(Ordering::SeqCst), 0);
                assert_eq!(
                    fixture
                        .store
                        .owner(&old.token.agent_id)
                        .await
                        .unwrap()
                        .unwrap()
                        .token,
                    old.token
                );
                assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 1);
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 36);
    assert!(
        violations.is_empty(),
        "{} renewal validation violations:\n{}",
        violations.len(),
        violations.join("\n")
    );
}
