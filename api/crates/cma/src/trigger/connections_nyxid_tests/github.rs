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

#[tokio::test]
async fn verifies_only_the_selected_active_user_service_and_its_live_grant() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() =
        Reply::json(json!({"services": [{"id": "github-catalog", "slug": "api-github"}]}));
    let strong = json!({"id":"strong", "slug":"api-github-2", "catalog_service_id":"github-catalog",
        "is_active":true, "credential_source":{"type":"personal"}});
    let weak = json!({"id":"weak", "slug":"strong", "catalog_service_id":"github-catalog",
        "is_active":true, "credential_source":{"type":"personal"}});
    provider.state.credential_replies.lock().unwrap().extend([
        ("strong".into(), Reply::json(json!({"id":"strong","slug":"api-github-2",
            "catalog_service_id":"github-catalog","label":"Repository access","granted_scopes":["repo"]}))),
        ("weak".into(), Reply::json(json!({"id":"weak","slug":"strong",
            "catalog_service_id":"github-catalog","label":"Profile access","granted_scopes":["read:user"]}))),
    ]);
    provider.state.set_services_reply(Reply::json(
        json!({"services":[strong.clone(), weak.clone()]}),
    ));
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor");
    assert!(matches!(
        broker.verify_github_repo_connection(&owner, "weak").await,
        Err(ServiceConnectionError::Refused)
    ));
    assert!(provider.state.github_proxy_reads.lock().unwrap().is_empty());
    assert!(matches!(
        broker.verify_github_repo_connection(&owner, "strong").await,
        Err(ServiceConnectionError::Unavailable(_))
    ));
    *provider.state.github_proxy_scopes.lock().unwrap() = Some("read:user".into());
    assert!(matches!(
        broker.verify_github_repo_connection(&owner, "strong").await,
        Err(ServiceConnectionError::Refused)
    ));
    *provider.state.github_proxy_scopes.lock().unwrap() = Some("read:user, repo".into());
    broker
        .verify_github_repo_connection(&owner, "strong")
        .await
        .unwrap();
    let reads = provider.state.github_proxy_reads.lock().unwrap().clone();
    assert_eq!(reads, vec![("github-catalog".into(), "strong".into()); 3]);

    let mut inactive = strong.clone();
    inactive["is_active"] = json!(false);
    let mut unauthorized = strong.clone();
    unauthorized["credential_source"] = json!({"type":"org","allowed":false});
    let mut wrong_catalog = strong.clone();
    wrong_catalog["catalog_service_id"] = json!("other-catalog");
    for rows in [
        vec![weak.clone()],
        vec![weak.clone(), inactive],
        vec![weak.clone(), unauthorized],
        vec![weak.clone(), wrong_catalog],
        vec![weak, strong.clone(), strong],
    ] {
        provider
            .state
            .set_services_reply(Reply::json(json!({"services":rows})));
        assert!(matches!(
            broker.verify_github_repo_connection(&owner, "strong").await,
            Err(ServiceConnectionError::Refused)
        ));
    }
    assert_eq!(*provider.state.github_proxy_reads.lock().unwrap(), reads);
}
