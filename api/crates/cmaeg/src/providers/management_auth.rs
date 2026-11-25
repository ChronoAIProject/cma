use super::{ProviderClient, ProviderError};
use cmaeg_core::{channel::Secret, ids::ProviderSlug};
use url::Url;

pub(super) struct CmaManagementAuth {
    origin: url::Origin,
    token: crate::service_auth::ServiceAuth,
}
impl CmaManagementAuth {
    pub(super) fn new(origin: &str, token: Secret) -> Result<Self, ProviderError> {
        let url = Url::parse(origin)
            .map_err(|_| ProviderError::InvalidRequest("invalid CMA management origin".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ProviderError::InvalidRequest(
                "invalid CMA management origin".into(),
            ));
        }
        Ok(Self {
            origin: url.origin(),
            token: crate::service_auth::ServiceAuth::Static(token),
        })
    }
    fn permits(&self, provider: &ProviderSlug, url: &Url) -> bool {
        if provider.as_str() != "cma_codex"
            || url.origin() != self.origin
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return false;
        }
        let Some(path) = url.path().strip_prefix("/api/v1/cmaeg/bindings/") else {
            return false;
        };
        let parts = path.split('/').collect::<Vec<_>>();
        let id = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        };
        matches!(parts.as_slice(), [binding] if id(binding))
            || matches!(parts.as_slice(), [binding,"preparation-inspections" | "preparations"] if id(binding))
            || matches!(parts.as_slice(), [binding,"conversations"] if id(binding))
            || matches!(parts.as_slice(), [binding,"conversations",conversation] | [binding,"conversations",conversation,"profile-drift"] if id(binding) && id(conversation))
    }
    pub(super) fn with_service(mut self, token: crate::service_auth::ServiceAuth) -> Self {
        self.token = token;
        self
    }
    fn permits_reset(&self, provider: &ProviderSlug, url: &Url) -> bool {
        if provider.as_str() != "cma_codex"
            || url.origin() != self.origin
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return false;
        }
        let Some(path) = url.path().strip_prefix("/api/v1/cmaeg/bindings/") else {
            return false;
        };
        let parts = path.split('/').collect::<Vec<_>>();
        let id = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        };
        matches!(parts.as_slice(), [binding, "conversations", conversation, "resets" | "profile-transfers"] if id(binding) && id(conversation))
    }
    pub(super) fn permits_worker_classifier(&self, url: &Url) -> bool {
        url.origin() == self.origin
            && url.path() == super::worker_classifier::PATH
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none()
    }
    pub(super) async fn worker_classifier_bearer(
        &self,
        url: &Url,
    ) -> Result<Secret, ProviderError> {
        if !self.permits_worker_classifier(url) {
            return Err(ProviderError::InvalidRequest(
                "worker classifier requires exact CMA management route".into(),
            ));
        }
        self.token
            .bearer()
            .await
            .map_err(|error| ProviderError::Unavailable(error.to_string()))
    }
}
impl ProviderClient {
    pub(super) async fn reset_bearer(
        &self,
        provider: &ProviderSlug,
        url: &Url,
    ) -> Result<Secret, ProviderError> {
        // Reset authority cannot fall back to the worker whose mapping is being repaired.
        let auth = self
            .management_auth
            .as_ref()
            .filter(|auth| auth.permits_reset(provider, url))
            .ok_or_else(|| {
                ProviderError::InvalidRequest(
                    "conversation reset requires configured CMA management authority".into(),
                )
            })?;
        auth.token
            .bearer()
            .await
            .map_err(|error| ProviderError::Unavailable(error.to_string()))
    }

    pub(crate) fn with_service_auth(mut self, token: crate::service_auth::ServiceAuth) -> Self {
        if let Some(auth) = &mut self.management_auth {
            auth.token = token;
        }
        self
    }

    pub(super) fn permits_preparation(&self, provider: &ProviderSlug, url: &Url) -> bool {
        self.management_auth
            .as_ref()
            .is_some_and(|auth| auth.permits(provider, url))
    }

    /// Preparation must never silently downgrade to an agent key. Only the
    /// configured manager can inspect or reserve a profile-owned hierarchy.
    pub(super) async fn preparation_bearer(
        &self,
        provider: &ProviderSlug,
        url: &Url,
    ) -> Result<Secret, ProviderError> {
        let auth = self
            .management_auth
            .as_ref()
            .filter(|auth| auth.permits(provider, url))
            .ok_or_else(|| {
                ProviderError::InvalidRequest(
                    "binding preparation requires configured CMA management authority".into(),
                )
            })?;
        auth.token
            .bearer()
            .await
            .map_err(|error| ProviderError::Unavailable(error.to_string()))
    }

    pub(super) async fn management_bearer(
        &self,
        provider: &ProviderSlug,
        url: &Url,
        agent_key: &Secret,
    ) -> Result<Secret, ProviderError> {
        match &self.management_auth {
            Some(auth) if auth.permits(provider, url) => auth
                .token
                .bearer()
                .await
                .map_err(|error| ProviderError::Unavailable(error.to_string())),
            _ => Ok(agent_key.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_bearer_is_confined_to_exact_origin_provider_and_management_paths() {
        let auth = CmaManagementAuth::new(
            "https://cma.example",
            Secret::new(uuid::Uuid::new_v4().to_string()),
        )
        .unwrap();
        let provider = ProviderSlug::new("cma_codex").unwrap();
        for path in [
            "/api/v1/cmaeg/bindings/binding_one",
            "/api/v1/cmaeg/bindings/binding_one/conversations/conversation_one",
            "/api/v1/cmaeg/bindings/binding_one/preparation-inspections",
            "/api/v1/cmaeg/bindings/binding_one/preparations",
        ] {
            assert!(auth.permits(
                &provider,
                &Url::parse(&format!("https://cma.example{path}")).unwrap()
            ));
        }
        for url in [
            "http://cma.example/api/v1/cmaeg/bindings/binding_one",
            "https://cma.example:8443/api/v1/cmaeg/bindings/binding_one",
            "https://other.example/api/v1/cmaeg/bindings/binding_one",
            "https://cma.example/api/v2/responses",
            "https://cma.example/.well-known/agent-card.json",
            "https://cma.example/api/v1/cmaeg/bindings/binding_one/conversations/conversation_one/events/event_one",
            "https://cma.example/api/v1/cmaeg/bindings/binding_one?target=other",
        ] {
            assert!(!auth.permits(&provider, &Url::parse(url).unwrap()), "{url}");
        }
        assert!(!auth.permits(
            &ProviderSlug::new("external").unwrap(),
            &Url::parse("https://cma.example/api/v1/cmaeg/bindings/binding_one").unwrap()
        ));
    }
}

#[cfg(test)]
mod socket_tests {
    use super::*;
    use axum::{extract::State, http::HeaderMap, Json, Router};
    use cmaeg_core::ids::{BindingId, ConversationId};
    use serde_json::{json, Value};
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    async fn receive(
        State(values): State<Arc<Mutex<Vec<String>>>>,
        headers: HeaderMap,
    ) -> Json<Value> {
        values
            .lock()
            .unwrap()
            .push(headers["authorization"].to_str().unwrap().into());
        Json(json!({"binding_id":"binding_one","level":"l1","honoured":[],"refused":[]}))
    }
    #[tokio::test]
    async fn management_calls_send_service_bearer_only_to_cma_provider() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().fallback(receive).with_state(observed.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let entries=["cma_codex","external"].map(|slug|json!({"slug":slug,"base_url":origin,"kind":"responses_http","max_inflight":2,
            "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"}}));
        let registry =
            super::super::ProviderRegistry::parse(&serde_json::to_string(&entries).unwrap())
                .unwrap();
        let mut client =
            ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 2)
                .unwrap();
        let token = Secret::new(uuid::Uuid::new_v4().to_string());
        let agent = Secret::new(uuid::Uuid::new_v4().to_string());
        client.management_auth = Some(CmaManagementAuth::new(&origin, token.clone()).unwrap());
        let binding = BindingId::new("binding_one").unwrap();
        let conversation = ConversationId::new("conversation_one").unwrap();
        let profile=serde_json::from_value(json!({"schema_version":1,"metadata":{"name":"socket-test","owner":{"kind":"human","subject":"owner"},"provider":"cma_codex"},
            "model":{"access":"managed","service":"model-service","api_type":"responses","model":"test","thinking_effort":"medium"},"permission":{"action_mode":"guarded"},"context":{}})).unwrap();
        let request = super::super::BindingRequest {
            schema_version: 1,
            target: Default::default(),
            profile,
            profile_digest: format!("sha256:{}", "a".repeat(64)),
            agent_key: agent.clone(),
            tools: super::super::ToolsAdvertisement {
                native_mcp_url: "https://gateway.example/mcp".into(),
                nyxid_service_slug: "cmaeg".into(),
            },
        };
        for slug in ["cma_codex", "external"] {
            let provider = ProviderSlug::new(slug).unwrap();
            client.bind(&provider, &binding, &request).await.unwrap();
            client
                .ensure_conversation(&provider, &binding, &conversation, &agent)
                .await
                .unwrap();
            client
                .delete_conversation(&provider, &binding, &conversation, &agent)
                .await
                .unwrap();
            client.unbind(&provider, &binding, &agent).await.unwrap();
        }
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 8);
        assert!(observed[..4]
            .iter()
            .all(|value| *value == format!("Bearer {}", token.as_str())));
        assert!(observed[4..]
            .iter()
            .all(|value| *value == format!("Bearer {}", agent.as_str())));
        task.abort();
    }
}

#[cfg(test)]
mod reset_tests {
    use super::*;
    use crate::{
        config::ServiceAccountConfig, providers::ProviderRegistry, service_auth::ServiceAuth,
    };
    use std::time::Duration;

    #[test]
    fn reset_authority_rejects_every_route_escape() {
        let auth =
            CmaManagementAuth::new("https://cma.example", Secret::new("manager-fixture")).unwrap();
        let provider = ProviderSlug::new("cma_codex").unwrap();
        let route = "/api/v1/cmaeg/bindings/binding_one/conversations/conversation_one/resets";
        let url = Url::parse(&format!("https://cma.example{route}")).unwrap();
        assert!(auth.permits_reset(&provider, &url));
        assert!(!auth.permits(&provider, &url));
        assert!(!auth.permits_reset(&ProviderSlug::new("external").unwrap(), &url));
        for invalid in [
            format!("http://cma.example{route}"),
            format!("https://cma.example:8443{route}"),
            format!("https://other.example{route}"),
            format!("https://user:password@cma.example{route}"),
            format!("https://cma.example{route}?target=other"),
            format!("https://cma.example{route}#fragment"),
            format!("https://cma.example{route}/"),
            format!("https://cma.example{route}/extra"),
            format!("https://cma.example{}", route.replace("binding_one", "")),
            format!(
                "https://cma.example{}",
                route.replace("binding_one", &"b".repeat(129))
            ),
            format!(
                "https://cma.example{}",
                route.replace("conversation_one", &"c".repeat(129))
            ),
            format!(
                "https://cma.example{}",
                route.replace("binding_one", "%62inding_one")
            ),
            format!(
                "https://cma.example{}",
                route.replace("conversation_one", "a%2Fb")
            ),
            format!("https://cma.example{}", route.replace("/resets", "")),
            "https://cma.example/api/v2/responses".into(),
        ] {
            assert!(
                !auth.permits_reset(&provider, &Url::parse(&invalid).unwrap()),
                "{invalid}"
            );
        }
    }

    #[tokio::test]
    async fn reset_requires_a_current_manager_token_without_worker_fallback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new()
                    .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
            )
            .await
            .unwrap();
        });
        let mut client = ProviderClient::new(
            ProviderRegistry::parse("[]").unwrap(),
            Duration::from_secs(1),
            Duration::from_secs(1),
            1,
        )
        .unwrap();
        let provider = ProviderSlug::new("cma_codex").unwrap();
        let url = Url::parse(&format!(
            "{origin}/api/v1/cmaeg/bindings/b/conversations/c/resets"
        ))
        .unwrap();
        assert!(matches!(
            client.reset_bearer(&provider, &url).await,
            Err(ProviderError::InvalidRequest(_))
        ));
        client.management_auth =
            Some(CmaManagementAuth::new(&origin, Secret::new("manager-fixture")).unwrap());
        assert_eq!(
            client.reset_bearer(&provider, &url).await.unwrap().as_str(),
            "manager-fixture"
        );
        let auth = ServiceAuth::account(
            &origin,
            ServiceAccountConfig {
                client_id: "reset-test".into(),
                client_secret: Secret::new(uuid::Uuid::new_v4().to_string()),
            },
        )
        .unwrap();
        client = client.with_service_auth(auth);
        assert!(matches!(
            client.reset_bearer(&provider, &url).await,
            Err(ProviderError::Unavailable(_))
        ));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
}
