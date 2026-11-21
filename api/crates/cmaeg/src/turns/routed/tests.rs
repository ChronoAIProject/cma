use super::{test_support::*, *};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn concurrent_claims_and_expired_workers_are_fenced() {
    let setup = Setup::new(supported()).await;
    let (first, second) = tokio::join!(
        setup
            .journal()
            .claim(&setup.plan, setup.target(), "same-worker"),
        setup
            .journal()
            .claim(&setup.plan, setup.target(), "same-worker")
    );
    let mut old = match (first, second) {
        (Ok(lease), Err(ExecutionError::LostLease | ExecutionError::Busy))
        | (Err(ExecutionError::LostLease | ExecutionError::Busy), Ok(lease)) => lease,
        _ => panic!("only one worker may acquire ownership"),
    };
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let next = setup
        .journal()
        .claim(&setup.plan, setup.target(), "next")
        .await
        .unwrap();
    assert!(next.state.ownership.epoch > old.state.ownership.epoch);
    assert_eq!(
        setup.journal().renew(&mut old).await,
        Err(ExecutionError::LostLease)
    );
    setup.journal().guard(&next).await.unwrap();
}

#[tokio::test]
async fn lost_start_commit_response_cannot_send_before_confirmation() {
    for lose_read in [false, true] {
        let setup = Setup::new(supported()).await;
        let mut lease = setup.prepared().await;
        setup
            .server
            .fault(setup.target().id.clone(), true, lose_read);
        let result = setup
            .journal()
            .transition(&mut lease, Phase::Starting)
            .await;
        if lose_read {
            assert_eq!(result, Err(ExecutionError::Uncertain));
        } else {
            result.unwrap();
            assert_eq!(lease.state.phase, Phase::Starting);
        }
        assert!(setup.port.starts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn large_frozen_body_restores_exact_bytes_and_credentials_rotate_separately() {
    let setup = Setup::new(supported()).await;
    *setup.port.next.lock().unwrap() = Some(StartResult::Unknown);
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Unknown
    ));
    {
        let mut channel = setup.current.channel.lock().unwrap();
        let definition = &mut std::sync::Arc::get_mut(&mut channel).unwrap().definition;
        let consumer = &setup.target().launch.consumer;
        definition
            .secrets
            .consumer_keys
            .iter_mut()
            .find(|(id, _)| id == consumer)
            .unwrap()
            .1 = cmaeg_core::channel::Secret::new("rotated-fixture");
    }
    let DispatchOutcome::Replay {
        mut lease, body, ..
    } = setup.dispatch().await.unwrap()
    else {
        panic!("accepted")
    };
    assert!(super::storage::bytes(&body).unwrap().len() > 65536);
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1);
    {
        let starts = setup.port.starts.lock().unwrap();
        assert_eq!(starts[0].0, starts[1].0);
        assert_ne!(starts[0].1, starts[1].1);
        assert!(!String::from_utf8_lossy(&starts[1].0).contains("rotated-fixture"));
    }
    assert!(setup
        .server
        .memory
        .rows
        .lock()
        .unwrap()
        .values()
        .all(|row| row.blob.len() <= 65536));
    setup.journal().release(&mut lease).await.unwrap();
}

#[tokio::test]
async fn body_corruption_never_reprepares_or_sends() {
    let setup = Setup::new(supported()).await;
    let mut lease = setup.prepared().await;
    let id = lease.state.body.as_ref().unwrap().chunks[1].clone();
    setup.journal().release(&mut lease).await.unwrap();
    setup.server.change(&id, |value| value["index"] = 0.into());
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::Corrupt)
    ));
    assert!(setup.port.starts.lock().unwrap().is_empty());
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn authority_is_rechecked_after_durable_start() {
    let setup = Setup::new(supported()).await;
    setup.current.revoke_at.store(2, Ordering::SeqCst);
    assert!(matches!(
        setup.dispatch().await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert!(setup.port.starts.lock().unwrap().is_empty());
    let lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "inspect")
        .await
        .unwrap();
    assert_eq!(lease.state.phase, Phase::Starting);
}

#[tokio::test]
async fn owner_source_consumer_and_binding_edits_refuse_start() {
    for edit in 0..5 {
        let setup = Setup::new(supported()).await;
        {
            let mut channel = setup.current.channel.lock().unwrap();
            let definition = &mut std::sync::Arc::get_mut(&mut channel).unwrap().definition;
            match edit {
                0 => definition.owner.subject = "other-owner".into(),
                1 => definition.sources.clear(),
                2 => definition.routing = None,
                3 => definition.secrets.consumer_keys.clear(),
                _ => {
                    definition
                        .routing
                        .as_mut()
                        .unwrap()
                        .consumers
                        .iter_mut()
                        .find(|value| value.id == setup.target().launch.consumer)
                        .unwrap()
                        .provider
                        .binding_id = cmaeg_core::ids::BindingId::new("changed").unwrap()
                }
            }
        }
        assert!(matches!(
            setup.dispatch().await,
            Err(ExecutionError::AuthorityChanged)
        ));
        assert!(setup.port.starts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn expired_event_and_unconfigured_provider_never_start() {
    let setup = Setup::new(supported()).await;
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 901, Ordering::SeqCst);
    assert!(matches!(
        setup.dispatch().await,
        Ok(DispatchOutcome::Terminal(Phase::Refused { code }))
            if code == super::expiry::EXPIRED_BEFORE_START
    ));
    assert!(setup.port.starts.lock().unwrap().is_empty());
    let unsupported = Setup::new(RecoverySupport::default()).await;
    assert!(matches!(
        unsupported.dispatch().await,
        Err(ExecutionError::Unsupported)
    ));
    assert!(unsupported.port.starts.lock().unwrap().is_empty());
}
