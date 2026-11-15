//! Ingress evidence must never widen into renewable desktop authority.
use super::*;

fn request(source: CredentialSource) -> RequestAuth {
    RequestAuth {
        binding: None,
        credential_source: source,
        subject: SubjectId::new("desktop-owner"),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
        profile: Profile::default(),
        delegated_credential: Some(uuid::Uuid::new_v4().to_string()),
    }
}

fn provider() -> Arc<AuthState> {
    // These capture/refusal cases must complete without provider I/O.
    AuthState::in_memory(
        AuthMode::Provider {
            base_url: "http://127.0.0.1:1".into(),
            client_id: "desktop-boundaries".into(),
            client_secret: uuid::Uuid::new_v4().to_string(),
            public_url: "https://cma.example.test".into(),
        },
        None,
    )
}

#[tokio::test]
async fn trusted_authority_still_requires_the_exact_subject_and_capability() {
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "desktop-owner".into(),
        },
        None,
    );
    let headers = HeaderMap::new();
    let mut resolved = auth.resolve_request(&headers).await.unwrap().unwrap();
    let authority = capture(auth.clone(), &headers, &resolved).unwrap();
    authority
        .current(&Caller::new(
            resolved.subject.clone(),
            resolved.authorization.clone(),
        ))
        .await
        .unwrap();
    for caller in [
        Caller::new(SubjectId::new("other"), resolved.authorization.clone()),
        Caller::new(resolved.subject.clone(), Authorization::new([])),
    ] {
        assert!(matches!(
            authority.current(&caller).await,
            Err(DesktopError::Unavailable)
        ));
    }
    resolved.authorization = Authorization::new([]);
    assert!(capture(auth, &headers, &resolved).is_err());
}

#[tokio::test]
async fn missing_credentials_and_read_only_proofs_cannot_open_a_desktop() {
    let auth = provider();
    let mut headers = HeaderMap::new();
    for source in [
        CredentialSource::Retained,
        CredentialSource::IdentityProxy {
            grant_digest: [0; 32],
        },
        CredentialSource::Binding,
        CredentialSource::delegated("fixture-read-grant"),
        CredentialSource::RequestBoundRead {
            expires_at: SystemTime::now() + Duration::from_secs(60),
        },
    ] {
        assert!(capture(auth.clone(), &headers, &request(source)).is_err());
    }
    // Cookie presence alone cannot supply the separately verified bearer.
    headers.insert(
        header::COOKIE,
        format!("{AUTH_COOKIE}=fixture").parse().unwrap(),
    );
    for source in [
        CredentialSource::Retained,
        CredentialSource::Independent { expires_at: None },
    ] {
        let mut resolved = request(source);
        resolved.delegated_credential = None;
        assert!(capture(auth.clone(), &headers, &resolved).is_err());
    }
}

#[tokio::test]
async fn captured_proxy_proof_cannot_survive_a_missing_verifier() {
    let auth = provider();
    let resolved = request(CredentialSource::IdentityProxy {
        grant_digest: [0; 32],
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        super::super::super::nyxid_proxy::HEADER,
        "fixture".parse().unwrap(),
    );
    let authority = capture(auth, &headers, &resolved).unwrap();
    assert!(matches!(
        authority
            .current(&Caller::new(resolved.subject, resolved.authorization))
            .await,
        Err(DesktopError::Unavailable)
    ));
}
