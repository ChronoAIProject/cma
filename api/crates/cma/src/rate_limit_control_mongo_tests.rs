use super::*;
use crate::store::request_limits::{MongoRequestLimits, COLLECTION};
use mongodb::{
    bson::{doc, Document},
    Client, Database,
};

async fn database() -> Database {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    Client::with_uri_str(uri).await.unwrap().database(&format!(
        "cma_control_tier_{}",
        uuid::Uuid::new_v4().simple()
    ))
}

async fn replica(database: &Database, enabled: bool, control: u32) -> Router {
    replica_with_policy(database, enabled, control, true).await
}

async fn replica_with_policy(
    database: &Database,
    enabled: bool,
    control: u32,
    controls: bool,
) -> Router {
    let n = |value| NonZeroU32::new(value).unwrap();
    let config = RateLimitConfig {
        default_lease_enabled: false,
        admission_per_minute: n(1000),
        health_per_minute: n(120),
        default_per_minute: n(600),
        create_per_minute: n(10),
        input_per_minute: n(120),
        control_tier_enabled: controls,
        control_per_minute: n(control),
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
async fn disabled_control_policy_keeps_shared_create_keys_and_never_writes_control_keys() {
    let database = database().await;
    let app = replica_with_policy(&database, false, 120, false).await;
    for (_, path) in CONTROL_ROUTES.iter().take(10) {
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, path, "owner"))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    refusal(
        app.oneshot(request(Method::POST, CONTROL_ROUTES[10].1, "owner"))
            .await
            .unwrap(),
    )
    .await;
    let rows = database.collection::<Document>(COLLECTION);
    assert_eq!(
        rows.count_documents(doc! {"_id": {"$regex": "^control:"}})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        rows.count_documents(doc! {"_id": {"$regex": "^create:"}})
            .await
            .unwrap(),
        1
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn control_shared_budget_survives_alias_replica_restart_and_subject_isolation() {
    let database = database().await;
    let left = replica(&database, false, 3).await;
    let right = replica(&database, false, 3).await;
    for (n, (_, path)) in CONTROL_ROUTES.iter().take(3).enumerate() {
        let app = if n % 2 == 0 { &left } else { &right };
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, path, "owner"))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    // Both processes still have local allowance: only the shared budget can refuse.
    refusal(
        right
            .clone()
            .oneshot(request(Method::POST, CONTROL_ROUTES[3].1, "owner"))
            .await
            .unwrap(),
    )
    .await;
    let restarted = replica(&database, false, 3).await;
    refusal(
        restarted
            .clone()
            .oneshot(request(Method::POST, CONTROL_ROUTES[4].1, "owner"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        right
            .clone()
            .oneshot(request(Method::POST, CONTROL_ROUTES[0].1, "another"))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    for n in 0..10 {
        let app = if n % 2 == 0 { &left } else { &right };
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, "/api/v1/sandboxes", "owner"))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
    }
    refusal(
        restarted
            .oneshot(request(Method::POST, "/api/v1/sandboxes", "owner"))
            .await
            .unwrap(),
    )
    .await;
    let rows = database.collection::<Document>(COLLECTION);
    assert_eq!(
        rows.count_documents(doc! {"_id": {"$regex": "^control:"}})
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        rows.count_documents(doc! {"_id": {"$regex": "^create:"}})
            .await
            .unwrap(),
        1
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn ten_shared_creates_leave_twenty_controls_and_inputs_available() {
    for enabled in [false, true] {
        let database = database().await;
        let left = replica(&database, enabled, 120).await;
        let right = replica(&database, enabled, 120).await;
        for n in 0..10 {
            let app = if n % 2 == 0 { &left } else { &right };
            assert_eq!(
                app.clone()
                    .oneshot(request(Method::POST, "/api/v1/sandboxes", "owner"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CREATED
            );
        }
        refusal(
            right
                .clone()
                .oneshot(request(Method::POST, "/api/v1/sandboxes", "owner"))
                .await
                .unwrap(),
        )
        .await;
        let mut latencies = Vec::new();
        for n in 0..20 {
            let app = if n % 2 == 0 { &left } else { &right };
            let started = Instant::now();
            assert_eq!(
                app.clone()
                    .oneshot(request(
                        Method::POST,
                        CONTROL_ROUTES[n % CONTROL_ROUTES.len()].1,
                        "owner"
                    ))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
            latencies.push(started.elapsed().as_micros());
            let input = if enabled {
                "/api/v2/responses"
            } else {
                "/api/v1/agents/agt_one/input"
            };
            assert_eq!(
                app.clone()
                    .oneshot(request(Method::POST, input, "owner"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
        }
        latencies.sort_unstable();
        eprintln!("local Mongo TC-076 rate middleware: input_flag={enabled}, controls=20/20, inputs=20/20, control_p99_us={}", latencies[19]);
        for owner in ["another", "third"] {
            assert_eq!(
                left.clone()
                    .oneshot(request(Method::POST, "/api/v1/sandboxes", owner))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CREATED
            );
            assert_eq!(
                right
                    .clone()
                    .oneshot(request(Method::POST, CONTROL_ROUTES[0].1, owner))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
        }
        let rows = database.collection::<Document>(COLLECTION);
        assert_eq!(
            rows.count_documents(doc! {"_id": {"$regex": "^control:"}})
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            rows.count_documents(doc! {"_id": {"$regex": "^input:"}})
                .await
                .unwrap(),
            1
        );
        database.drop().await.unwrap();
    }
}

async fn reject_shared_writes(database: &Database) {
    database.run_command(doc! {"collMod": COLLECTION, "validator": {"reject_all_test_writes": true}, "validationLevel": "strict"}).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn control_unavailable_shared_store_never_falls_back_to_local_allowance() {
    let database = database().await;
    let app = replica(&database, false, 120).await;
    reject_shared_writes(&database).await;
    for (_, path) in CONTROL_ROUTES {
        let response = app
            .clone()
            .oneshot(request(Method::POST, path, "owner"))
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn local_control_excess_is_refused_before_shared_writes() {
    let database = database().await;
    let app = replica(&database, false, 3).await;
    for (_, path) in CONTROL_ROUTES.iter().take(3) {
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, path, "owner"))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    // Refill only Mongo so a database check would attempt a now-invalid write.
    // Keeping both budgets exhausted would return 429 through either path.
    database
        .collection::<Document>(COLLECTION)
        .update_one(doc! {}, doc! {"$set": {"tat": 0_i64}})
        .await
        .unwrap();
    reject_shared_writes(&database).await;
    // Reaching Mongo would yield 503 under the validator; the local tier returns 429.
    refusal(
        app.oneshot(request(Method::POST, CONTROL_ROUTES[3].1, "owner"))
            .await
            .unwrap(),
    )
    .await;
    database.drop().await.unwrap();
}
