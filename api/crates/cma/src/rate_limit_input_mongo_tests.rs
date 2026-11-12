use super::*;
use crate::store::request_limits::{MongoRequestLimits, COLLECTION};
use mongodb::{
    bson::{doc, Document},
    Client, Database,
};

async fn database() -> Database {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    Client::with_uri_str(uri)
        .await
        .unwrap()
        .database(&format!("cma_input_tier_{}", uuid::Uuid::new_v4().simple()))
}
async fn replica(database: &Database, enabled: bool) -> Router {
    let n = |value| NonZeroU32::new(value).unwrap();
    let config = RateLimitConfig {
        admission_per_minute: n(1000),
        health_per_minute: n(120),
        default_per_minute: n(600),
        create_per_minute: n(10),
        input_per_minute: n(3),
        response_input_tier_enabled: enabled,
        anonymous_public: crate::config::AnonymousPublicConfig {
            per_minute: n(1_200),
            concurrency: n(16),
        },
    };
    router(RateLimiters::from_config(
        config,
        TrustedProxyConfig::sample(),
        Arc::new(MongoRequestLimits::open(database).await.unwrap()),
    ))
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn response_input_shared_budget_survives_alias_replica_and_restart_without_spending_creates()
{
    let database = database().await;
    let left = replica(&database, true).await;
    let right = replica(&database, true).await;
    for (app, path) in [
        (&left, "/api/v2/responses"),
        (&right, "/api/v2/agents/agt_one/responses"),
        (&left, "/api/v1/responses"),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, path, Some("owner"), false))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    // Neither process has spent its local burst: this is a shared refusal.
    refusal(
        right
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v1/agui/run",
                Some("owner"),
                false,
            ))
            .await
            .unwrap(),
    )
    .await;
    let restarted = replica(&database, true).await;
    refusal(
        restarted
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                Some("owner"),
                false,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        right
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                Some("another"),
                false
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    for n in 0..10 {
        let app = if n % 2 == 0 { &left } else { &right };
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    "/api/v1/conversations",
                    Some("owner"),
                    false
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
    }
    refusal(
        restarted
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v1/sandboxes",
                Some("owner"),
                false,
            ))
            .await
            .unwrap(),
    )
    .await;
    let rows = database.collection::<Document>(COLLECTION);
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 3); // two subjects/input + one subject/create
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn response_input_shared_disabled_policy_and_binding_creation_remain_under_create() {
    let database = database().await;
    let old_policy = replica(&database, false).await;
    for _ in 0..10 {
        assert_eq!(
            old_policy
                .clone()
                .oneshot(request(
                    Method::POST,
                    "/api/v2/responses",
                    Some("owner"),
                    false
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    let replacement = replica(&database, false).await;
    refusal(
        replacement
            .oneshot(request(
                Method::POST,
                "/api/v2/agents/agt_one/responses",
                Some("owner"),
                false,
            ))
            .await
            .unwrap(),
    )
    .await;
    let new_policy = replica(&database, true).await;
    refusal(
        new_policy
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                Some("owner"),
                true,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        new_policy
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                Some("owner"),
                false
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn response_input_unavailable_shared_store_never_falls_back_to_local_admission() {
    let database = database().await;
    let app = replica(&database, true).await;
    database.run_command(doc! {"collMod": COLLECTION, "validator": {"reject_all_test_writes": true}, "validationLevel": "strict"}).await.unwrap();
    for path in [
        "/api/v2/responses",
        "/api/v2/agents/agt_one/responses",
        "/api/v1/conversations",
    ] {
        let response = app
            .clone()
            .oneshot(request(Method::POST, path, Some("owner"), false))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
        assert_eq!(body["code"], "request_budget_unavailable");
    }
    assert_eq!(
        database
            .collection::<Document>(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    database.drop().await.unwrap();
}
