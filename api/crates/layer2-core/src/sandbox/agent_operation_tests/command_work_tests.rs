use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use layer3_adapter::ConversationInputSender;

use super::fixtures::*;
use super::ledger_tests::{record, reservation};
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentLogStore, AgentStore};
use crate::sandbox::*;

fn after(at: &str, seconds: u64) -> String {
    crate::rfc3339_at(
        UNIX_EPOCH + Duration::from_secs(crate::seconds_from_rfc3339(at).unwrap() + seconds),
    )
}

fn request(agent: &Agent, holder: &str, now: &str) -> CommandClaimRequest {
    CommandClaimRequest {
        agent_id: agent.id.clone(),
        holder: holder.into(),
        now: now.into(),
        expires_at: after(now, 30),
    }
}

async fn fixture(status: AgentStatus) -> (Arc<InMemorySandboxStore>, Agent, Sandbox) {
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = seed_active_parent(&store).await;
    let agent = test_agent(status, None);
    store.create_agent(agent.clone()).await.unwrap();
    (store, agent, parent)
}

async fn admit(
    store: &InMemorySandboxStore,
    agent: &Agent,
    parent: &Sandbox,
    key: &str,
    command: AgentCommand,
) -> AdmittedAgentCommand {
    let mut reservation = reservation(agent, parent, key);
    reservation.envelope.command = command;
    reservation.envelope.request_fingerprint = reservation.envelope.command.fingerprint().unwrap();
    reservation.admitted_at = crate::now_rfc3339();
    reservation.queue_deadline = after(&reservation.admitted_at, 300);
    record(store.reserve_command(reservation).await.unwrap())
}

#[tokio::test]
async fn competing_workers_take_one_lane_and_expiry_fences_every_old_write() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
    admit(&store, &agent, &parent, "two", AgentCommand::message("two")).await;
    let now = crate::now_rfc3339();
    let (a, b) = tokio::join!(
        store.claim_next_command(request(&agent, "a", &now)),
        store.claim_next_command(request(&agent, "b", &now))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let old = a.or(b).unwrap();
    assert_eq!(old.receipt.agent_order, 1);
    let old_claim = &old.work.lease.as_ref().unwrap().claim;
    let next_time = after(&now, 30);
    let next = store
        .claim_next_command(request(&agent, "replacement", &next_time))
        .await
        .unwrap()
        .unwrap();
    let claim = &next.work.lease.as_ref().unwrap().claim;
    assert_eq!(claim.command_id, old_claim.command_id);
    assert!(claim.fence > old_claim.fence);
    assert_eq!(
        store
            .renew_command_claim(old_claim, next_time.clone(), after(&next_time, 30))
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert_eq!(
        store
            .release_command_claim(old_claim, next_time.clone())
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert_eq!(
        store
            .reject_command(
                old_claim,
                CommandFailureCode::AuthorityRevoked,
                next_time.clone()
            )
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    store
        .renew_command_claim(claim, after(&next_time, 1), after(&next_time, 40))
        .await
        .unwrap();
    assert_eq!(
        store
            .renew_command_claim(claim, after(&next_time, 1), after(&next_time, 39))
            .await,
        Err(CommandLedgerError::Invalid)
    );
    assert_eq!(
        store
            .reject_command(claim, CommandFailureCode::QueueTimeout, next_time.clone())
            .await,
        Err(CommandLedgerError::Invalid)
    );
    store
        .reject_command(
            claim,
            CommandFailureCode::AuthorityRevoked,
            next_time.clone(),
        )
        .await
        .unwrap();
    let second = store
        .claim_next_command(request(&agent, "next", &next_time))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.receipt.agent_order, 2);
    assert!(second.work.lease.unwrap().claim.fence > claim.fence);
}

#[tokio::test]
async fn busy_messages_wait_while_control_keeps_its_original_turn() {
    let (store, agent, parent) = fixture(AgentStatus::Working).await;
    admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
    let now = crate::now_rfc3339();
    assert!(store
        .claim_next_command(request(&agent, "a", &now))
        .await
        .unwrap()
        .is_none());
    let control = admit(&store, &agent, &parent, "two", AgentCommand::Interrupt).await;
    assert_eq!(control.target_turn, agent.conversation.active_turn);
    let selected = store
        .claim_next_command(request(&agent, "a", &now))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        selected.receipt.envelope.command_id,
        control.receipt.envelope.command_id
    );
    assert_eq!(selected.receipt.agent_order, 2);
}

#[tokio::test]
async fn expired_work_is_refused_without_dispatch_and_keeps_its_receipt() {
    let (store, agent, parent) = fixture(AgentStatus::Working).await;
    let mut reservation = reservation(&agent, &parent, "expired");
    reservation.queue_deadline = LATER.into();
    let admitted = record(store.reserve_command(reservation.clone()).await.unwrap());
    let selected = store
        .claim_next_command(request(&agent, "a", LATEST))
        .await
        .unwrap()
        .unwrap();
    let terminal = store
        .reject_command(
            &selected.work.lease.unwrap().claim,
            CommandFailureCode::QueueTimeout,
            LATEST.into(),
        )
        .await
        .unwrap();
    assert_eq!(terminal.status, AgentResponseStatus::Failed);
    assert_eq!(terminal.receipt, admitted.receipt);
    assert_eq!(
        record(store.reserve_command(reservation).await.unwrap()),
        terminal
    );
    assert!(store
        .claim_next_command(request(&agent, "b", LATEST))
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn dispatch_commits_exact_command_before_runtime_input_and_cannot_be_replayed() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "one",
        AgentCommand::message("Inspect this."),
    )
    .await;
    let claimed = store
        .claim_next_command(request(&agent, "a", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let sends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured_sends = sends.clone();
    let captured = store.clone();
    let id = admitted.receipt.envelope.command_id.clone();
    let sender = ConversationInputSender::spawn(4, move |_| {
        captured_sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let store = captured.clone();
        let id = id.clone();
        async move {
            let command = store.get_command(&id).await.unwrap().unwrap();
            let dispatch = command.work.dispatch.unwrap();
            let current = store
                .get_agent(&dispatch.claim.agent_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                current.conversation.operation.unwrap().sequence,
                dispatch.operation_sequence
            );
            assert_eq!(command.status, AgentResponseStatus::InProgress);
            assert!(store.agent_log(&current.id, 0, 20).await.unwrap().iter().any(|entry| matches!(&entry.kind, AgentLogKind::TurnStarted { origin_key: Some(key), .. } if key == &format!("command:{}", id.as_str()))));
            Ok(())
        }
    });
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let caller = owner();
    let (first, duplicate) = tokio::join!(
        service.execute_claimed_command(&caller, &claimed),
        service.execute_claimed_command(&caller, &claimed)
    );
    assert_ne!(first.is_ok(), duplicate.is_ok());
    assert_eq!(sends.load(std::sync::atomic::Ordering::SeqCst), 1);
    let stored = store
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.receipt, admitted.receipt);
    assert!(stored.work.dispatch.is_some());
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    assert_eq!(
        store
            .reject_command(
                claim,
                CommandFailureCode::RuntimeUnavailable,
                crate::now_rfc3339()
            )
            .await,
        Err(CommandLedgerError::Conflict)
    );
    store
        .release_command_claim(claim, crate::now_rfc3339())
        .await
        .unwrap();
    assert!(store
        .claim_next_command(request(&agent, "replacement", &crate::now_rfc3339()))
        .await
        .unwrap()
        .is_none());
    assert!(service
        .execute_claimed_command(&owner(), &claimed)
        .await
        .is_err());
    assert_eq!(
        store
            .agent_log(&agent.id, 0, 50)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn substituted_input_or_metadata_rolls_back_both_agent_and_ledger() {
    for metadata_only in [false, true] {
        let (store, agent, parent) = fixture(AgentStatus::Ready).await;
        admit(
            &store,
            &agent,
            &parent,
            "one",
            AgentCommand::message("original"),
        )
        .await;
        let claimed = store
            .claim_next_command(request(&agent, "a", &crate::now_rfc3339()))
            .await
            .unwrap()
            .unwrap();
        let mut changed = claimed.clone();
        if let AgentCommand::Message {
            input, metadata, ..
        } = &mut changed.receipt.envelope.command
        {
            if metadata_only {
                metadata.insert("extra".into(), "different".into());
            } else {
                *input = AgentInput::text("substituted");
            }
        }
        changed.receipt.envelope.request_fingerprint =
            changed.receipt.envelope.command.fingerprint().unwrap();
        let (sender, inputs) = recording_sender(Ok(()));
        let service = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        assert_eq!(
            service
                .execute_claimed_command(&owner(), &changed)
                .await
                .unwrap_err(),
            AgentOperationError::Conflict
        );
        assert!(inputs.lock().unwrap().is_empty());
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        assert_eq!(
            store
                .get_command(&claimed.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(claimed)
        );
        assert!(store.agent_log(&agent.id, 0, 20).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn changed_turn_rejects_a_claimed_control_without_touching_the_new_turn() {
    let (store, agent, parent) = fixture(AgentStatus::Working).await;
    admit(&store, &agent, &parent, "one", AgentCommand::Interrupt).await;
    let claimed = store
        .claim_next_command(request(&agent, "a", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let mut next = agent.clone();
    next.conversation.active_turn = Some(ACTIVE_TURN + 1);
    next.revision += 1;
    store
        .save_agent(agent.revision, next.clone())
        .await
        .unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert_eq!(
        service
            .execute_claimed_command(&owner(), &claimed)
            .await
            .unwrap_err(),
        AgentOperationError::Conflict
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(next));
}

#[tokio::test]
async fn expired_or_replaced_claim_never_reaches_runtime() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    admit(&store, &agent, &parent, "one", AgentCommand::message("one")).await;
    let past = crate::rfc3339_at(SystemTime::now() - Duration::from_secs(60));
    let old = store
        .claim_next_command(request(&agent, "a", &past))
        .await
        .unwrap()
        .unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    assert_eq!(
        service
            .execute_claimed_command(&owner(), &old)
            .await
            .unwrap_err(),
        AgentOperationError::Conflict
    );
    let live = store
        .claim_next_command(request(&agent, "b", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    store
        .release_command_claim(
            &live.work.lease.as_ref().unwrap().claim,
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
    store
        .claim_next_command(request(&agent, "c", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        service
            .execute_claimed_command(&owner(), &live)
            .await
            .unwrap_err(),
        AgentOperationError::Conflict
    );
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn channel_attribution_cannot_impersonate_a_personal_caller() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let mut reservation = reservation(&agent, &parent, "channel");
    reservation.admitted_at = crate::now_rfc3339();
    reservation.queue_deadline = after(&reservation.admitted_at, 300);
    reservation.envelope.actor = AgentActor::Channel {
        bridge_id: "bridge".into(),
        grant_id: "grant".into(),
        platform: "fixture".into(),
        chat_id: "chat".into(),
        sender_id: "sender".into(),
    };
    store.reserve_command(reservation).await.unwrap();
    let claimed = store
        .claim_next_command(request(&agent, "a", &crate::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service =
        AgentOperationService::new(store, TestRuntimeRegistry::new(full_capabilities(), sender));
    assert_eq!(
        service
            .execute_claimed_command(&owner(), &claimed)
            .await
            .unwrap_err(),
        AgentOperationError::Forbidden
    );
    assert!(inputs.lock().unwrap().is_empty());
}
