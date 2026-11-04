//! Exact service-authenticated digest-only Manager classification transport.
use super::management_auth::CmaManagementAuth;
use crate::{
    auth::owner_keys::WorkerClassifier, records::RecordError, worker_credentials::canonical_digest,
};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::time::Duration;

pub const PATH: &str = "/api/v1/cmaeg/worker-credentials/classify";

pub struct WorkerClassifierClient {
    http: reqwest::Client,
    auth: CmaManagementAuth,
    endpoint: url::Url,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    classification: Classification,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Classification {
    KnownWorker,
    Unknown,
}

impl WorkerClassifierClient {
    pub(crate) fn new(
        config: &crate::Config,
        service: crate::service_auth::ServiceAuth,
    ) -> Result<Self, RecordError> {
        let auth = CmaManagementAuth::new(
            &config.management.cma_base_url,
            config.management.cma_service_token.clone(),
        )
        .map_err(|_| RecordError::Rejected("invalid worker classifier authority".into()))?
        .with_service(service);
        let endpoint = url::Url::parse(&config.management.cma_base_url)
            .and_then(|url| url.join(PATH))
            .map_err(|_| RecordError::Rejected("invalid worker classifier endpoint".into()))?;
        if !auth.permits_worker_classifier(&endpoint) {
            return Err(RecordError::Rejected(
                "worker classifier endpoint is not allowed".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(4))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| RecordError::Unavailable("worker classifier client unavailable".into()))?;
        Ok(Self {
            http,
            auth,
            endpoint,
        })
    }
}

impl WorkerClassifier for WorkerClassifierClient {
    fn was_worker<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<bool, RecordError>> {
        Box::pin(async move {
            let sha256 = canonical_digest(sha256)?;
            let unavailable =
                || RecordError::Unavailable("CMA worker classification unavailable".into());
            let bearer = self
                .auth
                .worker_classifier_bearer(&self.endpoint)
                .await
                .map_err(|_| unavailable())?;
            let mut response = self
                .http
                .post(self.endpoint.clone())
                .bearer_auth(bearer.as_str())
                .json(&serde_json::json!({"sha256":sha256}))
                .send()
                .await
                .map_err(|_| unavailable())?;
            if !response.status().is_success() {
                return Err(unavailable());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                if bytes.len() + chunk.len() > 16_384 {
                    return Err(unavailable());
                }
                bytes.extend_from_slice(&chunk);
            }
            let evidence: Evidence = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
            Ok(matches!(
                evidence.classification,
                Classification::KnownWorker
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
        Json, Router,
    };
    use cmaeg_core::channel::Secret;
    use std::sync::{Arc, Mutex};

    #[test]
    fn classifier_service_bearer_is_confined_to_one_exact_origin_and_path() {
        let auth =
            CmaManagementAuth::new("https://manager.example", Secret::new("service-fixture"))
                .unwrap();
        assert!(auth.permits_worker_classifier(
            &url::Url::parse(&format!("https://manager.example{PATH}")).unwrap()
        ));
        for raw in [
            format!("https://other.example{PATH}"),
            format!("http://manager.example{PATH}"),
            format!("https://manager.example{PATH}/child"),
            format!("https://manager.example{PATH}?sha256=anything"),
            format!("https://manager.example{PATH}#fragment"),
            "https://manager.example/api/v1/agents".into(),
        ] {
            assert!(
                !auth.permits_worker_classifier(&url::Url::parse(&raw).unwrap()),
                "{raw}"
            );
        }
    }

    #[tokio::test]
    async fn only_explicit_classification_is_accepted_and_transport_failures_never_mean_unknown() {
        type Reply = Arc<Mutex<(StatusCode, serde_json::Value, usize)>>;
        let reply: Reply = Arc::new(Mutex::new((
            StatusCode::OK,
            serde_json::json!({"classification":"unknown"}),
            0,
        )));
        let app = Router::new()
            .route(
                PATH,
                post(
                    |State(reply): State<Reply>,
                     headers: HeaderMap,
                     Json(body): Json<serde_json::Value>| async move {
                        assert_eq!(headers["authorization"], "Bearer service-fixture");
                        assert_eq!(body, serde_json::json!({"sha256":"a".repeat(64)}));
                        let mut reply = reply.lock().unwrap();
                        reply.2 += 1;
                        (reply.0, Json(reply.1.clone())).into_response()
                    },
                ),
            )
            .with_state(reply.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = WorkerClassifierClient {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(1))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            auth: CmaManagementAuth::new(&origin, Secret::new("service-fixture")).unwrap(),
            endpoint: url::Url::parse(&format!("{origin}{PATH}")).unwrap(),
        };
        assert!(!client.was_worker(&"a".repeat(64)).await.unwrap());
        reply.lock().unwrap().1 = serde_json::json!({"classification":"known_worker"});
        assert!(client
            .was_worker(&format!("sha256:{}", "a".repeat(64)))
            .await
            .unwrap());
        for body in [
            serde_json::json!({}),
            serde_json::json!({"classification":"unrecognized"}),
            serde_json::json!({"classification":"unknown","extra":true}),
        ] {
            reply.lock().unwrap().1 = body;
            assert!(client.was_worker(&"a".repeat(64)).await.is_err());
        }
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            {
                let mut value = reply.lock().unwrap();
                value.0 = status;
                value.1 = serde_json::json!({"classification":"unknown"});
            }
            assert!(client.was_worker(&"a".repeat(64)).await.is_err());
        }
        let count = reply.lock().unwrap().2;
        assert!(client.was_worker(&"A".repeat(64)).await.is_err());
        assert_eq!(reply.lock().unwrap().2, count);
        server.abort();
    }
}
