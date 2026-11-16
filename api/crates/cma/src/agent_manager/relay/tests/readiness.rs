use super::*;

#[tokio::test]
async fn dormant_owner_route_refuses_until_activation_without_disturbing_healthy_owner() {
    for terminate in [false, true] {
        let (store, instances) = cluster_with_starting(2, true).await;
        let id = AgentId("agt-startup-owner".into());
        let authority = instances[0]
            .coordinator
            .acquire(&id)
            .await
            .unwrap()
            .unwrap();
        let starting = &instances[1];
        assert!(starting.coordinator.is_starting());
        assert_eq!(store.membership().await.unwrap().members.len(), 1);
        let request = || {
            client()
                .post(format!("{}/api/v1/agents/{id}/files", starting.address))
                .header("authorization", "Bearer synthetic-authorized")
                .header("idempotency-key", "startup-request")
                .body("original body")
        };
        assert_eq!(
            request().send().await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(store
            .owner(&id)
            .await
            .unwrap()
            .is_some_and(|owner| owner.token == authority.token));
        assert!(starting.coordinator.owned().await.is_empty());
        if terminate {
            starting.coordinator.stop_admission();
        } else {
            starting.coordinator.activate().await.unwrap();
        }
        let response = request().send().await.unwrap();
        if terminate {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(starting.coordinator.owned().await.is_empty());
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["instance"], instances[0].coordinator.instance_id);
            assert_eq!(body["epoch"], authority.token.epoch);
            assert_eq!(body["bytes"], 13);
            assert_eq!(body["idempotency"], "startup-request");
        }
        assert!(authority.is_current());
    }
}
