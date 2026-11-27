use super::*;
use axum::extract::ConnectInfo;
use axum::{body::Body, middleware, routing::get, Router};
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::net::SocketAddr;
use std::{
    num::NonZeroU32,
    sync::{atomic::AtomicUsize, Mutex},
    time::Duration,
};
use tokio::sync::Notify;
use tower::ServiceExt;

use crate::store::request_limits::{MongoRequestLimits, COLLECTION};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn injected_policy_precedes_auth_and_mongo_and_is_inert_when_disabled() {
    for enabled in [false, true] {
        let uri =
            std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
        let mut options = ClientOptions::parse(uri).await.unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let captured = commands.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                captured.lock().unwrap().push(event.command_name);
            }
        }));
        let database = Client::with_options(options).unwrap().database(&format!(
            "cma_anonymous_budget_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let shared = Arc::new(MongoRequestLimits::open(&database).await.unwrap());
        commands.lock().unwrap().clear();
        let n = |value| NonZeroU32::new(value).unwrap();
        let config = RateLimitConfig {
            admission_per_minute: n(100),
            health_per_minute: n(120),
            default_per_minute: n(100),
            create_per_minute: n(10),
            input_per_minute: n(120),
            control_per_minute: n(120),
            response_input_tier_enabled: false,
            control_tier_enabled: true,
            default_lease_enabled: false,
            anonymous_public: crate::config::AnonymousPublicConfig {
                enabled,
                per_minute: n(1),
                concurrency: n(1),
            },
        };
        let limits = RateLimiters::from_config(config, TrustedProxyConfig::sample(), shared);
        assert_eq!(limits.anonymous_public.is_some(), enabled);
        assert!(
            commands.lock().unwrap().is_empty(),
            "policy construction performs no database work"
        );
        let auth_calls = Arc::new(AtomicUsize::new(0));
        let downstream_auth = auth_calls.clone();
        let handler_calls = Arc::new(AtomicUsize::new(0));
        let downstream_handler = handler_calls.clone();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler_started = started.clone();
        let handler_release = release.clone();
        let app = Router::new()
            .route(
                "/api/v1/triggers/public/pub_one",
                get(move || {
                    let started = handler_started.clone();
                    let release = handler_release.clone();
                    downstream_handler.fetch_add(1, Ordering::SeqCst);
                    async move {
                        started.notify_one();
                        release.notified().await;
                        axum::http::StatusCode::NO_CONTENT
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(limits.clone(), enforce))
            .layer(middleware::from_fn(move |request: Request, next: Next| {
                downstream_auth.fetch_add(1, Ordering::SeqCst);
                async move { next.run(request).await }
            }))
            .layer(middleware::from_fn_with_state(limits, admit));
        let request = |peer| {
            let mut request = Request::get("/api/v1/triggers/public/pub_one")
                .header(header::COOKIE, "visitor=synthetic")
                .body(Body::empty())
                .unwrap();
            request
                .extensions_mut()
                .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, peer], 443))));
            request
        };
        let first = tokio::spawn(app.clone().oneshot(request(1)));
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        let second = tokio::spawn(app.clone().oneshot(request(2)));
        let (expected, second) = if enabled {
            let response = tokio::time::timeout(Duration::from_secs(1), second)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(response.status(), 429);
            assert!(response.headers().contains_key(header::RETRY_AFTER));
            (1, None)
        } else {
            tokio::time::timeout(Duration::from_secs(5), started.notified())
                .await
                .unwrap();
            (2, Some(second))
        };
        release.notify_one();
        assert_eq!(first.await.unwrap().unwrap().status(), 204);
        if let Some(second) = second {
            release.notify_one();
            assert_eq!(second.await.unwrap().unwrap().status(), 204);
        }
        if enabled {
            let response = app.oneshot(request(3)).await.unwrap();
            assert_eq!(
                response.status(),
                429,
                "released concurrency still shares the rate budget"
            );
        }
        assert_eq!(auth_calls.load(Ordering::SeqCst), expected);
        assert_eq!(handler_calls.load(Ordering::SeqCst), expected);
        let observed = commands.lock().unwrap().clone();
        assert_eq!(
            observed
                .iter()
                .filter(|name| *name == "findAndModify")
                .count(),
            expected
        );
        assert!(
            observed.iter().all(|name| name == "findAndModify"),
            "unexpected policy I/O: {observed:?}"
        );
        assert_eq!(
            database
                .collection::<Document>(COLLECTION)
                .count_documents(doc! {})
                .await
                .unwrap(),
            expected as u64
        );
        database.drop().await.unwrap();
    }
}
