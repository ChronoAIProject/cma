use super::*;
use crate::agent_models::ChronoAgentModels;
use layer1_public::rest::{AgentModelCatalog, AgentModelCatalogError};
use layer2_core::dependency_diagnostics::{capture, Dependency, Failure, Reason};
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
        let (models, failure) = capture(catalog.models(&sandbox)).await;
        assert_eq!(models.unwrap()[0].id, "gpt-6");
        assert_eq!(failure, None);
        assert_eq!(requests.load(Ordering::SeqCst), 1);

        custody.corrupt.store(true, Ordering::SeqCst);
        let (result, failure) = capture(catalog.models(&sandbox)).await;
        assert_eq!(result, Err(AgentModelCatalogError::Unavailable));
        assert_eq!(
            failure,
            Some(Failure::new(Dependency::Storage, Reason::Corrupt))
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        custody.corrupt.store(false, Ordering::SeqCst);
        custody.delete(&sandbox.id, 1).await.unwrap();
        let (result, failure) = capture(catalog.models(&sandbox)).await;
        assert_eq!(result, Err(AgentModelCatalogError::Unavailable));
        assert_eq!(
            failure,
            Some(Failure::new(Dependency::Storage, Reason::CredentialMissing))
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
    let (result, failure) = capture(catalog.models(&sandbox)).await;
    assert_eq!(result, Err(AgentModelCatalogError::Unavailable));
    assert_eq!(
        failure,
        Some(Failure::new(
            Dependency::AuthenticationAuthority,
            Reason::CredentialExpired
        ))
    );
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn model_catalog_classifies_local_custody_guards_before_contacting_gateway() {
    let custody = Arc::new(MemoryCustody::default());
    let controller = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(
            reqwest::Client::new(),
            "http://127.0.0.1:1",
            "test",
        )),
        Arc::new(Tokens::default()),
        custody.clone(),
    );
    let catalog = ChronoAgentModels::new("http://127.0.0.1:1", controller).unwrap();
    let mut valid = sandbox();
    let services = vec![ServiceId::new("chrono-llm-public")];
    valid.profile.content.nyxid_services = services.clone();
    valid.credential.reaches = services.clone();
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let original =
        ManagedSandboxCredentials::provided_material(&valid.id, &valid.owner, &services, &key)
            .unwrap();
    valid.credential.source = original.source.clone();
    valid.credential.current = Some(original.authority.clone());
    valid.credential.key_generation = 1;
    valid.credential.delivered = true;
    valid.validate().unwrap();
    custody.insert(original.clone()).await.unwrap();
    let mut corrupt = valid.clone();
    corrupt.revision = 0;
    assert!(corrupt.validate().is_err());
    let mut missing = valid.clone();
    missing.credential.delivered = false;
    missing.credential.current = None;
    missing.validate().unwrap();
    let mut expiry = valid.clone();
    expiry.credential.source = SandboxCredentialSource::Managed;
    expiry.credential.current.as_mut().unwrap().expires_at = "invalid fixture expiry".into();
    expiry.validate().unwrap();
    let mut scope = valid.clone();
    scope.credential.reaches = vec![];
    scope.profile.content.nyxid_services = vec![];
    // The persisted scope fingerprint must agree with the modified scope.
    scope.credential.current.as_mut().unwrap().grant_fingerprint =
        layer2_core::profiles::content_digest(&Vec::<ServiceId>::new()).unwrap();
    scope.validate().unwrap();
    for (sandbox, dependency, reason) in [
        (corrupt, Dependency::Storage, Reason::Corrupt),
        (
            missing,
            Dependency::AuthenticationAuthority,
            Reason::CredentialMissing,
        ),
        (
            scope,
            Dependency::AuthenticationAuthority,
            Reason::ScopeMissing,
        ),
        (
            expiry,
            Dependency::AuthenticationAuthority,
            Reason::InvalidEvidence,
        ),
    ] {
        let (result, failure) = capture(catalog.models(&sandbox)).await;
        assert_eq!(result, Err(AgentModelCatalogError::Unavailable));
        assert_eq!(failure, Some(Failure::new(dependency, reason)));
    }
    let unavailable_catalog =
        ChronoAgentModels::new("http://127.0.0.1:1", Arc::new(UnavailableCredentials)).unwrap();
    let (result, failure) = capture(unavailable_catalog.models(&valid)).await;
    assert_eq!(result, Err(AgentModelCatalogError::Unavailable));
    assert_eq!(
        failure,
        Some(Failure::new(Dependency::Storage, Reason::Unavailable))
    );
    let mut wrong_owner = original;
    wrong_owner.owner = SubjectId::new("different-fixture");
    custody
        .values
        .lock()
        .unwrap()
        .insert((valid.id.clone(), 1), wrong_owner);
    let (result, failure) = capture(catalog.models(&valid)).await;
    assert_eq!(result, Err(AgentModelCatalogError::Unavailable));
    assert_eq!(
        failure,
        Some(Failure::new(
            Dependency::AuthenticationAuthority,
            Reason::CredentialMismatch
        ))
    );
}

struct UnavailableCredentials;
#[async_trait::async_trait]
impl SandboxCredentialController for UnavailableCredentials {
    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        unreachable!()
    }
    async fn material(
        &self,
        _: &SandboxId,
        _: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        Err(CredentialError::Unavailable(
            "private fixture storage detail".into(),
        ))
    }
    async fn custodied(&self, _: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        unreachable!()
    }
    async fn forget(&self, _: &SandboxId, _: u64) -> Result<(), CredentialError> {
        unreachable!()
    }
    async fn revoke(&self, _: &Sandbox, _: &str) -> Result<(), CredentialError> {
        unreachable!()
    }
}
