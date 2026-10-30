use super::*;
use crate::agent_models::ChronoAgentModels;
use layer1_public::rest::{AgentModelCatalog, AgentModelCatalogError};
use layer2_core::sandbox::{binding::ProvidedCredential, SandboxCredentialSource};

#[tokio::test]
async fn model_catalog_accepts_binding_and_provided_keys_without_local_expiry() {
    for binding in [false, true] {
        let mut sandbox = sandbox();
        let services = vec![ServiceId::new("chrono-llm-public")];
        sandbox.profile.content.nyxid_services = services.clone();
        sandbox.credential.reaches = services.clone();
        let key =
            ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
        let mut material = ManagedSandboxCredentials::provided_material(
            &sandbox.id,
            &sandbox.owner,
            &services,
            &key,
        )
        .unwrap();
        if binding {
            material.source = SandboxCredentialSource::Binding {
                binding_id: "binding-catalog".into(),
                tool_endpoint: None,
            };
        }
        sandbox.credential.source = material.source.clone();
        sandbox.credential.key_generation = 1;
        sandbox.credential.current = Some(material.authority.clone());
        sandbox.credential.delivered = true;
        sandbox.validate().unwrap();
        let custody = Arc::new(MemoryCustody::default());
        custody.insert(material).await.unwrap();
        let controller = ManagedSandboxCredentials::new(
            Arc::new(KeyIssuer::new(
                reqwest::Client::new(),
                "http://127.0.0.1:1",
                "test",
            )),
            Arc::new(Tokens::default()),
            custody.clone(),
        );
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let expected = key.expose().to_owned();
        let app = Router::new().route(
            "/api/v1/proxy/s/chrono-llm-public/models",
            get(move |headers: axum::http::HeaderMap| {
                let expected = expected.clone();
                let observed = observed.clone();
                async move {
                    assert_eq!(headers["authorization"], format!("Bearer {expected}"));
                    observed.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({"data":[{"id":"gpt-6"}]}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let catalog = ChronoAgentModels::new(&origin, controller).unwrap();
        assert_eq!(catalog.models(&sandbox).await.unwrap()[0].id, "gpt-6");
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        custody.corrupt.store(true, Ordering::SeqCst);
        assert_eq!(
            catalog.models(&sandbox).await,
            Err(AgentModelCatalogError::Unavailable)
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        custody.corrupt.store(false, Ordering::SeqCst);
        custody.delete(&sandbox.id, 1).await.unwrap();
        assert_eq!(
            catalog.models(&sandbox).await,
            Err(AgentModelCatalogError::Unavailable)
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn model_catalog_refuses_expired_managed_keys_before_requesting_the_provider() {
    let mut sandbox = sandbox();
    let services = vec![ServiceId::new("chrono-llm-public")];
    sandbox.profile.content.nyxid_services = services.clone();
    sandbox.credential.reaches = services.clone();
    let mut material = material(sandbox.owner.clone(), "expired-catalog-key");
    material.authority.expires_at = "2000-01-01T00:00:00Z".into();
    material.authority.grant_fingerprint =
        layer2_core::profiles::content_digest(&services).unwrap();
    sandbox.credential.key_generation = 1;
    sandbox.credential.current = Some(material.authority.clone());
    sandbox.credential.delivered = true;
    sandbox.validate().unwrap();
    let custody = Arc::new(MemoryCustody::default());
    custody.insert(material).await.unwrap();
    let controller = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(
            reqwest::Client::new(),
            "http://127.0.0.1:1",
            "test",
        )),
        Arc::new(Tokens::default()),
        custody,
    );
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let app = Router::new().route(
        "/api/v1/proxy/s/chrono-llm-public/models",
        get(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Json(serde_json::json!({"data":[{"id":"gpt-6"}]})) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let catalog = ChronoAgentModels::new(&origin, controller).unwrap();
    assert_eq!(
        catalog.models(&sandbox).await,
        Err(AgentModelCatalogError::Unavailable)
    );
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    server.abort();
}
