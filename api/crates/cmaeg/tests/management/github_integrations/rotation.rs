use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn secret_replacement_is_owner_scoped_write_only_and_replay_safe() {
    let mut fixture = Fixture::new(true).await;
    let second = fixture.replica().await;
    let first_secret = "synthetic-first-rotation-secret";
    let next_secret = "synthetic-next-rotation-secret";
    let app = fixture
        .register("token-alice", "rotate-app", Some(first_secret))
        .await;
    let app_ref = app["app_ref"].as_str().unwrap();
    let url = format!("{}/v1/github/apps/{app_ref}/webhook-secret", fixture.base);
    let request = json!({"expected_version":1,"webhook_secret":next_secret,"overlap_seconds":0});
    let denied = fixture
        .client
        .put(&url)
        .bearer_auth("token-bob")
        .header("Idempotency-Key", "rotate-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404);
    let changed = fixture
        .client
        .put(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "rotate-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status(), 200);
    let text = changed.text().await.unwrap();
    assert!(!text.contains(first_secret));
    assert!(!text.contains(next_secret));
    let metadata: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(metadata["version"], 2);
    assert_eq!(metadata["webhook_credential"]["version"], 2);
    let body = r#"{"zen":"Keep it simple","hook_id":9,"hook":{}}"#;
    let webhook = format!("{second}/v1/github/apps/{app_ref}/webhooks");
    assert_eq!(
        fixture
            .webhook(&webhook, first_secret, "after-rotation", "ping", body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        fixture
            .webhook(&webhook, next_secret, "after-rotation", "ping", body)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let replay = fixture
        .client
        .put(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "rotate-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), metadata);
    let conflict = fixture
        .client
        .put(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "rotate-secret")
        .json(&json!({"expected_version":1,"webhook_secret":first_secret,"overlap_seconds":0}))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
    assert_eq!(
        fixture
            .read(
                &second,
                &format!("/v1/github/apps/{app_ref}"),
                "token-alice"
            )
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap(),
        metadata
    );
    for index in 0..fixture.children.len() {
        let log =
            std::fs::read_to_string(fixture.files.dir.join(format!("github-{index}.log"))).unwrap();
        assert!(!log.contains(first_secret));
        assert!(!log.contains(next_secret));
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn secret_rotation_has_a_bounded_overlap_and_replay_cannot_restore_retired_keys() {
    let fixture = Fixture::new(true).await;
    let first = "synthetic-overlap-first-secret";
    let second = "synthetic-overlap-second-secret";
    let third = "synthetic-overlap-third-secret";
    let app = fixture
        .register("token-alice", "overlap-app", Some(first))
        .await;
    let app_ref = app["app_ref"].as_str().unwrap();
    let url = format!("{}/v1/github/apps/{app_ref}/webhook-secret", fixture.base);
    let request = json!({"expected_version":1,"webhook_secret":second});
    let rotated = fixture
        .client
        .put(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "overlap")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(rotated.status(), 200);
    let rotated: serde_json::Value = rotated.json().await.unwrap();
    assert_eq!(rotated["webhook_credential"]["previous"]["version"], 1);
    let remaining = rotated["webhook_credential"]["previous"]["valid_until"]
        .as_i64()
        .unwrap()
        - time::OffsetDateTime::now_utc().unix_timestamp();
    assert!((590..=600).contains(&remaining));
    let webhook = app["webhook_url"].as_str().unwrap();
    let body = r#"{"zen":"Keep it simple","hook_id":9,"hook":{}}"#;
    for secret in [first, second] {
        assert_eq!(
            fixture
                .webhook(webhook, secret, "overlap-ping", "ping", body)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let denied = fixture
        .client
        .put(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "nested-overlap")
        .json(&json!({"expected_version":2,"webhook_secret":third}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 409);
    let retired = fixture
        .client
        .put(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "retire-now")
        .json(&json!({"expected_version":2,"webhook_secret":third,"overlap_seconds":0}))
        .send()
        .await
        .unwrap();
    assert_eq!(retired.status(), 200);
    let replay = fixture
        .client
        .put(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "overlap")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), rotated);
    for secret in [first, second] {
        assert_eq!(
            fixture
                .webhook(webhook, secret, "overlap-ping", "ping", body)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    assert_eq!(
        fixture
            .webhook(webhook, third, "overlap-ping", "ping", body)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}
