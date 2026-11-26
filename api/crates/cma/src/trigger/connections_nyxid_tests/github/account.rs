use super::*;

#[tokio::test]
async fn verifies_only_the_selected_active_user_service_and_its_live_grant() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() =
        Reply::json(json!({"services": [{"id": "github-catalog", "slug": "api-github"}]}));
    let strong = json!({"id":"strong", "slug":"api-github-2", "catalog_service_id":"github-catalog",
        "is_active":true, "credential_source":{"type":"personal"}});
    let weak = json!({"id":"weak", "slug":"strong", "catalog_service_id":"github-catalog",
        "is_active":true, "credential_source":{"type":"personal"}});
    let default = json!({"id":"default", "slug":"api-github",
        "catalog_service_id":"github-catalog", "is_active":true,
        "credential_source":{"type":"personal"}});
    let strong_detail = json!({"id":"strong", "slug":"api-github-2",
        "catalog_service_id":"github-catalog", "label":"Repository access",
        "granted_scopes":["repo"], "credential_type":"oauth2", "is_active":true,
        "status":"active", "connection_status":"active"});
    provider.state.credential_replies.lock().unwrap().extend([
        ("strong".into(), Reply::json(strong_detail.clone())),
        ("weak".into(), Reply::json(json!({"id":"weak","slug":"strong",
            "catalog_service_id":"github-catalog","label":"Profile access","granted_scopes":["read:user"],
            "credential_type":"oauth2","is_active":true,"status":"active","connection_status":"active"}))),
    ]);
    provider.state.set_services_reply(Reply::json(
        json!({"services":[default, strong.clone(), weak.clone()]}),
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
    let proof = broker
        .verify_github_repo_connection(&owner, "strong")
        .await
        .unwrap();
    assert_eq!(proof.id(), "strong");
    assert_eq!(proof.catalog_id(), "github-catalog");
    let reads = provider.state.github_proxy_reads.lock().unwrap().clone();
    assert_eq!(reads, vec![("github-catalog".into(), "strong".into()); 3]);

    for (field, value) in [
        ("status", Some(json!("revoked"))),
        ("connection_status", Some(json!("expired"))),
        ("credential_type", None),
        ("is_active", None),
        ("status", None),
        ("connection_status", None),
    ] {
        let mut detail = strong_detail.clone();
        let fields = detail.as_object_mut().unwrap();
        match value {
            Some(value) => {
                fields.insert(field.into(), value);
            }
            None => {
                fields.remove(field);
            }
        }
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert("strong".into(), Reply::json(detail));
        assert!(matches!(
            broker.verify_github_repo_connection(&owner, "strong").await,
            Err(ServiceConnectionError::Refused)
        ));
    }
    assert_eq!(*provider.state.github_proxy_reads.lock().unwrap(), reads);
    provider
        .state
        .credential_replies
        .lock()
        .unwrap()
        .insert("strong".into(), Reply::json(strong_detail));

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

    // Preview is purpose-specific. It verifies each visible exact row and
    // never claims a live scope merely because the stored grant is positive.
    provider.state.set_services_reply(Reply::json(json!({
        "services":[
            {"id":"strong","slug":"api-github-2","catalog_service_id":"github-catalog","is_active":true},
            {"id":"weak","slug":"strong","catalog_service_id":"github-catalog","is_active":true}
        ]
    })));
    *provider.state.github_proxy_scopes.lock().unwrap() = None;
    let ordinary = serde_json::to_value(broker.github_connections(&owner, false).await).unwrap();
    assert!(ordinary["connections"][0].get("account").is_none());
    assert_eq!(*provider.state.github_proxy_reads.lock().unwrap(), reads);
    let report = serde_json::to_value(broker.github_connections(&owner, true).await).unwrap();
    assert_eq!(
        report["connections"][0]["account"],
        json!({"login":"fixture-strong","observed_scopes":null})
    );
    assert_eq!(
        report["connections"][1]["account"],
        json!({"login":"fixture-weak","observed_scopes":null})
    );
    *provider.state.github_proxy_scopes.lock().unwrap() = Some("read:user, repo".into());
    let report = serde_json::to_value(broker.github_connections(&owner, true).await).unwrap();
    assert_eq!(
        report["connections"][0]["account"]["observed_scopes"],
        json!(["read:user", "repo"])
    );
    let observed = provider.state.github_proxy_reads.lock().unwrap();
    let mut preview_routes = observed[reads.len()..].to_vec();
    preview_routes.sort();
    assert_eq!(
        preview_routes,
        vec![
            ("github-catalog".into(), "strong".into()),
            ("github-catalog".into(), "strong".into()),
            ("github-catalog".into(), "weak".into()),
            ("github-catalog".into(), "weak".into()),
        ]
    );
}
