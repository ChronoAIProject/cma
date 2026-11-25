use super::*;

#[tokio::test]
async fn trusted_mode_attaches_only_daily_authority() {
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "developer".into(),
        },
        None,
    );
    let resolved = auth
        .resolve_request(&HeaderMap::new())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.subject.as_str(), "developer");
    assert!(resolved.authorization.allows(Capability::UseOwnSandboxes));
    assert!(!resolved.authorization.allows(Capability::ConfigurePlatform));
}

#[test]
fn provider_diagnostics_redact_the_client_secret() {
    let mode = AuthMode::Provider {
        base_url: "https://identity.example".into(),
        client_id: "client".into(),
        client_secret: "never-print-me".into(),
        public_url: "https://cma.example".into(),
    };
    assert!(!format!("{mode:?}").contains("never-print-me"));
}

#[test]
fn endpoint_rebasing_preserves_only_the_provider_path_and_query() {
    assert_eq!(
        rebase(
            "https://public.example/oauth/token?audience=cma",
            "http://identity.internal:9090"
        )
        .unwrap(),
        "http://identity.internal:9090/oauth/token?audience=cma"
    );
}
