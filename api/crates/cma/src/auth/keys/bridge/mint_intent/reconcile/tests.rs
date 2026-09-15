use super::*;
use crate::auth::keys::bridge::test_fixture::{lease, Server, HUMAN_SECRET, RUNTIME_SECRET};
use layer2_core::{
    bridge::{BridgeConnector, BridgeId, BridgePlatform, BridgeSecretRef},
    ServiceId, SubjectId,
};
use serde_json::json;
use std::{sync::atomic::Ordering, time::SystemTime};

fn human(owner: ResourceOwner) -> BridgeControlPlaneLease {
    lease(
        owner,
        BridgeId("fixture-reconciliation".into()),
        BridgeConnector {
            issuer: "https://nyx.example".into(),
            base_url: "https://nyx.example".into(),
            bot_id: "bot-one".into(),
            platform: BridgePlatform::Telegram,
            route_ids: vec!["route-one".into()],
            key_id: "selected-key".into(),
            custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into()).unwrap(),
            callback_url: "https://cma.example/callback".into(),
        },
        1,
    )
}
fn service() -> ServiceId {
    ServiceId::new(
        layer3_adapter::AgentProvider::Codex
            .connection_services()
            .managed,
    )
}

#[tokio::test]
async fn uncertain_mint_observation_uses_exact_persisted_scope_and_original_person_or_org() {
    for owner in [
        ResourceOwner::Person {
            subject: SubjectId::new("fixture-person"),
        },
        ResourceOwner::Organization {
            subject: SubjectId::new("fixture-org"),
        },
    ] {
        let human = human(owner);
        let server = Server::new(&human, "reconciliation-test").await;
        let plan = server
            .issuer
            .plan_bridge_sandbox_key(&human, "sbx_channel_1", &[service()])
            .await
            .unwrap();
        let intent = plan.mint_intent().unwrap();
        let bytes = serde_json::to_string(&intent).unwrap();
        assert!(!bytes.contains(HUMAN_SECRET) && !bytes.contains(RUNTIME_SECRET));
        let intent = serde_json::from_str(&bytes).unwrap();
        server.state.mint_unavailable.store(true, Ordering::SeqCst);
        assert!(server
            .issuer
            .mint_bridge_sandbox_key(&human, plan)
            .await
            .is_err());
        let observed = server
            .issuer
            .observe_bridge_sandbox_mint(&human, &intent, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.provider_key_id, "key-created");
        assert_eq!(observed.provider_state_version, 1);
        assert_eq!(server.state.mint_count.load(Ordering::SeqCst), 1);
        assert_eq!(server.state.revoke_count.load(Ordering::SeqCst), 0);
        *server.state.listing_override.lock().unwrap() = Some(json!({"keys":[]}));
        assert!(server
            .issuer
            .observe_bridge_sandbox_mint(&human, &intent, 1)
            .await
            .unwrap()
            .is_none());
        assert_eq!(server.state.mint_count.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn orphan_observation_refuses_ambiguous_listing_changed_owner_scope_and_expiry() {
    let human = human(ResourceOwner::Organization {
        subject: SubjectId::new("fixture-org"),
    });
    let server = Server::new(&human, "reconciliation-test").await;
    let plan = server
        .issuer
        .plan_bridge_sandbox_key(&human, "sbx_channel_1", &[service()])
        .await
        .unwrap();
    let intent = plan.mint_intent().unwrap();
    server
        .issuer
        .mint_bridge_sandbox_key(&human, plan)
        .await
        .unwrap();
    let name = server.state.create_body.lock().unwrap().as_ref().unwrap()["name"].clone();
    let row = json!({"id":"key-created","name":name,"created_at":layer2_core::rfc3339_at(SystemTime::now())});
    let mut other = row.clone();
    other["id"] = json!("another-key");
    for listing in [
        json!({"keys":[row.clone(),row.clone()]}),
        json!({"keys":[row.clone(),other]}),
        json!({"keys":[row],"next_cursor":"partial"}),
    ] {
        *server.state.listing_override.lock().unwrap() = Some(listing);
        assert!(server
            .issuer
            .observe_bridge_sandbox_mint(&human, &intent, 1)
            .await
            .is_err());
    }
    *server.state.listing_override.lock().unwrap() = None;
    server
        .state
        .wrong_metadata_owner
        .store(true, Ordering::SeqCst);
    assert!(server
        .issuer
        .observe_bridge_sandbox_mint(&human, &intent, 1)
        .await
        .is_err());
    server
        .state
        .wrong_metadata_owner
        .store(false, Ordering::SeqCst);
    for (field, value) in [
        ("grant_fingerprint", json!("b".repeat(64))),
        ("expires_at", json!("2099-01-01T00:00:00Z")),
    ] {
        let mut changed = serde_json::to_value(&intent).unwrap();
        changed[field] = value;
        let changed = serde_json::from_value(changed).unwrap();
        assert!(server
            .issuer
            .observe_bridge_sandbox_mint(&human, &changed, 1)
            .await
            .is_err());
    }
    assert_eq!(server.state.revoke_count.load(Ordering::SeqCst), 0);
    assert_eq!(server.state.mint_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn another_human_grant_incarnation_cannot_reconcile_a_retained_intent() {
    let owner = ResourceOwner::Organization {
        subject: SubjectId::new("fixture-org"),
    };
    let original = human(owner.clone());
    let server = Server::new(&original, "reconciliation-test").await;
    let intent = server
        .issuer
        .plan_bridge_sandbox_key(&original, "sbx_channel_1", &[service()])
        .await
        .unwrap()
        .mint_intent()
        .unwrap();
    let replacement = human(owner);
    assert!(server
        .issuer
        .observe_bridge_sandbox_mint(&replacement, &intent, 1)
        .await
        .is_err());
    assert_eq!(server.state.list_count.load(Ordering::SeqCst), 0);
    assert_eq!(server.state.mint_count.load(Ordering::SeqCst), 0);
}
