use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::{CredentialError, CredentialMaterial};
use layer2_core::{SandboxId, ServiceId};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

fn sandbox() -> Sandbox {
    let mut value = crate::store::agent_access_fixtures::sandbox("models-cache");
    value.credential.reaches = vec![ServiceId("chrono-llm-public".into())];
    value.profile.content.nyxid_services = value.credential.reaches.clone();
    value
}
fn models() -> Vec<AgentModelView> {
    vec![AgentModelView {
        id: "gpt-6".into(),
        label: "Fixture".into(),
        available: false,
    }]
}

#[tokio::test]
async fn catalogue_cache_scopes_success_and_coalesces_concurrent_reads() {
    let cache = CatalogCache::new();
    let root = sandbox();
    let calls = AtomicUsize::new(0);
    let values = futures_util::future::join_all((0..200).map(|_| {
        cache.get(&root, "chrono-llm-public", || async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(models())
        })
    }))
    .await;
    assert!(values.iter().all(|value| value.as_ref() == Ok(&models())));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for field in 0..6 {
        let mut other = root.clone();
        match field {
            0 => other.id = SandboxId("another".into()),
            1 => other.created_at = "2026-09-06T00:00:00Z".into(),
            2 => other.owner = layer2_core::SubjectId("other".into()),
            3 => other.credential.current.as_mut().unwrap().generation += 1,
            4 => {
                other
                    .credential
                    .current
                    .as_mut()
                    .unwrap()
                    .provider_state_version += 1
            }
            _ => {}
        }
        let service = if field == 5 {
            "chrono-llm"
        } else {
            "chrono-llm-public"
        };
        cache
            .get(&other, service, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(models())
            })
            .await
            .unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 7);
}

#[tokio::test]
async fn failed_or_expired_catalogues_are_reloaded() {
    let cache = CatalogCache(entries(Duration::from_millis(1)));
    let root = sandbox();
    assert!(cache
        .get(&root, "public", || async {
            Err(AgentModelCatalogError::Unavailable)
        })
        .await
        .is_err());
    cache
        .get(&root, "public", || async { Ok(models()) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(cache
        .get(&root, "public", || async {
            Err(AgentModelCatalogError::Unavailable)
        })
        .await
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn concurrent_stalled_catalogues_share_the_original_wait_bound() {
    let cache = CatalogCache::new();
    let root = sandbox();
    let started = tokio::time::Instant::now();
    let values = futures_util::future::join_all((0..200).map(|_| {
        cache.get(&root, "public", || async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(models())
        })
    }))
    .await;
    assert!(values.into_iter().all(|value| value.is_err()));
    assert_eq!(started.elapsed(), Duration::from_secs(15));
}

struct Material {
    value: Mutex<Option<CredentialMaterial>>,
    calls: AtomicUsize,
    wait_until: Option<SystemTime>,
}
#[async_trait::async_trait]
impl SandboxCredentialController for Material {
    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        panic!("discovery issued a credential")
    }
    async fn material(
        &self,
        _: &SandboxId,
        _: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(remaining) = self
            .wait_until
            .and_then(|deadline| deadline.duration_since(SystemTime::now()).ok())
        {
            tokio::time::sleep(remaining + Duration::from_millis(10)).await;
        }
        Ok(self.value.lock().unwrap().clone())
    }
    async fn custodied(&self, _: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        panic!("discovery listed custody")
    }
    async fn forget(&self, _: &SandboxId, _: u64) -> Result<(), CredentialError> {
        panic!("discovery removed custody")
    }
    async fn revoke(&self, _: &Sandbox, _: &str) -> Result<(), CredentialError> {
        panic!("discovery revoked custody")
    }
}

#[tokio::test]
async fn catalogue_hits_still_refuse_missing_mismatched_expired_or_rotated_custody() {
    use axum::{routing::get, Json, Router};
    let root = sandbox();
    root.validate().unwrap();
    let valid = CredentialMaterial {
        source: root.credential.source.clone(),
        github_route: None,
        sandbox_id: root.id.clone(),
        owner: root.owner.clone(),
        authority: root.credential.current.clone().unwrap(),
        value: SecretValue::new("synthetic-model-fixture".into()),
    };
    let custody = Arc::new(Material {
        value: Mutex::new(Some(valid.clone())),
        calls: AtomicUsize::new(0),
        wait_until: None,
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let request_count = calls.clone();
    let delay_ms = Arc::new(AtomicUsize::new(0));
    let response_delay = delay_ms.clone();
    let app = Router::new().route(
        "/api/v1/proxy/s/chrono-llm-public/models",
        get(move || {
            request_count.fetch_add(1, Ordering::SeqCst);
            let delay = response_delay.load(Ordering::SeqCst);
            async move {
                tokio::time::sleep(Duration::from_millis(delay as u64)).await;
                Json(serde_json::json!({"data":[{"id":"gpt-6"}]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let catalog = ChronoAgentModels::new(&origin, custody.clone())
        .unwrap()
        .with_cache(true);
    for _ in 0..2 {
        assert!(catalog.models(&root).await.is_ok());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(custody.calls.load(Ordering::SeqCst), 2);
    for kind in 0..5 {
        let mut material = valid.clone();
        match kind {
            0 => material.owner = layer2_core::SubjectId("other".into()),
            1 => material.sandbox_id = SandboxId("other".into()),
            2 => material.authority.provider_state_version += 1,
            3 => material.value = SecretValue::new(String::new()),
            _ => {}
        }
        *custody.value.lock().unwrap() = (kind != 4).then_some(material);
        assert!(catalog.models(&root).await.is_err());
    }
    *custody.value.lock().unwrap() = Some(valid);
    let mut expired = root.clone();
    expired.credential.current.as_mut().unwrap().expires_at = "2026-01-01T00:00:00Z".into();
    assert!(catalog.models(&expired).await.is_err());
    let mut undelivered = root.clone();
    undelivered.credential.delivered = false;
    assert!(catalog.models(&undelivered).await.is_err());
    let mut rotated = root.clone();
    rotated.credential.key_generation += 1;
    rotated.credential.current.as_mut().unwrap().generation += 1;
    assert!(catalog.models(&rotated).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut expiring = root.clone();
    expiring.credential.current.as_mut().unwrap().expires_at =
        layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(2));
    custody.value.lock().unwrap().as_mut().unwrap().authority =
        expiring.credential.current.clone().unwrap();
    delay_ms.store(2000, Ordering::SeqCst);
    assert!(catalog.models(&expiring).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn catalogue_does_not_send_a_bearer_that_expires_during_custody_lookup() {
    use axum::{routing::get, Json, Router};
    use layer2_core::dependency_diagnostics::capture;

    let calls = Arc::new(AtomicUsize::new(0));
    let request_count = calls.clone();
    let app = Router::new().route(
        "/api/v1/proxy/s/chrono-llm-public/models",
        get(move || {
            request_count.fetch_add(1, Ordering::SeqCst);
            async { Json(serde_json::json!({"data":[{"id":"gpt-6"}]})) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for cached in [false, true] {
        let mut root = sandbox();
        let authority = root.credential.current.as_mut().unwrap();
        authority.expires_at = layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(2));
        let expiry = SystemTime::UNIX_EPOCH
            + Duration::from_secs(seconds_from_rfc3339(&authority.expires_at).unwrap());
        let custody = Arc::new(Material {
            value: Mutex::new(Some(CredentialMaterial {
                source: root.credential.source.clone(),
                github_route: None,
                sandbox_id: root.id.clone(),
                owner: root.owner.clone(),
                authority: authority.clone(),
                value: SecretValue::new("synthetic-model-fixture".into()),
            })),
            calls: AtomicUsize::new(0),
            wait_until: Some(expiry),
        });
        let catalog = ChronoAgentModels::new(&origin, custody.clone())
            .unwrap()
            .with_cache(cached);
        let (result, failure) = capture(catalog.models(&root)).await;
        assert_eq!(custody.calls.load(Ordering::SeqCst), 1);
        assert_eq!(result, Err(AgentModelCatalogError::Unavailable));
        assert_eq!(
            failure,
            Some(Failure::new(
                Dependency::AuthenticationAuthority,
                Reason::CredentialExpired,
            )),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "cache enabled: {cached}");
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
