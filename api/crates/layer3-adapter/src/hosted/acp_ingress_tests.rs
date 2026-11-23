use super::fixture::SyntheticSocket;
pub(super) use super::fixture::{fixture, Fixture};
use super::*;
use crate::hosted::update_relay::UpdateRelay;
use crate::ingress_budget::{AllocationPlan, IngressClass};
use crate::ConversationUpdate;
use serde_json::json;
use std::sync::Arc;

#[test]
fn bridge_identity_custody_survives_shared_owners_and_refuses_before_copying() {
    let Fixture {
        bridge,
        root,
        policy,
        acknowledgements,
        writes,
        ..
    } = fixture();
    assert_eq!(
        bridge._identity_storage.as_ref().unwrap().requested_bytes(),
        264
    );
    let available = policy.budget(IngressClass::Data).available_bytes();
    let blocker = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("bridge identity pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    assert!(matches!(
        super::identity::bridge_identity_storage(Some(&policy)),
        Err(crate::AdapterError::AtCapacity(_))
    ));
    assert!(super::identity::bridge_identity_storage(None)
        .unwrap()
        .is_none());
    drop((blocker, acknowledgements, writes, policy));
    let owner = Arc::new(bridge);
    let remaining = owner.clone();
    let held = root.available_bytes();
    drop(owner);
    assert_eq!(root.available_bytes(), held);
    assert!(held <= root.capacity_bytes() - 264);
    drop(remaining);
    root.assert_idle();
}

#[tokio::test]
async fn admitted_startup_cannot_fall_through_to_unowned_legacy_queue() {
    let fixture = fixture();
    let (updates, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let baseline = fixture.root.available_bytes();
    let result = fixture
        .bridge
        .route_update(
            "conversation",
            ConversationUpdate::Lost { frames: 1 },
            &updates,
        )
        .await;
    assert!(matches!(result, Err(crate::AdapterError::Rejected(_))));
    assert!(fixture.bridge.conversation.lock().await.startup.is_empty());
    assert_eq!(fixture.bridge.conversation.lock().await.startup_lost, 0);
    assert!(captured.take().unwrap().is_none());
    assert_eq!(fixture.root.available_bytes(), baseline);

    fixture.activate().await;
    fixture
        .bridge
        .route_update(
            "conversation",
            ConversationUpdate::Lost { frames: 1 },
            &updates,
        )
        .await
        .unwrap();
    assert!(matches!(
        captured.take().unwrap(),
        Some(ConversationUpdate::Lost { frames: 1 })
    ));
}

#[tokio::test]
async fn legacy_startup_still_queues_until_identity_acceptance() {
    let mut fixture = fixture();
    fixture.bridge.ingress = None;
    let (updates, mut captured) = UpdateRelay::capture();
    fixture
        .bridge
        .route_update(
            "conversation",
            ConversationUpdate::Lost { frames: 1 },
            &updates,
        )
        .await
        .unwrap();
    assert_eq!(fixture.bridge.conversation.lock().await.startup.len(), 1);
    assert!(captured.try_recv().is_err());
    fixture
        .bridge
        .activate_session("conversation".into(), &updates)
        .await
        .unwrap();
    assert!(matches!(
        captured.try_recv(),
        Ok(ConversationUpdate::WithoutPromptEvidence(update))
            if matches!(*update, ConversationUpdate::Lost { frames: 1 })
    ));
    assert!(fixture.bridge.conversation.lock().await.startup.is_empty());
}

#[tokio::test]
async fn typed_pressure_refuses_before_cursor_effect_or_ack() {
    let fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let value = json!({"method":"session/update", "params":{"sessionId":"conversation", "update":{
        "sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"x".repeat(256 * 1024)}
    }}});
    let wire = fixture.retained(&value, 1);
    let pools = fixture.bridge.json_pools.as_ref().unwrap();
    let before = fixture.root.available_bytes();
    let parsed = pools.decode(&wire[24..]).unwrap();
    let json_bytes = before - fixture.root.available_bytes();
    drop(parsed);
    let data = fixture.policy.budget(IngressClass::Data);
    // Leave space for the actual raw copy/node and JSON, but less than the
    // independent typed text. Retrying cursor 1 below proves no acceptance.
    let held_bytes = data.available_bytes() - json_bytes - wire.len() - 16 * 1024;
    let held = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("pressure payload", &[held_bytes]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; held_bytes - 512]);
    let failure = fixture
        .bridge
        .receive_output(&wire, &relay)
        .await
        .unwrap_err();
    assert!(matches!(failure, crate::AdapterError::AtCapacity(_)));
    assert!(captured.take().unwrap().is_none());
    assert!(fixture.writes.lock().unwrap().is_empty());
    drop(held);
    fixture.bridge.receive_output(&wire, &relay).await.unwrap();
    let delivery = captured.take().unwrap().unwrap();
    assert_eq!(delivery.delivery().receipt.unwrap().source.cursor, 1);
    let Some(ConversationUpdate::WithoutPromptEvidence(activity)) = delivery.delivery().effect
    else {
        panic!("passive delivery lost its activity wrapper");
    };
    assert!(matches!(activity.effect(), ConversationUpdate::Said(_)));
    assert!(delivery.turn_evidence().is_none());
}

#[tokio::test]
async fn complete_delivery_clones_share_receipt_and_hold_storage_until_last_drop() {
    let fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let before = fixture.root.available_bytes();
    let wire = fixture.retained(&json!({"method":"session/update", "params":{
        "sessionId":"conversation", "update":{"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"retained"}}
    }}), 1);
    fixture.bridge.receive_output(&wire, &relay).await.unwrap();
    let delivery = captured.take().unwrap().unwrap();
    let used = fixture.root.available_bytes();
    assert!(used < before);
    let clone = delivery.clone();
    assert!(std::ptr::eq(
        delivery.delivery().receipt.unwrap(),
        clone.delivery().receipt.unwrap()
    ));
    assert_eq!(fixture.root.available_bytes(), used);
    drop(delivery);
    assert_eq!(fixture.root.available_bytes(), used);
    drop(clone);
    assert_eq!(fixture.root.available_bytes(), before);
}

#[tokio::test]
async fn committed_ack_progresses_while_data_is_exhausted() {
    let mut fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let wire = fixture.retained(&json!({"method":"session/update", "params":{
        "sessionId":"conversation", "update":{"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"committed"}}
    }}), 1);
    fixture.bridge.receive_output(&wire, &relay).await.unwrap();
    let delivery = captured.take().unwrap().unwrap();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let held = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("saturated data", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    delivery
        .delivery()
        .receipt
        .unwrap()
        .acknowledge_admitted()
        .unwrap();
    let ack = fixture.acknowledgements.recv().await.unwrap();
    let state = fixture.bridge.output_ack_state().await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture.bridge.acknowledge_output(&*ack, state.as_ref()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(&*fixture.writes.lock().unwrap()[0], Message::Binary(bytes) if bytes.len() == 25 && bytes[0] == 5)
    );
    drop(held);
}

#[tokio::test]
async fn recovered_terminal_keeps_its_original_owner_after_json_drops() {
    let fixture = fixture();
    let prompt = crate::PromptIdentity::new("conversation", 1, 2);
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    *fixture.bridge.recovery.lock().await =
        super::super::recovery::PromptRecovery::new(Some(prompt.clone()), Some(&fixture.policy))
            .unwrap();
    let before = fixture.root.available_bytes();
    let value = json!({"id":prompt.as_str(), "result":{"stopReason":"end_turn", "_meta":{"text":"x".repeat(65536)}}});
    let terminal = fixture.bridge.prepare_terminal(&value).unwrap();
    let frame = fixture
        .bridge
        .json_pools
        .as_ref()
        .unwrap()
        .decode(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .with_prepared_terminal(terminal);
    assert!(fixture
        .bridge
        .recover_prompt_result(prompt.as_str(), &frame, &relay)
        .await
        .unwrap());
    drop(frame);
    assert!(captured.take().unwrap().is_none());
    assert!(fixture.root.available_bytes() < before);
    fixture.bridge.release_recovered_prompt(&relay).await;
    let terminal = captured.take().unwrap().unwrap();
    assert!(
        matches!(terminal.effect(), ConversationUpdate::TurnEndedForPrompt {prompt_id, ..} if prompt_id == &prompt)
    );
    drop(terminal);
    assert_eq!(fixture.root.available_bytes(), before);
}

#[test]
fn prompt_id_borrowed_check_matches_the_native_parser() {
    let valid = crate::PromptIdentity::new("conversation", 1, 2);
    for id in [
        valid.as_str(),
        "",
        "cma-prompt-v1-xyz",
        "1",
        "cma-rpc-fixture-1",
    ] {
        assert_eq!(
            valid_prompt_id(id),
            crate::PromptIdentity::parse(id).is_some()
        );
    }
}

#[tokio::test]
async fn prompt_admission_failure_does_not_publish_a_native_terminal() {
    let fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let content = vec![serde_json::from_value::<crate::ContentBlock>(
        json!({"type":"text", "text":"retry this exact prompt"}),
    )
    .unwrap()];
    let data = fixture.policy.budget(IngressClass::Data);
    let before = data.available_bytes();
    let preparation = super::identity::json_storage(Some(&fixture.policy), &content).unwrap();
    let preparation_bytes = before - data.available_bytes();
    drop(preparation);
    let held_bytes = before - 1024 - preparation_bytes;
    let held = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("pressure before RPC preparation", &[held_bytes]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; held_bytes - 512]);
    let result = fixture
        .bridge
        .drive(
            crate::ConversationInput::RecoverablePrompt {
                content,
                prompt_id: crate::PromptIdentity::new("conversation", 1, 2),
            },
            &relay,
        )
        .await;
    assert!(matches!(result, Err(crate::AdapterError::AtCapacity(_))));
    assert!(captured.take().unwrap().is_none());
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert!(fixture.bridge.pending.lock().await.is_empty());
    assert_eq!(
        fixture
            .bridge
            .owned_prompt_waiters
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    drop(held);
    assert_eq!(data.available_bytes(), before);
}

#[tokio::test]
async fn admitted_open_refuses_each_missing_prerequisite_before_polling_socket() {
    for missing in 0..5 {
        let fixture = fixture();
        let before = fixture.root.available_bytes();
        let mut settings = super::super::tests::settings(None, false);
        settings.retained_json = missing != 0;
        settings.bounded_raw_output = missing != 1;
        settings.bounded_permission_state = missing != 2;
        settings.shared_runtime_output = missing != 3;
        settings.output_json_budget = missing != 4;
        let (updates, _) = mpsc::channel(1);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            super::super::Bridge::open_admitted(
                Box::new(SyntheticSocket(fixture.writes.clone())),
                "agent",
                "/synthetic/work".into(),
                (settings, None, fixture.policy.admit_attachment().unwrap()),
                None,
                None,
                updates,
            ),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(error)
            if matches!(error.as_adapter(), Some(crate::AdapterError::Rejected(_)))));
        assert!(fixture.writes.lock().unwrap().is_empty());
        assert_eq!(fixture.root.available_bytes(), before);
    }
}
