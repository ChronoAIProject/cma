use super::*;

#[tokio::test]
async fn reports_both_owner_visible_grants_without_claiming_a_selected_connection() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() =
        Reply::json(json!({"services": [{"id": "github-catalog", "slug": "api-github"}]}));
    let weak = json!({"id":"weak", "slug":"api-github", "catalog_service_id":"github-catalog",
        "is_active":true, "credential_source":{"type":"personal"}});
    let strong = json!({"id":"strong", "slug":"api-github-2", "catalog_service_id":"github-catalog",
        "is_active":true, "credential_source":{"type":"personal"}});
    let denied = json!({"id":"denied", "slug":"api-github", "catalog_service_id":"github-catalog",
        "is_active":true, "credential_source":{"type":"org","allowed":false}});
    provider.state.credential_replies.lock().unwrap().extend([
        (
            "weak".into(),
            Reply::json(json!({"id":"weak", "slug":"api-github",
            "catalog_service_id":"github-catalog", "label":"Profile access",
            "granted_scopes":["read:user"]})),
        ),
        (
            "strong".into(),
            Reply::json(json!({"id":"strong", "slug":"api-github-2",
            "catalog_service_id":"github-catalog", "label":"Repository access",
            "granted_scopes":["repo", "read:user"]})),
        ),
    ]);
    let tokens = Tokens::available();
    let broker = broker(&provider, tokens.clone());
    for rows in [
        vec![weak.clone(), strong.clone(), denied.clone()],
        vec![strong.clone(), denied.clone(), weak.clone()],
    ] {
        provider
            .state
            .set_services_reply(Reply::json(json!({"services": rows})));
        let result = broker.github_connections(&SubjectId::new("visitor")).await;
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "manage_url": "https://nyx.example/keys",
                "connections": [
                    {"id":"strong","label":"Repository access","granted_scopes":["repo","read:user"],
                        "manage_url":"https://nyx.example/keys/strong"},
                    {"id":"weak","label":"Profile access","granted_scopes":["read:user"],
                        "manage_url":"https://nyx.example/keys/weak"}
                ]
            })
        );
    }
    assert_eq!(provider.state.credential_reads.lock().unwrap().len(), 4);
    assert_eq!(
        tokens.owners.lock().unwrap().as_slice(),
        &[SubjectId::new("visitor"), SubjectId::new("visitor")]
    );

    provider.state.credential_replies.lock().unwrap().insert(
        "strong".into(),
        Reply::json(json!({"id":"different","slug":"api-github-2",
            "catalog_service_id":"github-catalog","label":"Repository access","granted_scopes":["repo"]})),
    );
    let result = broker.github_connections(&SubjectId::new("visitor")).await;
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"manage_url":"https://nyx.example/keys"})
    );
}
