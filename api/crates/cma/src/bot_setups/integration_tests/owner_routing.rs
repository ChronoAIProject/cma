use super::*;
use layer2_core::{
    agent_ownership::{
        test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, PROTOCOL_VERSION,
    },
    AgentId,
};

#[tokio::test]
async fn delegated_delete_target_requires_current_event_binding_and_consent_without_writes() {
    let f = Fixture::new().await;
    let notification = f.activate().await;
    let before = f.peer.mint_calls.load(Ordering::SeqCst);
    for event in [EVENT.into(), capability(&notification)] {
        let body = json!({"method":"DELETE", "path":"/api/v1/agents/agt_delete", "idempotency_key":"delete-one", "event_ref":event});
        let target = f
            .authority
            .owner_delete_target(KEY, &serde_json::to_vec(&body).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(target.agent_id, AgentId("agt_delete".into()));
        assert_eq!(target.context.caller.subject.as_str(), "visitor");
        assert_eq!(target.credential.expose(), USER_TOKEN);
        assert!(f
            .authority
            .owner_delete_target("wrong-binding", &serde_json::to_vec(&body).unwrap())
            .await
            .is_err());
    }
    assert_eq!(
        before,
        f.peer.mint_calls.load(Ordering::SeqCst),
        "routing never mints a reply target"
    );
    f.authority
        .auth
        .revoke_actor_link(
            &authorization::actor(&f.actor),
            &notification.grant.grant_id,
        )
        .await
        .unwrap();
    let body = json!({"method":"DELETE", "path":"/api/v1/agents/agt_delete", "idempotency_key":"delete-one", "event_ref":capability(&notification)});
    assert!(f
        .authority
        .owner_delete_target(KEY, &serde_json::to_vec(&body).unwrap())
        .await
        .is_err());
}

#[tokio::test]
async fn delegated_delete_handler_cannot_bypass_positive_exact_owner_scope() {
    let f = Fixture::new().await;
    let notification = f.activate().await;
    let ownership = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::register(
        ownership.client(),
        Member {
            instance_id: "delegation-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap();
    let effects = Arc::new(AtomicUsize::new(0));
    let observed = effects.clone();
    let resources = Router::new().route(
        "/api/v1/agents/{agent}",
        axum::routing::delete(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Json(json!({"deleted":true})) }
        }),
    );
    let router = f
        .authority
        .delegation_router_with_ownership(resources, Some(coordinator.clone()));
    let intent = json!({"event_ref":capability(&notification),"method":"DELETE","path":"/api/v1/agents/agt_delete","idempotency_key":"delete-one"});
    let make = || {
        let mut request = Request::post("/api/v1/agent-delegations/operations")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(intent.to_string()))
            .unwrap();
        request.extensions_mut().insert(AuthMethod::BindingKey {
            binding_id: BINDING.into(),
        });
        request
            .extensions_mut()
            .insert(DelegatedCredential::new(KEY.into()));
        request
    };
    assert_eq!(
        router.clone().oneshot(make()).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let wrong = coordinator
        .acquire(&AgentId("agt_wrong".into()))
        .await
        .unwrap()
        .unwrap();
    let response = wrong
        .run(router.clone().oneshot(make()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let owner = coordinator
        .acquire(&AgentId("agt_delete".into()))
        .await
        .unwrap()
        .unwrap();
    let response = owner.run(router.oneshot(make())).await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}
