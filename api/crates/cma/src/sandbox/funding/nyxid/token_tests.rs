use super::*;
use crate::producer_token::oauth::tests::{issued, Server};
use serde_json::json;

fn provider(server: &Server) -> NyxIdFundingProvider {
    NyxIdFundingProvider {
        config: SandboxBillingConfig {
            base_url: server.url.clone(),
            credential_ref: String::new(),
            client_credentials: None,
            producer_id: uuid::Uuid::new_v4().to_string(),
        },
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        tokens: server.provider(),
    }
}

#[tokio::test]
async fn unauthorized_funding_refreshes_and_replays_identical_idempotent_bytes_once() {
    for path in [
        vec!["quotes"],
        vec!["windows"],
        vec!["windows", "w", "evidence"],
        vec!["receipts", "r"],
    ] {
        let server = Server::new(vec![
            issued("first", 3600),
            (401, "ignored".into()),
            issued("second", 3600),
            (200, "{}".into()),
        ])
        .await;
        let provider = provider(&server);
        let body = json!({"request_id": uuid::Uuid::new_v4().to_string(), "operation_id": uuid::Uuid::new_v4().to_string(), "intervals": []});
        let (method, body) = if path[0] == "receipts" {
            (reqwest::Method::GET, None)
        } else {
            (reqwest::Method::POST, Some(body))
        };
        provider.send(method, &path, body).await.unwrap();
        let calls = server.calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[1].0, calls[3].0);
        assert_eq!(calls[1].2, calls[3].2);
        assert_eq!(calls[1].1["authorization"], "Bearer first");
        assert_eq!(calls[3].1["authorization"], "Bearer second");
        assert_eq!(calls[3].1.get_all("authorization").iter().count(), 1);
    }
}

#[tokio::test]
async fn second_401_is_the_original_funding_error_without_another_exchange() {
    let server = Server::new(vec![
        issued("first", 3600),
        (401, "".into()),
        issued("second", 3600),
        (401, "".into()),
    ])
    .await;
    assert_eq!(
        provider(&server)
            .send(reqwest::Method::POST, &["windows"], Some(json!({})))
            .await,
        Err(FundingError::Invalid("producer HTTP 401".into()))
    );
    assert_eq!(server.calls.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn refresh_failure_does_not_replay_funding_or_change_static_mode() {
    let server = Server::new(vec![
        issued("first", 3600),
        (401, "".into()),
        (503, "".into()),
        (503, "".into()),
        (503, "".into()),
    ])
    .await;
    assert!(matches!(
        provider(&server)
            .send(reqwest::Method::POST, &["windows"], Some(json!({})))
            .await,
        Err(FundingError::Retryable(_))
    ));
    assert_eq!(
        server
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0.ends_with("windows"))
            .count(),
        1
    );
    let server = Server::new(vec![(401, "".into())]).await;
    let mut provider = provider(&server);
    provider.tokens = Arc::new(StaticTokenProvider("USER".into()));
    assert_eq!(
        provider
            .send(reqwest::Method::POST, &["quotes"], Some(json!({})))
            .await,
        Err(FundingError::Invalid("producer HTTP 401".into()))
    );
    assert_eq!(server.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn configuration_selects_oauth_and_resolves_the_referenced_environment() {
    let server = Server::new(vec![issued("configured", 3600), (404, "{}".into())]).await;
    let provider = NyxIdFundingProvider::new(SandboxBillingConfig {
        base_url: server.url.clone(),
        credential_ref: "MISSING_STATIC_REFERENCE".into(),
        client_credentials: Some(("USER".into(), "USER".into())),
        producer_id: uuid::Uuid::new_v4().to_string(),
    })
    .unwrap();
    assert_eq!(provider.recover("r").await.unwrap(), None);
    let calls = server.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "/oauth/token");
    assert_eq!(calls[1].1["authorization"], "Bearer configured");
    let form: std::collections::HashMap<_, _> = url::form_urlencoded::parse(calls[0].2.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(form["client_id"], std::env::var("USER").unwrap());
    assert_eq!(form["client_secret"], std::env::var("USER").unwrap());
}
