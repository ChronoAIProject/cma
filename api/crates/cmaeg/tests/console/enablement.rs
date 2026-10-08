//! The console fails soft: a missing client or an unproven NyxID answers a
//! generic 503 page while readiness and the rest of the gateway are unaffected.
use super::support::{config, Harness};
use cmaeg::console::DiscoveryError;
use std::sync::atomic::Ordering;

fn gauge(metrics: &str, name: &str) -> Option<f64> {
    metrics
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .and_then(|value| value.trim().parse().ok())
}

async fn assert_soft_disabled(harness: &Harness) {
    let console = harness.get("/console").await;
    assert_eq!(console.status(), 503);
    let page = console.text().await.unwrap();
    assert!(page.contains("Console unavailable"), "{page}");
    assert!(page.contains("<meta name=\"robots\" content=\"noindex\">"));
    assert_eq!(harness.get("/console/anything/else").await.status(), 503);
    assert_eq!(harness.get("/readyz").await.status(), 200);
    assert_eq!(harness.get("/healthz").await.status(), 200);
}

#[tokio::test]
async fn a_missing_client_id_or_secret_answers_503_and_starts_no_discovery() {
    let metrics = cmaeg::telemetry::install().unwrap();
    for missing in [
        cmaeg::ConsoleConfig {
            client_id: String::new(),
            ..config()
        },
        cmaeg::ConsoleConfig {
            client_secret: None,
            ..config()
        },
    ] {
        let harness = Harness::start(missing).await;
        assert!(harness.console.spawn_discovery().is_none());
        // Even a reachable, valid NyxID cannot enable an incomplete client.
        harness.console.discover_once().await.unwrap();
        assert!(!harness.console.is_enabled());
        assert_soft_disabled(&harness).await;
        assert_eq!(gauge(&metrics.render(), "cmaeg_console_enabled"), Some(0.0));
        harness.close().await;
    }
}

#[tokio::test]
async fn discovery_enables_the_console_and_sets_its_gauge() {
    let metrics = cmaeg::telemetry::install().unwrap();
    let harness = Harness::start(config()).await;
    assert_soft_disabled(&harness).await;
    assert_eq!(gauge(&metrics.render(), "cmaeg_console_enabled"), Some(0.0));
    harness.console.discover_once().await.unwrap();
    assert!(harness.console.is_enabled());
    assert_eq!(gauge(&metrics.render(), "cmaeg_console_enabled"), Some(1.0));
    assert_ne!(harness.get("/console").await.status(), 503);
    harness.close().await;
}

#[tokio::test]
async fn an_unreachable_nyxid_keeps_last_good_endpoints_and_a_mismatch_withdraws_them() {
    let harness = Harness::start(config()).await;
    harness
        .nyxid
        .oauth
        .discovery_status
        .store(503, Ordering::SeqCst);
    assert_eq!(
        harness.console.discover_once().await,
        Err(DiscoveryError::Unavailable)
    );
    assert_soft_disabled(&harness).await;
    harness
        .nyxid
        .oauth
        .discovery_status
        .store(200, Ordering::SeqCst);
    harness.console.discover_once().await.unwrap();
    harness
        .nyxid
        .oauth
        .discovery_status
        .store(500, Ordering::SeqCst);
    assert_eq!(
        harness.console.discover_once().await,
        Err(DiscoveryError::Unavailable)
    );
    assert!(harness.console.is_enabled());
    harness
        .nyxid
        .oauth
        .discovery_status
        .store(200, Ordering::SeqCst);
    *harness.nyxid.oauth.discovery.lock().unwrap() = Some(serde_json::json!({
        "authorization_endpoint": "https://elsewhere.example/oauth/authorize",
        "token_endpoint": format!("{}/oauth/token", harness.nyxid.base_url),
        "revocation_endpoint": format!("{}/oauth/revoke", harness.nyxid.base_url),
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"]
    }));
    assert_eq!(
        harness.console.discover_once().await,
        Err(DiscoveryError::Mismatch("authorization_endpoint"))
    );
    assert_soft_disabled(&harness).await;
    harness.close().await;
}
