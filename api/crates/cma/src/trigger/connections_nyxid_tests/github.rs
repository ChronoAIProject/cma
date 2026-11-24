use super::*;
use crate::trigger::GitHubRepositoryNeed::{PrivateRepositoryRead, PublicRepositoryWrite};

#[tokio::test]
async fn declared_write_checks_only_the_unique_active_default_oauth_key() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() =
        Reply::json(json!({"services": [{"id": "github-catalog", "slug": "api-github"}]}));
    let row = |id, slug, active| {
        json!({
            "id": id, "slug": slug, "catalog_service_id": "github-catalog", "is_active": active,
            "credential_source": {"type":"personal"}
        })
    };
    let key = |id, slug, scopes: Value| {
        Reply::json(json!({
            "id":id, "slug":slug, "catalog_service_id":"github-catalog", "label":id,
            "credential_type":"oauth2", "is_active":true, "granted_scopes": scopes,
            "status":"active", "connection_status":"active"
        }))
    };
    let weak = row("weak", "api-github", true);
    let strong = row("strong", "api-github-2", true);
    provider.state.credential_replies.lock().unwrap().extend([
        (
            "weak".into(),
            key("weak", "api-github", json!(["read:user"])),
        ),
        (
            "strong".into(),
            key("strong", "api-github-2", json!(["repo"])),
        ),
    ]);
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor");
    for rows in [
        vec![weak.clone(), strong.clone()],
        vec![strong.clone(), weak.clone()],
    ] {
        provider
            .state
            .set_services_reply(Reply::json(json!({"services":rows})));
        let report = broker.github_connections(&owner).await;
        assert_eq!(
            serde_json::to_value(report.scope_check(PublicRepositoryWrite)).unwrap(),
            json!({
                "status":"insufficient_scope", "connection_id":"weak", "required_scope":"public_repo",
                "manage_url":"https://nyx.example/keys/weak"
            })
        );
    }
    provider.state.credential_replies.lock().unwrap().insert(
        "strong".into(),
        key("strong", "api-github", json!(["repo"])),
    );
    let other = row("strong", "api-github", true);
    for rows in [vec![weak.clone(), other.clone()], vec![other, weak.clone()]] {
        provider
            .state
            .set_services_reply(Reply::json(json!({"services":rows})));
        let report = broker.github_connections(&owner).await;
        assert_eq!(
            serde_json::to_value(report.scope_check(PublicRepositoryWrite)).unwrap(),
            json!({"status":"verification_unavailable"})
        );
    }

    provider.state.set_services_reply(Reply::json(json!({
        "services":[row("weak", "api-github", false)]
    })));
    let report = broker.github_connections(&owner).await;
    assert_eq!(
        serde_json::to_value(report.scope_check(PublicRepositoryWrite)).unwrap(),
        json!({"status":"missing_connection"})
    );
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":[{
            "id":"weak", "slug":"api-github", "catalog_service_id":"foreign", "is_active":true
        }]})));
    let report = broker.github_connections(&owner).await;
    assert_eq!(
        serde_json::to_value(report.scope_check(PublicRepositoryWrite)).unwrap(),
        json!({"status":"verification_unavailable"})
    );
}

#[tokio::test]
async fn scope_check_distinguishes_missing_and_empty_evidence_and_the_two_operations() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() =
        Reply::json(json!({"services":[{"id":"github-catalog","slug":"api-github"}]}));
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":[{
            "id":"selected", "slug":"api-github", "catalog_service_id":"github-catalog",
            "is_active":true, "credential_source":{"type":"personal"}
        }]})));
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor");
    for (credential_type, scopes, write, read) in [
        (
            "oauth2",
            json!(null),
            "verification_unavailable",
            "verification_unavailable",
        ),
        (
            "oauth2",
            json!([]),
            "insufficient_scope",
            "insufficient_scope",
        ),
        (
            "oauth2",
            json!([""]),
            "verification_unavailable",
            "verification_unavailable",
        ),
        (
            "oauth2",
            json!(["public_repo"]),
            "scope_satisfied",
            "insufficient_scope",
        ),
        (
            "oauth2",
            json!(["repo"]),
            "scope_satisfied",
            "scope_satisfied",
        ),
        (
            "bearer",
            json!(["repo"]),
            "verification_unavailable",
            "verification_unavailable",
        ),
    ] {
        provider.state.credential_replies.lock().unwrap().insert(
            "selected".into(), Reply::json(json!({
                "id":"selected", "slug":"api-github", "catalog_service_id":"github-catalog",
                "label":"GitHub", "is_active":true, "credential_type":credential_type, "granted_scopes":scopes,
                "status":"active", "connection_status":"active"
            })),
        );
        let report = broker.github_connections(&owner).await;
        for (need, expected) in [
            (PublicRepositoryWrite, write),
            (PrivateRepositoryRead, read),
        ] {
            let check = serde_json::to_value(report.scope_check(need)).unwrap();
            assert_eq!(check["status"], expected);
            if expected == "insufficient_scope" || expected == "scope_satisfied" {
                assert_eq!(check["connection_id"], "selected");
            }
        }
        assert!(serde_json::to_value(report)
            .unwrap()
            .get("scope_check")
            .is_none());
    }

    for (status, connection_status, expected) in [
        (Some("expired"), Some("active"), "verification_unavailable"),
        (Some("revoked"), Some("active"), "verification_unavailable"),
        (Some("active"), Some("expired"), "verification_unavailable"),
        (Some("active"), None, "verification_unavailable"),
        (None, Some("active"), "verification_unavailable"),
        (Some("active"), Some("active"), "scope_satisfied"),
    ] {
        let mut detail = json!({
            "id":"selected", "slug":"api-github", "catalog_service_id":"github-catalog",
            "label":"GitHub", "is_active":true, "credential_type":"oauth2", "granted_scopes":["repo"]
        });
        if let Some(status) = status {
            detail["status"] = json!(status);
        }
        if let Some(connection_status) = connection_status {
            detail["connection_status"] = json!(connection_status);
        }
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert("selected".into(), Reply::json(detail));
        let report = broker.github_connections(&owner).await;
        for need in [PublicRepositoryWrite, PrivateRepositoryRead] {
            let check = serde_json::to_value(report.scope_check(need)).unwrap();
            assert_eq!(check["status"], expected);
        }
    }
}

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
}
