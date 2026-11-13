//! Independent desktop services share only Mongo and the external receiver.
use super::*;
use layer2_core::sandbox::store::{SandboxSessionStore, SandboxStore};
use layer2_core::sandbox::{DesktopError, DesktopFrame, DesktopService};
use layer2_core::{AgentId, Caller};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

mod fixture;
mod receiver;
use fixture::Services;

mod recovery_tests;
mod router_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn two_services_and_two_agents_share_one_exclusive_desktop() {
    let f = Services::new(true).await;
    let (left, right) = tokio::join!(
        f.left
            .open_authorized(&f.caller, &f.agents[0], f.authority()),
        f.right
            .open_authorized(&f.caller, &f.agents[1], f.authority()),
    );
    let (mut connection, service) = match (left, right) {
        (Ok(connection), Err(DesktopError::Busy)) => (connection, &f.left),
        (Err(DesktopError::Busy), Ok(connection)) => (connection, &f.right),
        _ => panic!("only one Agent on one Sandbox may attach across AM instances"),
    };
    connection
        .send(DesktopFrame::Binary(vec![1]))
        .await
        .unwrap();
    assert_eq!(f.receiver.frames(), 1);
    let prior = f.receiver.active_claim().await;
    service.close_connection(&mut connection).await;
    assert!(connection
        .send(DesktopFrame::Binary(vec![2]))
        .await
        .is_err());
    let mut successor = f
        .right
        .open_authorized(&f.caller, &f.agents[1], f.authority())
        .await
        .unwrap();
    let next = f.receiver.active_claim().await;
    assert!(next.epoch > prior.epoch);
    // A delayed duplicate close is exact-token and cannot revoke the successor.
    service.close_connection(&mut connection).await;
    successor.send(DesktopFrame::Binary(vec![3])).await.unwrap();
    f.right.close_connection(&mut successor).await;
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn runtime_open_failure_releases_only_a_definite_no_effect_claim() {
    let f = Services::new(true).await;
    f.receiver.fail_open.store(1, Ordering::SeqCst);
    assert!(matches!(
        f.left
            .open_authorized(&f.caller, &f.agents[0], f.authority())
            .await,
        Err(DesktopError::RuntimeUnavailable)
    ));
    assert!(!f
        .stores
        .right
        .has_open_sandbox_session(&f.stores.request.sandbox_id, &layer2_core::now_rfc3339())
        .await
        .unwrap());
    f.receiver.fail_open.store(2, Ordering::SeqCst);
    assert!(matches!(
        f.right
            .open_authorized(&f.caller, &f.agents[1], f.authority())
            .await,
        Err(DesktopError::RuntimeUnavailable)
    ));
    assert!(f
        .stores
        .left
        .has_open_sandbox_session(&f.stores.request.sandbox_id, &layer2_core::now_rfc3339())
        .await
        .unwrap());
    f.receiver.fail_open.store(0, Ordering::SeqCst);
    assert!(matches!(
        f.left
            .open_authorized(&f.caller, &f.agents[0], f.authority())
            .await,
        Err(DesktopError::Busy)
    ));
    assert_eq!(f.receiver.legacy_opens.load(Ordering::SeqCst), 0);
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn stalled_renewal_does_not_block_frames_but_closes_at_original_deadline() {
    let f = Services::new(true).await;
    f.receiver.block_renew();
    let mut connection = f
        .left
        .open_authorized(&f.caller, &f.agents[0], f.authority())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(4), f.receiver.renewing())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_millis(100),
        connection.send(DesktopFrame::Binary(vec![1])),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(f.receiver.frames(), 1);
    assert!(
        tokio::time::timeout(Duration::from_secs(11), connection.recv())
            .await
            .unwrap()
            .is_err()
    );
    assert!(connection
        .send(DesktopFrame::Binary(vec![2]))
        .await
        .is_err());
    assert!(matches!(
        f.right
            .open_authorized(&f.caller, &f.agents[1], f.authority())
            .await,
        Err(DesktopError::Busy)
    ));
    f.left.close_connection(&mut connection).await;
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn remote_authorization_change_ends_an_existing_stream() {
    let f = Services::new(true).await;
    let mut connection = f
        .left
        .open_authorized(&f.caller, &f.agents[0], f.authority())
        .await
        .unwrap();
    f.stop_agent(0).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), connection.recv())
            .await
            .unwrap()
            .is_err()
    );
    assert!(connection
        .send(DesktopFrame::Binary(vec![1]))
        .await
        .is_err());
    f.left.close_connection(&mut connection).await;
    let mut successor = f
        .right
        .open_authorized(&f.caller, &f.agents[1], f.authority())
        .await
        .unwrap();
    f.right.close_connection(&mut successor).await;
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn switch_off_preserves_legacy_desktop_protocol() {
    let f = Services::new(false).await;
    let mut connection = f
        .left
        .open_authorized(&f.caller, &f.agents[0], f.authority())
        .await
        .unwrap();
    assert_eq!(f.receiver.legacy_opens.load(Ordering::SeqCst), 1);
    assert!(matches!(
        f.left
            .open_authorized(&f.caller, &f.agents[1], f.authority())
            .await,
        Err(DesktopError::Busy)
    ));
    connection
        .send(DesktopFrame::Binary(vec![1]))
        .await
        .unwrap();
    f.left.close_connection(&mut connection).await;
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn store_outage_refuses_both_services_without_opening_a_socket() {
    let f = Services::new(true).await;
    f.stores
        .db
        .run_command(mongodb::bson::doc! {
            "collMod": "sandbox_sessions", "validator": { "$expr": { "$eq": [1, 0] } },
            "validationLevel": "strict",
        })
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        f.left
            .open_authorized(&f.caller, &f.agents[0], f.authority()),
        f.right
            .open_authorized(&f.caller, &f.agents[1], f.authority()),
    );
    assert!(matches!(left, Err(DesktopError::AuthorityUnavailable)));
    assert!(matches!(right, Err(DesktopError::AuthorityUnavailable)));
    assert_eq!(
        f.stores
            .db
            .collection::<mongodb::bson::Document>("sandbox_sessions")
            .count_documents(mongodb::bson::doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(f.receiver.legacy_opens.load(Ordering::SeqCst), 0);
    assert_eq!(f.receiver.frames(), 0);
    f.stores.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn replica_mode_refuses_a_captured_caller_without_current_credential_proof() {
    let f = Services::new(true).await;
    assert!(matches!(
        f.left.open(&f.caller, &f.agents[0]).await,
        Err(DesktopError::AuthorityUnavailable)
    ));
    assert!(matches!(
        f.right.open(&f.caller, &f.agents[1]).await,
        Err(DesktopError::AuthorityUnavailable)
    ));
    assert!(!f
        .stores
        .left
        .has_open_sandbox_session(&f.stores.request.sandbox_id, &layer2_core::now_rfc3339())
        .await
        .unwrap());
    f.stores.db.drop().await.unwrap();
}
