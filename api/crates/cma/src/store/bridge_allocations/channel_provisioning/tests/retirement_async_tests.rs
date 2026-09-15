use super::super::lifecycle_tests::lifecycle_fixture::LifecycleFixture;
use super::*;
use crate::sandbox::channel_startup::test_support::retirement_workers;
use layer2_core::sandbox::{
    agent_io::*, AgentLogKind, AgentStatus, AgentStopReason, AgentWorkStep,
};
use std::sync::Arc;
#[path = "retirement_async_recovery_tests.rs"]
mod recovery;
#[path = "retirement_async_fixture.rs"]
mod support;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_real_async_stop_reconciles_original_turn_without_redispatch() {
    for completed in [false, true] {
        let lifecycle = LifecycleFixture::new().await;
        let (queued, dispatched) = support::dispatch(&lifecycle, "retirement-async-original").await;
        if completed {
            support::finish_turn(&lifecycle, &dispatched).await;
        }
        let resources = lifecycle.f.original.resources.clone();
        let registry = lifecycle.registry.clone();
        let (stopper, worker) = retirement_workers(
            resources.clone(),
            lifecycle.f.original.adapter.clone(),
            registry.clone(),
            "retirement-first-agent-worker",
        );
        let f = RetirementFixture::with_agents(lifecycle.f, registry.clone(), Some(stopper));
        let id = dispatched.receipt.envelope.agent_id.clone();
        let command_id = dispatched.receipt.envelope.command_id.clone();
        let source = f.db().collection::<Document>("bridge_callback_receipts");
        let original_source = source
            .find_one(doc! {"_id": &queued.inbox.id})
            .await
            .unwrap()
            .unwrap();
        let original_channel_source = support::source_records(&f, &dispatched).await;
        let prefix = support::log(&f).await;
        let before = f.sandbox().await;
        let original_agent = resources.get_agent(&id).await.unwrap().unwrap();
        f.start().await;
        f.step().await; // Durable root stop intent only.
        assert!(support::close_requests(&f).await.is_empty());
        f.step().await; // Production stopper schedules canonical Agent work.
        let scheduled = resources.get_agent(&id).await.unwrap().unwrap();
        assert!(scheduled.status.is_live());
        assert_eq!(
            scheduled.requested_stop_reason,
            Some(AgentStopReason::SandboxRetired)
        );
        assert_eq!(scheduled.work.step, AgentWorkStep::FenceInteraction);
        assert_eq!(
            f.receipt().await.unwrap().phase,
            RetirementPhase::StopIntent
        );
        assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
        assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 0);
        assert!(worker.run_one(&id, SystemTime::now()).await.unwrap());
        assert_eq!(
            resources.get_agent(&id).await.unwrap().unwrap().work.step,
            AgentWorkStep::CloseProcess
        );
        f.retry().await;
        f.step().await; // A pending close cannot release credential/runtime cleanup.
        assert_eq!(f.sandbox().await.credential, before.credential);
        assert_eq!(
            f.receipt().await.unwrap().phase,
            RetirementPhase::StopIntent
        );
        assert!(worker.run_one(&id, SystemTime::now()).await.unwrap());
        assert_eq!(
            resources.get_agent(&id).await.unwrap().unwrap().work.step,
            AgentWorkStep::PublishStopped
        );
        assert_eq!(support::close_requests(&f).await.len(), 1);
        assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
        assert!(worker.run_one(&id, SystemTime::now()).await.unwrap());
        let stopped = resources.get_agent(&id).await.unwrap().unwrap();
        assert_eq!(stopped.status, AgentStatus::Stopped);
        assert_eq!(stopped.stop_reason, Some(AgentStopReason::SandboxRetired));
        assert_eq!(stopped.work.step, AgentWorkStep::None);
        assert_eq!(
            stopped.conversation.conversation_id,
            original_agent.conversation.conversation_id
        );
        assert_eq!(
            stopped.creation_fingerprint,
            original_agent.creation_fingerprint
        );
        assert_eq!(stopped.profile, original_agent.profile);
        assert!(!worker.run_one(&id, SystemTime::now()).await.unwrap());
        assert_eq!(support::close_requests(&f).await.len(), 1);
        let reopened = Arc::new(
            crate::store::MongoSandboxStore::open(&f.f.original.client, &f.f.original.database)
                .await
                .unwrap(),
        );
        let commands = AgentCommandWorker::new(
            reopened.clone(),
            Arc::new(support::NoDispatch),
            "retirement-command-recovery".into(),
        )
        .unwrap();
        commands.run_one(&command_id).await.unwrap();
        let recovered = reopened.get_command(&command_id).await.unwrap().unwrap();
        assert_eq!(recovered.receipt, dispatched.receipt);
        assert_eq!(recovered.work.dispatch, dispatched.work.dispatch);
        assert_eq!(
            recovered.status,
            if completed {
                AgentResponseStatus::Completed
            } else {
                AgentResponseStatus::Failed
            }
        );
        if completed {
            assert!(recovered.work.failure.is_none());
        } else {
            assert_eq!(
                recovered.work.failure.as_ref().unwrap().code,
                CommandFailureCode::RuntimeLost
            );
        }
        commands.run_one(&command_id).await.unwrap();
        assert_eq!(
            reopened.get_command(&command_id).await.unwrap().unwrap(),
            recovered
        );
        assert!(reopened
            .claim_next_command(CommandClaimRequest {
                agent_id: id.clone(),
                holder: "no-retirement-redispatch".into(),
                now: layer2_core::now_rfc3339(),
                expires_at: layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(30))
            })
            .await
            .unwrap()
            .is_none());
        f.retry().await;
        for _ in 0..4 {
            f.step().await;
        }
        let after = f.sandbox().await;
        assert_eq!(after.work.step, SandboxWorkStep::MarkRetired);
        assert_eq!(after.status, SandboxStatus::Retiring);
        assert!(after.retired_at.is_none() && after.purge_after.is_none());
        assert_eq!(
            source
                .find_one(doc! {"_id": &queued.inbox.id})
                .await
                .unwrap()
                .unwrap(),
            original_source
        );
        assert_eq!(
            support::source_records(&f, &dispatched).await,
            original_channel_source
        );
        let log = support::log(&f).await;
        assert!(log.starts_with(&prefix));
        assert_eq!(
            log.iter()
                .filter(|event| matches!(event.kind, AgentLogKind::TurnStarted { .. }))
                .count(),
            1
        );
        assert_eq!(
            log.iter()
                .filter(|event| matches!(event.kind, AgentLogKind::Stopped { .. }))
                .count(),
            1
        );
        assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
        assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 1);
        assert_eq!(f.stops.calls.load(Ordering::SeqCst), 0);
        f.close().await;
    }
}
