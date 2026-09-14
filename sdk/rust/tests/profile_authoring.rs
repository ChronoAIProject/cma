#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error, Result,
    profile_authoring::{ProfileAuthoringSkillSearch, ResolveRepositoryRequest},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use wire::WireServer;

fn bounds() -> Value {
    json!({"min_cpu_millis":100,"max_cpu_millis":8000,"min_memory_mib":128,"max_memory_mib":16384})
}

fn skill() -> Value {
    json!({"id":"review","name":"Review","versions":["999999999999999999.1","2.10","2.9","0.0"]})
}

fn repository(reference: Option<&str>) -> Value {
    let mut value = json!({"url":"https://github.com/Owner/Project","default_branch":"main","commit_sha":"a".repeat(40)});
    if let Some(reference) = reference {
        value["reference"] = json!(reference);
    }
    value
}

fn repository_request(reference: Option<&str>) -> ResolveRepositoryRequest {
    ResolveRepositoryRequest {
        url: "https://github.com/owner/project.git/".into(),
        reference: reference.map(str::to_owned),
    }
}

#[tokio::test]
async fn authoring_reads_use_authenticated_exact_paths_and_preserve_opaque_values() {
    let server = WireServer::new().await;
    server.expect(wire::json(
        "GET",
        "/api/v1/profile-authoring/bounds",
        None,
        200,
        bounds(),
        true,
    ));
    assert_eq!(
        server
            .client
            .profile_authoring()
            .bounds()
            .await
            .unwrap()
            .max_cpu_millis,
        8000
    );
    server.expect(wire::json("GET", "/api/v1/profile-authoring/services", None, 200,
        json!([{"id":"github","catalog_id":"canonical-github","name":"GitHub","description":"Repository access","private_data":"not projected"}]), true));
    let services = server.client.profile_authoring().services().await.unwrap();
    assert_eq!(services[0].id, "github");
    assert_eq!(services[0].catalog_id.as_deref(), Some("canonical-github"));
    assert_eq!(
        services[0].description.as_deref(),
        Some("Repository access")
    );
    server.expect(wire::json(
        "GET",
        "/api/v1/profile-authoring/skills?q=build+%26+test%2F%2B&limit=2&cursor=cursor%2Fone%2B",
        None,
        200,
        json!({"items":[skill()],"next":"cursor/two+"}),
        true,
    ));
    let first = server
        .client
        .profile_authoring()
        .search_skills(&ProfileAuthoringSkillSearch {
            q: Some("build & test/+".into()),
            limit: Some(2),
            cursor: Some("cursor/one+".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        first.items[0].versions,
        ["999999999999999999.1", "2.10", "2.9", "0.0"]
    );
    server.expect(wire::json(
        "GET",
        "/api/v1/profile-authoring/skills?cursor=cursor%2Ftwo%2B",
        None,
        200,
        json!({"items":[]}),
        true,
    ));
    let last = server
        .client
        .profile_authoring()
        .search_skills(&ProfileAuthoringSkillSearch {
            cursor: first.next,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(last.items.is_empty());
    assert!(last.next.is_none());
    assert_eq!(server.request_count(), 4);
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}

#[tokio::test]
async fn repository_resolution_is_a_read_only_post_preserving_reference_absence() {
    let server = WireServer::new().await;
    for reference in [None, Some("release/v1")] {
        let request = repository_request(reference);
        let body = serde_json::to_value(&request).unwrap();
        assert_eq!(body.get("reference").is_some(), reference.is_some());
        server.expect(wire::json(
            "POST",
            "/api/v1/profile-authoring/repositories/resolve",
            Some(body),
            200,
            repository(reference),
            true,
        ));
        let result = server
            .client
            .profile_authoring()
            .resolve_repository(&request)
            .await
            .unwrap();
        assert_eq!(result.url, "https://github.com/Owner/Project");
        assert_eq!(result.reference.as_deref(), reference);
        assert_eq!(result.default_branch, "main");
        assert_eq!(result.commit_sha, "a".repeat(40));
    }
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn raw_repository_grammar_is_checked_after_server_admission() {
    let server = WireServer::new().await;
    let urls = [
        "http://github.com/Owner/Project".to_owned(),
        "https://github.com//Owner/Project".into(),
        "https://github.com/Owner//Project".into(),
        "https://github.com/Owner/Project//".into(),
        "https://github.com/Owner/./Project".into(),
        "https://github.com/Owner/Other/../Project".into(),
        "https://github.com/Owner/%2e/Project".into(),
        "https://github.com/Owner/Project?query".into(),
        "https://github.com/Owner/Project#fragment".into(),
        "https://github.com/./Project".into(),
        "https://github.com/Owner/..git".into(),
        format!("https://github.com/{}/Project", "o".repeat(101)),
        format!("https://github.com/Owner/{}.git/", "r".repeat(101)),
    ];
    for url in &urls {
        for requested_url in [repository_request(None).url, url.clone()] {
            let request = ResolveRepositoryRequest {
                url: requested_url,
                reference: None,
            };
            let mut response = repository(None);
            response["url"] = json!(url);
            server.expect(wire::json(
                "POST",
                "/api/v1/profile-authoring/repositories/resolve",
                Some(serde_json::to_value(&request).unwrap()),
                200,
                response,
                true,
            ));
            assert!(
                matches!(
                    server
                        .client
                        .profile_authoring()
                        .resolve_repository(&request)
                        .await,
                    Err(Error::Protocol { .. })
                ),
                "{url}"
            );
        }
        let request = ResolveRepositoryRequest {
            url: url.clone(),
            reference: None,
        };
        server.expect(wire::json(
            "POST",
            "/api/v1/profile-authoring/repositories/resolve",
            Some(serde_json::to_value(&request).unwrap()),
            422,
            json!({"status":422,"code":"invalid_repository","detail":"Repository refused"}),
            true,
        ));
        assert!(matches!(
            server.client.profile_authoring().resolve_repository(&request).await,
            Err(Error::Api { status: 422, code: Some(code), .. }) if code == "invalid_repository"
        ));
    }
    assert_eq!(server.request_count(), urls.len() * 3);
    server.assert_drained();
}

#[tokio::test]
async fn repository_component_bound_applies_after_git_suffix_normalization() {
    let server = WireServer::new().await;
    let identity = format!("{}/{}", "o".repeat(100), "r".repeat(100));
    let request = ResolveRepositoryRequest {
        url: format!("https://github.com/{}.git/", identity.to_uppercase()),
        reference: None,
    };
    let mut response = repository(None);
    response["url"] = json!(format!("https://github.com/{identity}"));
    server.expect(wire::json(
        "POST",
        "/api/v1/profile-authoring/repositories/resolve",
        Some(serde_json::to_value(&request).unwrap()),
        200,
        response.clone(),
        true,
    ));
    let resolved = server
        .client
        .profile_authoring()
        .resolve_repository(&request)
        .await
        .unwrap();
    assert_eq!(resolved.url, response["url"].as_str().unwrap());
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn skill_search_rejects_invalid_byte_bounds_and_controls_before_http() {
    let server = WireServer::new().await;
    let cases = [
        ProfileAuthoringSkillSearch {
            limit: Some(0),
            ..Default::default()
        },
        ProfileAuthoringSkillSearch {
            limit: Some(51),
            ..Default::default()
        },
        ProfileAuthoringSkillSearch {
            q: Some("\u{00e9}".repeat(101)),
            ..Default::default()
        },
        ProfileAuthoringSkillSearch {
            q: Some("search\nline".into()),
            ..Default::default()
        },
        ProfileAuthoringSkillSearch {
            cursor: Some(String::new()),
            ..Default::default()
        },
        ProfileAuthoringSkillSearch {
            cursor: Some("x".repeat(2049)),
            ..Default::default()
        },
        ProfileAuthoringSkillSearch {
            cursor: Some("cursor\u{0085}".into()),
            ..Default::default()
        },
    ];
    for query in cases {
        assert!(matches!(
            server
                .client
                .profile_authoring()
                .search_skills(&query)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
    server.assert_drained();
}

async fn invoke(server: &WireServer, path: &str) -> Result<()> {
    let authoring = server.client.profile_authoring();
    match path {
        "bounds" => authoring.bounds().await.map(|_| ()),
        "services" => authoring.services().await.map(|_| ()),
        "skills" => authoring
            .search_skills(&ProfileAuthoringSkillSearch::default())
            .await
            .map(|_| ()),
        "repositories/resolve" => authoring
            .resolve_repository(&repository_request(None))
            .await
            .map(|_| ()),
        _ => panic!("unknown authoring fixture path"),
    }
}

fn expect(server: &WireServer, path: &str, status: u16, body: Value) {
    let repository = path == "repositories/resolve";
    server.expect(wire::json(
        if repository { "POST" } else { "GET" },
        &format!("/api/v1/profile-authoring/{path}"),
        repository.then(|| serde_json::to_value(repository_request(None)).unwrap()),
        status,
        body,
        true,
    ));
}

#[tokio::test]
async fn catalog_descriptions_use_the_authoring_specific_utf8_bound() {
    let server = WireServer::new().await;
    let description = "\u{754c}".repeat(2048);
    assert_eq!(description.len(), 6144);
    for path in ["services", "skills"] {
        for suffix in ["", "a"] {
            let mut entry = skill();
            entry["description"] = json!(format!("{description}{suffix}"));
            let response = if path == "services" {
                json!([entry])
            } else {
                json!({"items":[entry]})
            };
            expect(&server, path, 200, response);
            let result = invoke(&server, path).await;
            if suffix.is_empty() {
                result.unwrap();
            } else {
                assert!(matches!(result, Err(Error::Protocol { .. })));
            }
        }
    }
    assert_eq!(server.request_count(), 4);
    server.assert_drained();
}

#[tokio::test]
async fn empty_filtered_skill_pages_retain_advancing_continuations() {
    let server = WireServer::new().await;
    for cursor in ["previous", "next-filtered"] {
        server.expect(wire::json(
            "GET",
            &format!("/api/v1/profile-authoring/skills?cursor={cursor}"),
            None,
            200,
            json!({"items":[],"next":"next-filtered"}),
            true,
        ));
        let result = server
            .client
            .profile_authoring()
            .search_skills(&ProfileAuthoringSkillSearch {
                cursor: Some(cursor.into()),
                ..Default::default()
            })
            .await;
        if cursor == "previous" {
            let page = result.unwrap();
            assert!(page.items.is_empty());
            assert_eq!(page.next.as_deref(), Some("next-filtered"));
        } else {
            assert!(matches!(result, Err(Error::Protocol { .. })));
        }
    }
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn authoring_refuses_malformed_response_shapes_and_inconsistent_evidence() {
    let server = WireServer::new().await;
    let mut invalid_bounds = bounds();
    invalid_bounds["min_cpu_millis"] = json!(9000);
    let mut foreign_repository = repository(None);
    foreign_repository["url"] = json!("https://github.com/Other/Project");
    let mut invalid_commit = repository(None);
    invalid_commit["commit_sha"] = json!("A".repeat(40));
    let mut invalid_default = repository(None);
    invalid_default["default_branch"] = json!("");
    let mut invalid_version = skill();
    invalid_version["versions"] = json!(["01.2"]);
    let mut unordered = skill();
    unordered["versions"] = json!(["2.9", "2.10"]);
    for (path, body) in [
        ("bounds", invalid_bounds),
        (
            "bounds",
            json!({"min_cpu_millis":0,"max_cpu_millis":1,"min_memory_mib":1,"max_memory_mib":1}),
        ),
        ("bounds", json!({"min_cpu_millis":"100"})),
        (
            "services",
            json!([{"id":"same","name":"One"},{"id":"same","name":"Two"}]),
        ),
        ("services", json!([{"id":"service","name":""}])),
        (
            "services",
            json!([{"id":"service","name":"Name","description":null}]),
        ),
        ("services", json!({"items":[]})),
        (
            "skills",
            json!({"items":[{"id":"review","name":"Review","description":null,"versions":["1.0"]}]}),
        ),
        ("skills", json!({"items":[skill()],"next":null})),
        ("skills", json!({"items":[invalid_version]})),
        ("skills", json!({"items":[unordered]})),
        ("skills", json!({"items":[skill(),skill()]})),
        ("skills", json!({"items":[skill()],"next":""})),
        ("repositories/resolve", foreign_repository),
        ("repositories/resolve", invalid_commit),
        ("repositories/resolve", invalid_default),
        ("repositories/resolve", repository(Some("main"))),
        (
            "repositories/resolve",
            json!({"url":repository(None)["url"],"reference":null,"default_branch":"main","commit_sha":"a".repeat(40)}),
        ),
    ] {
        expect(&server, path, 200, body);
        assert!(
            matches!(invoke(&server, path).await, Err(Error::Protocol { .. })),
            "{path}"
        );
    }
    assert_eq!(server.request_count(), 18);
    server.assert_drained();
}

#[tokio::test]
async fn authoring_preserves_typed_refusals_without_retry_and_requires_http_200() {
    let server = WireServer::new().await;
    let cases = [
        ("bounds", 401, "not_authenticated"),
        ("bounds", 403, "forbidden"),
        ("services", 403, "forbidden"),
        ("services", 503, "profile_services_unavailable"),
        ("skills", 403, "forbidden"),
        ("skills", 400, "invalid_profile_authoring_query"),
        ("skills", 503, "profile_skills_unavailable"),
        ("repositories/resolve", 403, "forbidden"),
        ("repositories/resolve", 422, "invalid_repository"),
        ("repositories/resolve", 422, "invalid_repository_reference"),
        ("repositories/resolve", 404, "repository_not_found"),
        (
            "repositories/resolve",
            404,
            "repository_reference_not_found",
        ),
        ("repositories/resolve", 503, "repository_unavailable"),
    ];
    for (path, status, code) in cases {
        let repository = path == "repositories/resolve";
        let mut expected = wire::json(
            if repository { "POST" } else { "GET" },
            &format!("/api/v1/profile-authoring/{path}"),
            repository.then(|| serde_json::to_value(repository_request(None)).unwrap()),
            status,
            json!({"status":status,"code":code,"detail":"Authoring refused","field":"authoring"}),
            true,
        );
        expected
            .response_headers
            .insert("x-request-id".into(), "request-authoring".into());
        expected
            .response_headers
            .insert("retry-after".into(), "3".into());
        server.expect(expected);
        assert_eq!(
            invoke(&server, path).await.unwrap_err(),
            Error::Api {
                status,
                code: Some(code.into()),
                message: "Authoring refused".into(),
                request_id: Some("request-authoring".into()),
                retry_after: Some(Duration::from_secs(3)),
                extensions: BTreeMap::from([("field".into(), json!("authoring"))]),
            }
        );
    }
    for path in ["bounds", "services", "skills", "repositories/resolve"] {
        expect(&server, path, 201, json!({}));
        assert!(matches!(
            invoke(&server, path).await,
            Err(Error::Protocol { .. })
        ));
    }
    assert_eq!(server.request_count(), cases.len() + 4);
    server.assert_drained();
}
