use super::*;

const CATALOG_SCOPE_REFUSAL: &str =
    "API key scope forbidden: API key does not have access to this service";

#[tokio::test]
async fn catalog_scope_refusal_preserves_request_authority_at_search_and_version_reads() {
    let search = format!("{SKILLS}/skill-search");
    let versions = format!("{SKILLS}/skills/research/versions");
    let tokens = TestTokens::new(Some("unrelated-stored-authority"));
    let credential = AuthoringCredential::new("request-authority".into());
    for refused_read in [&search, &versions] {
        let upstream = Upstream::start().await;
        upstream.skills(json!([{"guid":"research","name":"Research"}]), false, None);
        upstream.versions("research", json!([{"version":"1.2","isDeprecated":false}]));
        upstream.response(
            refused_read,
            403,
            json!({
                "error": "api_key_scope_forbidden",
                "error_code": 9000,
                "message": CATALOG_SCOPE_REFUSAL,
            }),
        );
        let source = upstream.source(Some(tokens.clone()));
        assert_eq!(
            source.skills(&owner(), &query(), Some(&credential)).await,
            Err(AuthoringSourceError::SkillsAuthorizationRequired)
        );
        let calls = upstream.seen();
        assert_eq!(calls.last().unwrap().path, *refused_read);
        assert_eq!(calls.len(), if refused_read == &search { 1 } else { 2 });
        assert!(calls
            .iter()
            .all(|call| { call.authorization.as_deref() == Some("Bearer request-authority") }));
        assert!(tokens.owners.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_node_scope_refusal_is_not_a_catalog_consent_request() {
    let upstream = Upstream::start().await;
    upstream.response(
        &format!("{SKILLS}/skill-search"),
        403,
        json!({
            "error": "api_key_scope_forbidden",
            "message": "API key scope forbidden: API key does not have access to this node",
        }),
    );
    assert_eq!(
        upstream.source(None).skills(&owner(), &query(), None).await,
        Err(AuthoringSourceError::Unavailable(
            "skill search answered HTTP 403".into()
        ))
    );
}
