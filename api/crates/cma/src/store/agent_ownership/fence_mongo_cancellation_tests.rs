//! A caller cancelled during a fenced transaction's first command must not
//! strand a server-side lock on the instance-wide member row (#3189).
use super::*;
use crate::store::sandbox_mongo::start_transaction;
use std::time::Instant;

/// Stall whichever command opens the transaction, cancel the caller while it
/// is in flight, and let the stalled command finish on the server.
async fn cancel_first_command<F: std::future::Future>(f: &Fixture, operation: F) {
    f.admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
            "data": {
                "appName": f.app(), "failCommands": ["find", "update", "findAndModify"],
                "blockConnection": true, "blockTimeMS": 400_i32,
            },
        })
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), operation)
            .await
            .is_err(),
        "the stalled first command must be in flight when the caller is cancelled"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    // Turn the fault off without killing sessions: a stranded transaction must
    // stay visible to the competing writer below.
    f.admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
}

/// Another client cannot reuse the dropped server session, so only a lock
/// left by the cancelled transaction could keep its fence conflicting.
async fn independent_fence_succeeds_promptly(f: &Fixture) {
    let started = Instant::now();
    let mut conflicts = 0;
    loop {
        let mut session = f.admin.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let fenced = f
            .independent
            .fence_transaction_typed(&f.authority.token, &mut session)
            .await;
        let _ = session.abort_transaction().await;
        match fenced {
            Ok(()) => break,
            Err(FenceError::Driver(error)) if is_transient(&error) => conflicts += 1,
            Err(FenceError::Driver(error)) => panic!("unexpected fence driver error: {error}"),
            Err(FenceError::Domain(error)) => panic!("unexpected fence refusal: {error:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a cancelled first command stranded the member row lock ({conflicts} conflicts)"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn cancelled_fence_opening_its_transaction_strands_no_member_lock() {
    let f = Fixture::new().await;
    let ownership = f.store.agent_ownership.clone().unwrap();
    {
        let mut session = f.store.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        cancel_first_command(
            &f,
            ownership.fence_transaction_typed(&f.authority.token, &mut session),
        )
        .await;
    }
    independent_fence_succeeds_promptly(&f).await;
    f.clear().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn cancelled_renewal_strands_no_member_lock() {
    let f = Fixture::new().await;
    let ownership = f.store.agent_ownership.clone().unwrap();
    cancel_first_command(&f, ownership.renew(&f.authority.token)).await;
    independent_fence_succeeds_promptly(&f).await;
    f.clear().await;
    f.cleanup().await;
}
