use super::*;
use std::sync::atomic::Ordering;
use test_fixture::{lease, Server, RUNTIME_SECRET};

fn connector() -> layer2_core::bridge::BridgeConnector {
    layer2_core::bridge::BridgeConnector {
        issuer: "https://nyx.example".into(),
        base_url: "https://nyx.example".into(),
        bot_id: "bot-one".into(),
        platform: layer2_core::bridge::BridgePlatform::Telegram,
        route_ids: vec!["route-one".into()],
        key_id: "selected-key".into(),
        custody: layer2_core::bridge::BridgeSecretRef::new(
            "brsec_00000000000000000000000000000001".into(),
        )
        .unwrap(),
        callback_url: "https://cma.example/callback".into(),
    }
}
fn bridge() -> layer2_core::bridge::BridgeId {
    layer2_core::bridge::BridgeId("fixture-bridge".into())
}
fn service() -> ServiceId {
    ServiceId::new(
        layer3_adapter::AgentProvider::Codex
            .connection_services()
            .managed,
    )
}

#[tokio::test]
async fn human_scope_preflight_has_no_key_effect_and_mint_verifies_person_or_exact_org_metadata() {
    for owner in [
        ResourceOwner::Person {
            subject: SubjectId::new("fixture-person"),
        },
        ResourceOwner::Organization {
            subject: SubjectId::new("fixture-org"),
        },
    ] {
        let lease = lease(owner, bridge(), connector(), 1);
        let server = Server::new(&lease, "test").await;
        let plan = server
            .issuer
            .plan_bridge_sandbox_key(&lease, "sbx_channel_1", &[service()])
            .await
            .unwrap();
        assert_eq!(server.state.mint_count.load(Ordering::SeqCst), 0);
        let material = server
            .issuer
            .mint_bridge_sandbox_key(&lease, plan)
            .await
            .unwrap();
        assert_eq!(material.value.expose(), RUNTIME_SECRET);
        assert_eq!(server.state.mint_count.load(Ordering::SeqCst), 1);
        let custody_expiry = mongodb::bson::DateTime::parse_rfc3339_str(layer2_core::rfc3339_at(
            material.expires_at,
        ))
        .unwrap()
        .to_system_time();
        assert_eq!(material.expires_at, custody_expiry);
        let material = MintedKey {
            expires_at: custody_expiry,
            ..material
        };
        let fresh = server
            .issuer
            .plan_bridge_sandbox_key(&lease, "sbx_channel_1", &[service()])
            .await
            .unwrap();
        server
            .issuer
            .require_bridge_sandbox_material(&lease, &fresh, &material)
            .await
            .unwrap();
        assert!(!format!("{material:?}").contains(RUNTIME_SECRET));
    }
}

#[tokio::test]
async fn key_plan_cannot_cross_same_metadata_revision_control_plane_grant_renewal() {
    let owner = ResourceOwner::Organization {
        subject: SubjectId::new("fixture-org"),
    };
    let old = lease(owner.clone(), bridge(), connector(), 1);
    let server = Server::new(&old, "test").await;
    let plan = server
        .issuer
        .plan_bridge_sandbox_key(&old, "sbx_channel_1", &[service()])
        .await
        .unwrap();
    let replacement = lease(owner, bridge(), connector(), 1);
    assert!(server
        .issuer
        .mint_bridge_sandbox_key(&replacement, plan)
        .await
        .is_err());
    assert_eq!(server.state.mint_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn key_post_metadata_owner_mismatch_returns_no_deliverable_secret_or_automatic_retry() {
    let lease = lease(
        ResourceOwner::Person {
            subject: SubjectId::new("fixture-person"),
        },
        bridge(),
        connector(),
        1,
    );
    let server = Server::new(&lease, "test").await;
    let plan = server
        .issuer
        .plan_bridge_sandbox_key(&lease, "sbx_channel_1", &[service()])
        .await
        .unwrap();
    server
        .state
        .wrong_metadata_owner
        .store(true, Ordering::SeqCst);
    let result = server.issuer.mint_bridge_sandbox_key(&lease, plan).await;
    assert!(result.is_err());
    assert!(!format!("{result:?}").contains(RUNTIME_SECRET));
    assert_eq!(server.state.mint_count.load(Ordering::SeqCst), 1);
}
