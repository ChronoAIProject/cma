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
    *provider.state.github_proxy_scopes.lock().unwrap() = Some("repo".into());
    for rows in [
        vec![weak.clone(), strong.clone()],
        vec![strong.clone(), weak.clone()],
    ] {
        provider
            .state
            .set_services_reply(Reply::json(json!({"services":rows})));
        let report = broker.github_connections(&owner, false).await;
        assert_eq!(
            serde_json::to_value(report.scope_check(PublicRepositoryWrite)).unwrap(),
            json!({
                "status":"insufficient_scope", "connection_id":"weak", "required_scope":"public_repo",
                "manage_url":"https://nyx.example/keys/weak"
            })
        );
        assert_eq!(
            serde_json::to_value(
                broker
                    .exact_github_scope_check(&owner, "strong", PublicRepositoryWrite)
                    .await
            )
            .unwrap(),
            json!({"status":"scope_satisfied", "connection_id":"strong"})
        );
        assert_eq!(
            serde_json::to_value(
                broker
                    .exact_github_scope_check(&owner, "foreign", PublicRepositoryWrite)
                    .await
            )
            .unwrap(),
            json!({"status":"verification_unavailable"})
        );
    }
    provider.state.set_services_reply(Reply::json(json!({
        "services":[weak.clone(), row("strong", "api-github-2", false)]
    })));
    assert_eq!(
        serde_json::to_value(
            broker
                .exact_github_scope_check(&owner, "strong", PublicRepositoryWrite)
                .await
        )
        .unwrap(),
        json!({"status":"verification_unavailable"})
    );

    provider.state.credential_replies.lock().unwrap().insert(
        "strong".into(),
        key("strong", "api-github", json!(["repo"])),
    );
    let other = row("strong", "api-github", true);
    for rows in [vec![weak.clone(), other.clone()], vec![other, weak.clone()]] {
        provider
            .state
            .set_services_reply(Reply::json(json!({"services":rows})));
        let report = broker.github_connections(&owner, false).await;
        assert_eq!(
            serde_json::to_value(report.scope_check(PublicRepositoryWrite)).unwrap(),
            json!({"status":"verification_unavailable"})
        );
    }

    provider.state.set_services_reply(Reply::json(json!({
        "services":[row("weak", "api-github", false)]
    })));
    let report = broker.github_connections(&owner, false).await;
    assert_eq!(
        serde_json::to_value(report.scope_check(PublicRepositoryWrite)).unwrap(),
        json!({"status":"missing_connection"})
    );
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":[{
            "id":"weak", "slug":"api-github", "catalog_service_id":"foreign", "is_active":true
        }]})));
    let report = broker.github_connections(&owner, false).await;
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
        let report = broker.github_connections(&owner, false).await;
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
        let report = broker.github_connections(&owner, false).await;
        for need in [PublicRepositoryWrite, PrivateRepositoryRead] {
            let check = serde_json::to_value(report.scope_check(need)).unwrap();
            assert_eq!(check["status"], expected);
        }
    }
}

#[tokio::test]
async fn exact_scope_reports_require_agreeing_live_and_recorded_permissions() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() =
        Reply::json(json!({"services":[{"id":"github-catalog","slug":"api-github"}]}));
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":[{
            "id":"selected", "slug":"api-github-2", "catalog_service_id":"github-catalog",
            "is_active":true, "credential_source":{"type":"personal"}
        }]})));
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor");
    for (stored, live, need, status) in [
        (
            json!(["public_repo"]),
            Some("public_repo"),
            PublicRepositoryWrite,
            "scope_satisfied",
        ),
        (
            json!(["public_repo"]),
            Some("public_repo"),
            PrivateRepositoryRead,
            "insufficient_scope",
        ),
        (
            json!(["repo"]),
            Some("read:user"),
            PublicRepositoryWrite,
            "verification_unavailable",
        ),
        (
            json!(["repo"]),
            Some("repo,,read:user"),
            PrivateRepositoryRead,
            "verification_unavailable",
        ),
        (
            json!(["repo"]),
            None,
            PrivateRepositoryRead,
            "verification_unavailable",
        ),
        (
            json!([]),
            Some(""),
            PublicRepositoryWrite,
            "insufficient_scope",
        ),
    ] {
        provider.state.credential_replies.lock().unwrap().insert(
            "selected".into(),
            Reply::json(json!({
                "id":"selected", "slug":"api-github-2", "catalog_service_id":"github-catalog",
                "label":"Owner's connection", "granted_scopes":stored, "credential_type":"oauth2",
                "is_active":true, "status":"active", "connection_status":"active"
            })),
        );
        *provider.state.github_proxy_scopes.lock().unwrap() = live.map(str::to_owned);
        let value = serde_json::to_value(
            broker
                .exact_github_scope_check(&owner, "selected", need)
                .await,
        )
        .unwrap();
        assert_eq!(value["status"], status);
        if status == "insufficient_scope" {
            assert_eq!(value["connection_id"], "selected");
            assert_eq!(value["manage_url"], "https://nyx.example/keys/selected");
        }
    }
    assert_eq!(
        *provider.state.github_proxy_reads.lock().unwrap(),
        vec![("github-catalog".into(), "selected".into()); 6]
    );
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
        let result = broker
            .github_connections(&SubjectId::new("visitor"), false)
            .await;
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
    let result = broker
        .github_connections(&SubjectId::new("visitor"), false)
        .await;
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"manage_url":"https://nyx.example/keys"})
    );
}

#[path = "github/account.rs"]
mod account;
