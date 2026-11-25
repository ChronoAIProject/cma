use std::time::Duration;

use reqwest::{Client, StatusCode};
use tempfile::TempDir;
use tokio::net::TcpListener;
use url::Url;

use crate::{ledger::Ledger, model::Check, runner::serve_on};

use super::config;

#[tokio::test]
async fn offset_mismatch_leaves_health_live_and_readiness_unavailable() {
    let dir = TempDir::new().unwrap();
    let config = config(&Url::parse("http://127.0.0.1/").unwrap(), dir.path());
    let ledger = Ledger::new(&config);
    ledger.init().unwrap();
    let mut persisted = config.jobs();
    let (job, _) = persisted
        .iter_mut()
        .find(|(job, _)| job.check == Check::Agent)
        .unwrap();
    job.offset += 1;
    ledger.configure(&persisted).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(serve_on(listener, config, ledger.clone()));
    let client = Client::new();
    let health = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let ready = client.get(format!("{base}/readyz")).send().await.unwrap();
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(ledger.available().is_ok());
    assert!(ledger
        .metrics()
        .unwrap()
        .jobs
        .iter()
        .all(|job| job.times[0].is_none()));
    server.abort();
}
