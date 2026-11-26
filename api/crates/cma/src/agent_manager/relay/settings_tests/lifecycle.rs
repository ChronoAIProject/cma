use super::*;

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn settings_owner_handoff_preserves_completed_replay_and_refuses_stale_proof() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let missing = replicas[0]
        .request(Method::PATCH, Some("handoff-settings"), Some(changed()))
        .await;
    assert_eq!(missing.0, StatusCode::SERVICE_UNAVAILABLE, "{}", missing.1);
    assert_eq!(missing.1["code"], "agent_owner_unavailable");
    assert!(replicas
        .iter()
        .all(|replica| replica.runtime.resolutions.lock().unwrap().is_empty()));
    let (owner, authority) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let initial = replicas[peer]
        .request(Method::PATCH, Some("handoff-settings"), Some(changed()))
        .await;
    assert_eq!(initial.0, StatusCode::OK, "{}", initial.1);
    let admitted = durable
        .get_agent(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    let uri: Uri = format!("/api/v1/agents/{AGENT}/settings").parse().unwrap();
    let original =
        json!({"model":"gpt-6","reasoning_effort":"high","permission_mode":"autonomous"});
    let body = serde_json::to_vec(&original).unwrap();
    let headers = HeaderMap::from_iter([
        (
            axum::http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        ),
        (
            axum::http::HeaderName::from_static("idempotency-key"),
            "stale-settings".parse().unwrap(),
        ),
    ]);
    let relay = replicas[peer].relay.as_ref().unwrap();
    let captured_proof = relay
        .authentication
        .sign(
            &replicas[peer].coordinator.as_ref().unwrap().instance_id,
            &authority.token,
            &Method::PATCH,
            &uri,
            &headers,
            &body,
        )
        .unwrap();
    let old = replicas[owner].coordinator.as_ref().unwrap();
    let drained = old.begin_drain().await;
    let unavailable = replicas[peer]
        .request(Method::PATCH, Some("during-drain"), Some(changed()))
        .await;
    assert_eq!(
        unavailable.0,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        unavailable.1
    );
    assert_eq!(unavailable.1["code"], "agent_owner_unavailable");
    for owned in drained {
        old.handoff(&owned, async { Ok(()) }).await.unwrap();
    }
    assert!(!authority.is_current());
    *replicas[owner].runtime.target.lock().unwrap() = None;
    for replica in &replicas {
        assert_eq!(replica.request(Method::GET, None, None).await, initial);
    }
    let next = replicas[peer]
        .coordinator
        .as_ref()
        .unwrap()
        .acquire(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    assert!(next.token.epoch > authority.token.epoch);
    replicas[peer].runtime.attach();
    for replica in &replicas {
        let stale = client()
            .patch(format!("{}{uri}", replica.address))
            .headers(headers.clone())
            .header(PROOF, captured_proof.clone())
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut wrong_epoch = next.token.clone();
    wrong_epoch.epoch = authority.token.epoch;
    let proof = relay
        .authentication
        .sign(
            &next.token.instance_id,
            &wrong_epoch,
            &Method::PATCH,
            &uri,
            &headers,
            &body,
        )
        .unwrap();
    let response = client()
        .patch(format!("{}{uri}", replicas[peer].address))
        .headers(headers)
        .header(PROOF, proof)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());
    assert_eq!(
        durable.get_agent(&admitted.id).await.unwrap().unwrap(),
        admitted
    );
    assert_eq!(
        replicas[peer]
            .request(Method::PATCH, Some("handoff-settings"), Some(changed()))
            .await,
        initial
    );
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());
    assert_eq!(
        durable.get_agent(&admitted.id).await.unwrap().unwrap(),
        admitted
    );
    let updated = replicas[peer]
        .request(Method::PATCH, Some("new-owner-settings"), Some(original))
        .await;
    assert_eq!(updated.0, StatusCode::OK, "{}", updated.1);
    assert_eq!(updated.1["reasoning_effort"], "high");
    assert_eq!(
        replicas[owner].request(Method::GET, None, None).await,
        updated
    );
    assert_eq!(replicas[owner].runtime.inputs.lock().unwrap().len(), 1);
    assert_eq!(replicas[peer].runtime.inputs.lock().unwrap().len(), 1);
    assert!(replicas[peer]
        .runtime
        .resolutions
        .lock()
        .unwrap()
        .iter()
        .all(|(_, scope)| scope.as_ref() == Some(&next.token)));
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn settings_noop_needs_no_runtime_effect_and_rejects_changed_idempotency_body() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let (owner, _) = fixture::acquire(&replicas).await;
    let before = durable
        .get_agent(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    let original =
        json!({"model":"gpt-6","reasoning_effort":"high","permission_mode":"autonomous"});
    let result = replicas[1 - owner]
        .request(
            Method::PATCH,
            Some("unchanged-settings"),
            Some(original.clone()),
        )
        .await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert_eq!(result.1["pending"], false);
    assert!(replicas
        .iter()
        .all(|replica| replica.runtime.resolutions.lock().unwrap().is_empty()));
    assert_eq!(
        durable.get_agent(&before.id).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        replicas[owner]
            .request(Method::PATCH, Some("unchanged-settings"), Some(original))
            .await,
        result
    );
    let (status, problem) = replicas[owner]
        .request(Method::PATCH, Some("unchanged-settings"), Some(changed()))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "idempotency_key_reused");
    assert!(replicas
        .iter()
        .all(|replica| replica.runtime.inputs.lock().unwrap().is_empty()));
    assert_eq!(
        durable.get_agent(&before.id).await.unwrap().unwrap(),
        before
    );
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn committed_settings_with_lost_http_response_replay_across_replicas_once() {
    use std::sync::atomic::Ordering;
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let (owner, _) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    replicas[peer].drop_response.store(true, Ordering::SeqCst);
    let response = client()
        .patch(format!(
            "{}/api/v1/agents/{AGENT}/settings",
            replicas[peer].address
        ))
        .header("idempotency-key", "lost-settings-response")
        .json(&changed())
        .send()
        .await;
    if let Ok(response) = response {
        assert!(response.bytes().await.is_err());
    }
    assert!(!replicas[peer].drop_response.load(Ordering::SeqCst));
    let saved = durable
        .get_agent(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    assert!(!saved.settings_pending());
    assert_eq!(
        saved.settings().reasoning_effort,
        layer2_core::profiles::ThinkingEffort::Medium
    );
    for replica in &replicas {
        let (status, result) = replica
            .request(
                Method::PATCH,
                Some("lost-settings-response"),
                Some(changed()),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["pending"], false);
        assert_eq!(result["reasoning_effort"], "medium");
    }
    assert_eq!(replicas[owner].runtime.inputs.lock().unwrap().len(), 1);
    assert!(replicas[peer].runtime.inputs.lock().unwrap().is_empty());
    assert_eq!(durable.get_agent(&saved.id).await.unwrap().unwrap(), saved);
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}
