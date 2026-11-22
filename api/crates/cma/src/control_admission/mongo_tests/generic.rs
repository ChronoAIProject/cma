use super::*;
use crate::{config::RateLimitConfig, rate_limit::RateLimiters};
use axum::body::Body;
use mongodb::bson::Document;
use tower::ServiceExt;

fn limits() -> RateLimitConfig {
    let n = |value| std::num::NonZeroU32::new(value).unwrap();
    RateLimitConfig {
        admission_per_minute: n(6000),
        health_per_minute: n(120),
        default_per_minute: n(1),
        create_per_minute: n(1),
        input_per_minute: n(120),
        control_per_minute: n(120),
        control_tier_enabled: false,
        response_input_tier_enabled: false,
        anonymous_public: crate::config::AnonymousPublicConfig {
            enabled: false,
            per_minute: n(1200),
            concurrency: n(16),
        },
        default_lease_enabled: false,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn flag_off_preserves_generic_quota_and_enabled_stop_survives_create_exhaustion() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut observations = vec![];
    for enabled in [None, Some(false), Some(true)] {
        let writes = Arc::new(AtomicUsize::new(0));
        let captured = writes.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let mongodb::event::command::CommandEvent::Started(event) = event {
                if event.command.get_str("findAndModify").ok()
                    == Some(crate::store::request_limits::COLLECTION)
                {
                    captured.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        let database = Client::with_options(options).unwrap().database(&format!(
            "cma_control_generic_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = Arc::new(
            crate::store::request_limits::MongoRequestLimits::open(&database)
                .await
                .unwrap(),
        );
        let limiter = RateLimiters::from_config(
            limits(),
            crate::config::TrustedProxyConfig {
                cidrs: vec![],
                forwarded_hops: 1,
                cloudflare_cidrs: vec![],
            },
            store,
        );
        if let Some(enabled) = enabled {
            limiter.install_lifecycle(ControlAdmissionConfig {
                enabled,
                ..Default::default()
            });
        }
        let router = Router::new()
            .route("/api/v1/agents", post(|| async { StatusCode::ACCEPTED }))
            .route(
                "/api/v1/agents/{id}",
                delete(|| async { StatusCode::ACCEPTED }).get(|| async { StatusCode::OK }),
            )
            .route(
                "/api/v1/agents/{id}/stop",
                post(|| async { StatusCode::ACCEPTED }),
            )
            .layer(middleware::from_fn_with_state(
                limiter.clone(),
                crate::rate_limit::enforce,
            ));
        let router = routes(router, limiter.lifecycle()).layer(middleware::from_fn(authenticated));
        let mut statuses = vec![];
        for (method, path) in [
            (Method::POST, "/api/v1/agents"),
            (Method::POST, "/api/v1/agents"),
            (Method::GET, "/api/v1/agents/a"),
            (Method::POST, "/api/v1/agents/a/stop"),
            (Method::DELETE, "/api/v1/agents/a"),
            (Method::POST, "/api/v1/agents/a/stop"),
            (Method::DELETE, "/api/v1/agents/a"),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header("x-fixture-subject", "one")
                .header("x-cma-agent-control-admitted", "forged")
                .body(Body::empty())
                .unwrap();
            statuses.push(
                router
                    .clone()
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status()
                    .as_u16(),
            );
        }
        let collection = database.collection::<Document>(crate::store::request_limits::COLLECTION);
        let lifecycle_keys = collection
            .count_documents(doc! {"_id":{"$regex":"^agent-control-v1:"}})
            .await
            .unwrap();
        let observation = (statuses, writes.load(Ordering::SeqCst), lifecycle_keys);
        if enabled == Some(true) {
            assert_eq!(observation, (vec![202, 429, 200, 202, 202, 429, 429], 4, 2));
        } else {
            assert_eq!(observation, (vec![202, 429, 200, 429, 429, 429, 429], 2, 0));
        }
        observations.push(observation);
        database.drop().await.unwrap();
    }
    assert_eq!(
        observations[0], observations[1],
        "flag-off status and quota-write parity"
    );
}
