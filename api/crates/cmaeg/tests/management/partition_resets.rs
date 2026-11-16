use super::*;

#[tokio::test]
async fn owner_reset_is_read_only_at_gateway_and_replays_after_lost_provider_response() {
    let provider = FakeProvider::start(FakeProviderConfig {
        reset_lost_response: true,
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let channel = created(&client, &base).await;
    let id = channel["channel_id"].as_str().unwrap();
    let url = format!("{base}/v1/channels/{id}/partition/resets");
    let body = json!({"partition_key":"oracle","expected_version":channel["version"],"expected_agent_id":"agt_deleted"});
    let before = cma.records();
    let start = provider.log().len();
    for (token, status) in [("token-bob", 404), ("invalid", 401)] {
        assert_eq!(
            client
                .post(&url)
                .bearer_auth(token)
                .header("Idempotency-Key", "repair-one")
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            status
        );
    }
    let mut stale = body.clone();
    stale["expected_version"] = 999.into();
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "repair-one")
            .json(&stale)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(provider.log().len(), start);
    let send = || {
        client
            .post(&url)
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "repair-one")
            .json(&body)
            .send()
    };
    assert!(!send().await.unwrap().status().is_success());
    let response = send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let result: serde_json::Value = response.json().await.unwrap();
    assert_eq!(result["reset"]["incarnation"], 1);
    assert_eq!(result["reset"]["previous_agent_id"], "agt_deleted");
    let repeated: serde_json::Value = send().await.unwrap().json().await.unwrap();
    assert_eq!(result, repeated);
    let logs = provider.log();
    let attempts = &logs[start..];
    assert_eq!(attempts.len(), 3);
    assert!(attempts
        .iter()
        .all(|r| r.idempotency_key == attempts[0].idempotency_key && r.body == attempts[0].body));
    assert_eq!(attempts[0].idempotency_key.as_ref().unwrap().len(), 64);
    let mut changed = body.clone();
    changed["partition_key"] = "other".into();
    let conflict = client
        .post(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "repair-one")
        .json(&changed)
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
    assert_eq!(
        conflict.json::<serde_json::Value>().await.unwrap()["detail"],
        "partition_reset_key_conflict"
    );
    let after = cma.records();
    assert_eq!(after.len(), before.len());
    for (id, row) in before {
        let next = &after[&id];
        assert_eq!(
            (&row.owner, &row.blob, row.version),
            (&next.owner, &next.blob, next.version)
        );
    }
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn provider_reset_gate_and_eligibility_refusals_remain_named() {
    for code in [
        "partition_reset_disabled",
        "partition_reset_agent_not_deleted",
        "partition_reset_mapping_changed",
        "partition_reset_existing_agent",
        "partition_reset_affinity_unsupported",
    ] {
        let provider = FakeProvider::start(FakeProviderConfig {
            reset_refusal: Some(code.into()),
            ..Default::default()
        })
        .await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let (base, stop, task) = app(&provider, &cma, &nyxid).await;
        let client = Client::new();
        let channel = created(&client, &base).await;
        let response=client.post(format!("{base}/v1/channels/{}/partition/resets",channel["channel_id"].as_str().unwrap())).bearer_auth("token-alice").header("Idempotency-Key","repair").json(&json!({"partition_key":"oracle","expected_version":channel["version"],"expected_agent_id":"agt_deleted"})).send().await.unwrap();
        assert_eq!(response.status(), 409);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["detail"],
            code
        );
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn organization_reset_preserves_owner_kind_and_scopes_provider_key() {
    let provider = FakeProvider::start(Default::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    nyxid.oauth.orgs.lock().unwrap().insert(
        "org-reset".into(),
        (
            "Reset organization".into(),
            [("alice".into(), "admin".into())].into(),
        ),
    );
    let (token, _) = nyxid.oauth.issue_tokens("alice");
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let mut create = create_body();
    create["profile"]["metadata"]["owner"] = json!({"kind":"organization","subject":"org-reset"});
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth(&token)
        .header("X-CMAEG-Owner", "org-reset")
        .json(&create)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let channel: serde_json::Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{channel}");
    let url = format!(
        "{base}/v1/channels/{}/partition/resets",
        channel["channel_id"].as_str().unwrap()
    );
    let body = json!({"partition_key":"oracle","expected_version":channel["version"],"expected_agent_id":"agt_deleted"});
    let forbidden = client
        .post(&url)
        .bearer_auth(&token)
        .header("Idempotency-Key", "same-key")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 404);
    let response = client
        .post(&url)
        .bearer_auth(&token)
        .header("X-CMAEG-Owner", "org-reset")
        .header("Idempotency-Key", "same-key")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let last = provider.log().last().unwrap().clone();
    let forwarded: serde_json::Value = serde_json::from_str(&last.body).unwrap();
    assert_eq!(forwarded["owner"], "org-reset");
    assert_eq!(forwarded["organization"], true);
    stop.send(()).unwrap();
    task.await.unwrap();
}
