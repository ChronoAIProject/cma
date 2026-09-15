use super::*;
#[path = "tests/boundaries.rs"]
mod boundaries;
#[path = "tests/channel_grants_tests.rs"]
mod channel_grants;
#[path = "../channel_children/mongo_tests.rs"]
mod children;
#[path = "tests/fixture.rs"]
pub(crate) mod fixture;
#[path = "tests/renewal.rs"]
mod renewal;
#[path = "tests/reuse.rs"]
pub(crate) mod reuse;
#[path = "tests/startup.rs"]
mod startup;

use crate::{
    auth::keys::bridge::test_fixture::{HUMAN_SECRET, RUNTIME_SECRET},
    sandbox::SandboxCredentialCustody,
};
use fixture::Fixture;
#[path = "tests/channel_reply_tests.rs"]
mod channel_reply;
#[path = "tests/command_reset_tests.rs"]
mod command_reset_tests;
#[path = "tests/command_revival_tests.rs"]
mod command_revival;
#[path = "tests/reuse_cleanup_tests.rs"]
mod reuse_cleanup;
#[path = "tests/reuse_command_fixture.rs"]
mod reuse_command_fixture;
#[path = "tests/reuse_command_reset_tests.rs"]
mod reuse_command_reset_tests;
#[path = "tests/reuse_command_tests.rs"]
mod reuse_command_tests;
#[path = "tests/reuse_pause_tests.rs"]
mod reuse_pause_tests;
#[path = "tests/stopped_reply_tests.rs"]
mod stopped_reply;
use layer2_core::sandbox::store::{SandboxStore, SandboxWorkerStore};
use layer3_adapter::{wire::WireOperation, RuntimeHandle};
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_driver_real_runtime_install_and_scoped_delivery_reopen_without_old_worker_work() {
    let mut f = Fixture::new().await;
    f.step().await.unwrap();
    f.step().await.unwrap();
    f.reopen_driver();
    let snapshot = f.children().await;
    assert_eq!(
        snapshot.state.phase,
        crate::sandbox::channel_provisioning::state::ChannelProvisioningPhase::WorkspaceAdmission
    );
    assert_eq!(f.server.state.mint_count.load(Ordering::SeqCst), 1);
    let pending = f
        .custody
        .get(&snapshot.sandbox.id, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.value.expose(), RUNTIME_SECRET);
    assert_eq!(snapshot.sandbox.credential.current, Some(pending.authority));
    assert!(snapshot.sandbox.credential.delivered);
    assert!(f
        .resources
        .due_sandbox_work("2999-01-01T00:00:00Z", 10)
        .await
        .unwrap()
        .is_empty());
    let runtime = f
        .backend
        .snapshot(&RuntimeHandle::new(
            &snapshot.sandbox.runtime.as_ref().unwrap().runtime_id,
        ))
        .await
        .unwrap();
    assert!(runtime.credential_delivered);
    assert!(runtime.workspaces.is_empty() && runtime.agents.is_empty());
    assert!(!serde_json::to_string(&runtime.request)
        .unwrap()
        .contains(HUMAN_SECRET));
    assert!(!serde_json::to_string(&runtime.request)
        .unwrap()
        .contains(RUNTIME_SECRET));
    let stored = f
        .client
        .database(&f.database)
        .collection::<mongodb::bson::Document>("sandbox_credentials")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{stored:?}").contains(RUNTIME_SECRET));
    assert!(f
        .resources
        .get_sandbox(&snapshot.sandbox.id)
        .await
        .unwrap()
        .is_some());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_driver_lost_install_reply_reconciles_same_runtime_and_operation_after_restart() {
    let mut f = Fixture::new().await;
    f.step().await.unwrap();
    f.step().await.unwrap();
    f.backend.lose_next_response(WireOperation::Install);
    assert!(f.step().await.is_err());
    f.release_current_claim().await;
    f.reopen_driver();
    let snapshot = f.children().await;
    assert!(snapshot.sandbox.credential.delivered);
    let installs = f
        .backend
        .requests()
        .await
        .into_iter()
        .filter(|(_, frame)| frame.operation() == WireOperation::Install)
        .collect::<Vec<_>>();
    assert_eq!(installs.len(), 2);
    assert_eq!(installs[0].0, installs[1].0);
    assert_eq!(installs[0].1, installs[1].1);
    assert_eq!(f.server.state.mint_count.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_driver_unknown_mint_is_held_after_restart_and_never_issues_second_post() {
    let mut f = Fixture::new().await;
    for _ in 0..3 {
        f.step().await.unwrap();
    }
    f.server
        .state
        .mint_unavailable
        .store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    assert_eq!(f.server.state.mint_count.load(Ordering::SeqCst), 1);
    f.release_current_claim().await;
    f.reopen_driver();
    f.server
        .state
        .mint_unavailable
        .store(false, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    assert_eq!(f.server.state.mint_count.load(Ordering::SeqCst), 1);
    assert!(f
        .custody
        .get(&f.record.placement.target().sandbox_id, 1)
        .await
        .unwrap()
        .is_none());
    let sandbox = f
        .resources
        .get_sandbox(&f.record.placement.target().sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!sandbox.credential.delivered && sandbox.credential.pending.is_none());
    assert!(f
        .resources
        .due_sandbox_work("2999-01-01T00:00:00Z", 10)
        .await
        .unwrap()
        .is_empty());
    f.close().await;
}
