use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse as _;
use axum::routing::any;
use layer3_adapter::{CredentialFrame, RuntimeCredential};
use tokio::sync::mpsc;

use super::*;

struct Fixture {
    root: PathBuf,
    config: GithubProxyConfig,
    environment: BTreeMap<String, String>,
}

impl Fixture {
    fn new(upstream: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from("/tmp").join(format!(
            "cma-gh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let mut environment = BTreeMap::from([(
            "CHRONO_SANDBOX_REACHES".into(),
            "chrono-llm-public,ornn-api".into(),
        )]);
        let platform = ProxyConfig {
            role: "platform",
            bind: "127.0.0.1:9092".parse().unwrap(),
            upstream: upstream.into(),
        };
        let config = configuration(&root.join("state"), &mut environment, Some(&platform))
            .unwrap()
            .unwrap();
        Self {
            root,
            config,
            environment,
        }
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .unix_socket(self.config.directory.join("api.sock"))
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

async fn install(holder: &CredentialHolder, generation: u64) {
    holder
        .install(&CredentialFrame {
            operation_id: format!("github-credential-{generation}"),
            value: RuntimeCredential::new(format!("scoped-fixture-generation-{generation}")),
            custody_generation: Some(generation),
            github_route: None,
        })
        .await
        .unwrap();
}

#[test]
fn configuration_prepares_managed_transport_without_a_grant_and_keeps_identity_out() {
    for reaches in [
        "",
        "api-github-other",
        "ornn-api,chrono-llm",
        "other-api-github",
    ] {
        let mut env = BTreeMap::from([("CHRONO_SANDBOX_REACHES".into(), reaches.into())]);
        let original = env.clone();
        assert!(configuration(Path::new("/tmp/state"), &mut env, None)
            .unwrap()
            .is_none());
        assert_eq!(env, original);
    }
    let mut env = BTreeMap::from([("CHRONO_SANDBOX_REACHES".into(), "api-github".into())]);
    assert!(configuration(Path::new("/tmp/state"), &mut env, None).is_err());
    let fixture = Fixture::new("https://nyxid.example");
    assert!(!fixture.environment["CHRONO_SANDBOX_REACHES"].contains("api-github"));
    assert_eq!(
        fixture.config.upstream,
        "https://nyxid.example/api/v1/proxy/s/api-github"
    );
    assert_eq!(fixture.environment["GH_TOKEN"], PLACEHOLDER);
    assert_eq!(fixture.environment["GH_HOST"], "github.com");
    assert!(!fixture
        .environment
        .keys()
        .any(|key| key.starts_with("GIT_")));
    for name in ["GH_TOKEN", "GH_HOST", "GH_CONFIG_DIR", "GH_PROMPT_DISABLED"] {
        assert!(reserved_environment(name));
    }
    assert!(!reserved_environment("AGENT_SETTING"));
}

#[tokio::test]
async fn socket_and_configuration_are_private_recoverable_and_not_replaced_while_live() {
    let fixture = Fixture::new("https://nyxid.example");
    let prepared = PreparedGithubProxy::bind(&fixture.config).await.unwrap();
    for (path, mode) in [
        (fixture.config.directory.clone(), 0o700),
        (fixture.config.directory.join("api.sock"), 0o600),
        (fixture.config.directory.join("config.yml"), 0o600),
    ] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.config.directory.join("config.yml")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        config["http_unix_socket"],
        fixture.config.directory.join("api.sock").to_str().unwrap()
    );
    assert_eq!(config.as_object().unwrap().len(), 2);
    assert_eq!(
        PreparedGithubProxy::bind(&fixture.config)
            .await
            .err()
            .unwrap()
            .code,
        "github_proxy_in_use"
    );
    drop(prepared);
    drop(PreparedGithubProxy::bind(&fixture.config).await.unwrap());
    std::fs::remove_file(fixture.config.directory.join("api.sock")).unwrap();
    std::fs::write(fixture.config.directory.join("api.sock"), b"owner-data").unwrap();
    assert_eq!(
        PreparedGithubProxy::bind(&fixture.config)
            .await
            .err()
            .unwrap()
            .code,
        "github_proxy_path_conflict"
    );
    assert_eq!(
        std::fs::read(fixture.config.directory.join("api.sock")).unwrap(),
        b"owner-data"
    );
}

#[tokio::test]
async fn proxy_preserves_requests_rotates_custody_and_refuses_foreign_hosts() {
    let (sent, mut received) = mpsc::channel(8);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener_addr = listener.local_addr().unwrap();
    let fixture = Fixture::new(&format!("http://{listener_addr}"));
    let upstream = tokio::spawn(async move {
        let app = axum::Router::new().fallback(any(move |request: axum::extract::Request| {
            let sent = sent.clone();
            async move {
                let (parts, body) = request.into_parts();
                let body = axum::body::to_bytes(body, 1024).await.unwrap();
                sent.send((parts.method, parts.uri, parts.headers, body))
                    .await
                    .unwrap();
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"login":"viewer"}"#,
                )
            }
        }));
        axum::serve(listener, app).await.unwrap();
    });
    let holder = CredentialHolder::new();
    PreparedGithubProxy::bind(&fixture.config)
        .await
        .unwrap()
        .start(holder.clone());
    let client = fixture.client();
    assert_eq!(
        client
            .get("http://api.github.com/user")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    for generation in [1, 2] {
        install(&holder, generation).await;
        let response = client
            .post("http://api.github.com/graphql?test=1")
            .bearer_auth(PLACEHOLDER)
            .header("open-sandbox-api-key", "caller-routing-value")
            .header("x-github-api-version", "2022-11-28")
            .json(&serde_json::json!({"query": "query { viewer { login } }"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["login"],
            "viewer"
        );
        let (method, uri, headers, body) = received.recv().await.unwrap();
        assert_eq!(method, "POST");
        assert_eq!(uri, "/api/v1/proxy/s/api-github/graphql?test=1");
        assert_eq!(
            headers[header::AUTHORIZATION],
            format!("Bearer scoped-fixture-generation-{generation}")
        );
        assert_eq!(headers["x-github-api-version"], "2022-11-28");
        assert!(!headers.contains_key("open-sandbox-api-key"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["query"],
            "query { viewer { login } }"
        );
    }
    let exact = CredentialFrame {
        operation_id: "replace-github".into(),
        value: "exact-route-fixture-key".into(),
        custody_generation: Some(3),
        github_route: Some(layer3_adapter::GithubCredentialRoute {
            service_id: "repository-connection".into(),
            catalog_id: "github-catalog".into(),
            repository_scope: None,
            other_services: vec![layer3_adapter::PlatformServiceRoute {
                service_id: "skill-connection".into(),
                slug: "ornn-api".into(),
                catalog_id: None,
            }],
        }),
    };
    holder.install(&exact).await.unwrap();
    for (generation, id) in [(2, "repository-connection"), (3, "other-connection")] {
        let mut stale = exact.clone();
        stale.custody_generation = Some(generation);
        stale.github_route.as_mut().unwrap().service_id = id.into();
        assert!(holder.install(&stale).await.is_err());
    }
    let mut changed_catalog = exact.clone();
    changed_catalog.github_route.as_mut().unwrap().catalog_id = "other-catalog".into();
    assert!(holder.install(&changed_catalog).await.is_err());
    assert_eq!(
        client
            .get("http://api.github.com/user?per_page=1")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let (method, uri, headers, _) = received.recv().await.unwrap();
    assert_eq!(method, "GET");
    assert_eq!(
        uri,
        "/api/v1/proxy/github-catalog/user?per_page=1&_nyxid_via=repository-connection"
    );
    assert!(headers[header::AUTHORIZATION] == "Bearer exact-route-fixture-key");
    let platform_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let platform_addr = platform_listener.local_addr().unwrap();
    let platform_router = crate::upstream::router(
        crate::upstream_target::TargetBase::parse(&format!("http://{}", listener_addr)).unwrap(),
        holder.clone(),
        None,
    );
    let platform = tokio::spawn(async move {
        axum::serve(platform_listener, platform_router)
            .await
            .unwrap()
    });
    let platform_client = reqwest::Client::new();
    for (request, forwarded) in [
        (
            "/api/v1/proxy/s/api-github/user",
            "/api/v1/proxy/github-catalog/user?_nyxid_via=repository-connection",
        ),
        (
            "/proxy/github-catalog/user",
            "/api/v1/proxy/github-catalog/user?_nyxid_via=repository-connection",
        ),
        (
            "/api/v1/proxy/s/ornn-api/skills",
            "/api/v1/proxy/s/ornn-api/skills?_nyxid_via=skill-connection",
        ),
    ] {
        assert_eq!(
            platform_client
                .get(format!("http://{platform_addr}{request}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let (_, uri, headers, _) = received.recv().await.unwrap();
        assert_eq!(uri, forwarded);
        assert!(headers[header::AUTHORIZATION] == "Bearer exact-route-fixture-key");
    }
    for request in [
        "/api/v1/proxy/s/github-sibling/user",
        "/api/v1/proxy/s/api-github/user?_nyxid_via=sibling-connection",
        "/api/v1/proxy/sibling-connection/user",
    ] {
        assert_eq!(
            platform_client
                .get(format!("http://{platform_addr}{request}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    platform.abort();
    let _ = platform.await;
    assert_eq!(
        client
            .get("http://api.github.com/user?%5Fnyxid_via=other-connection")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    for url in [
        "http://untrusted.example/user",
        "http://uploads.github.com/user",
        "http://api.github.com:8080/user",
    ] {
        assert_eq!(
            client.get(url).send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    assert!(received.try_recv().is_err());
    upstream.abort();
    let _ = upstream.await;
}

#[tokio::test]
async fn github_writes_expose_the_upstream_decision_and_do_not_retry_a_lost_response() {
    use axum::http::Method;
    use std::time::Duration;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
    let (events, mut received) = mpsc::unbounded_channel();
    let upstream = tokio::spawn(async move {
        let app = axum::Router::new().fallback(any(move |request: axum::extract::Request| {
            let events = events.clone();
            async move {
                let (parts, body) = request.into_parts();
                let body = axum::body::to_bytes(body, 1024).await.unwrap();
                let outcome = match parts.uri.path().rsplit('/').next().unwrap() {
                    "user" => StatusCode::OK,
                    "blobs" => StatusCode::CREATED,
                    "1" => StatusCode::FORBIDDEN,
                    _ => StatusCode::GATEWAY_TIMEOUT,
                };
                events.send((parts, body)).unwrap();
                if outcome == StatusCode::GATEWAY_TIMEOUT {
                    // Leave the request unresolved after observing its payload.
                    std::future::pending::<()>().await;
                }
                (
                    outcome,
                    [("x-oauth-scopes", "public_repo, read:user")],
                    axum::Json(serde_json::json!({"message": "fixture decision"})),
                )
            }
        }));
        axum::serve(listener, app).await.unwrap();
    });
    let holder = CredentialHolder::new();
    holder
        .install(&CredentialFrame {
            operation_id: "github-write-contract".into(),
            value: "github-write-fixture-key".into(),
            custody_generation: Some(1),
            github_route: Some(layer3_adapter::GithubCredentialRoute {
                service_id: "repository-connection".into(),
                catalog_id: "github-catalog".into(),
                repository_scope: None,
                other_services: vec![],
            }),
        })
        .await
        .unwrap();
    PreparedGithubProxy::bind(&fixture.config)
        .await
        .unwrap()
        .start(holder);
    let client = reqwest::Client::builder()
        .unix_socket(fixture.config.socket())
        .timeout(Duration::from_secs(610))
        .build()
        .unwrap();
    let payload = serde_json::json!({"content": "fixture-write"});
    for (method, path, status) in [
        (Method::GET, "/user", StatusCode::OK),
        (
            Method::POST,
            "/repos/example/project/git/blobs",
            StatusCode::CREATED,
        ),
        (
            Method::PATCH,
            "/repos/example/project/pulls/1",
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut request = client.request(method.clone(), format!("http://api.github.com{path}"));
        if method != Method::GET {
            request = request.json(&payload);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers()["x-oauth-scopes"],
            "public_repo, read:user"
        );
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["message"],
            "fixture decision"
        );
        let (parts, body) = received.recv().await.unwrap();
        assert_eq!(parts.method, method);
        let approval_wait = parts.headers.get("x-nyxid-approval-wait-seconds");
        assert_eq!(approval_wait.is_none(), method == Method::GET);
        if let Some(wait) = approval_wait {
            assert_eq!(wait, "80");
        }
        assert!(!parts.headers.contains_key("x-nyxid-approval-mode"));
        assert_eq!(
            parts.uri.to_string(),
            format!("/api/v1/proxy/github-catalog{path}?_nyxid_via=repository-connection")
        );
        if method != Method::GET {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                payload
            );
        }
    }

    let delayed = tokio::spawn(async move {
        client
            .patch("http://api.github.com/repos/example/project/pulls/2")
            .json(&payload)
            .send()
            .await
            .unwrap()
    });
    let (parts, body) = tokio::time::timeout(Duration::from_secs(5), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parts.method, Method::PATCH);
    assert!(!body.is_empty());
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(100)).await;
    tokio::time::resume();
    let response = tokio::time::timeout(Duration::from_secs(5), delayed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    let problem: serde_json::Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "github_proxy_deadline");
    assert_eq!(problem["retryable"], false);
    assert_eq!(problem["connection_id"], "repository-connection");
    assert_eq!(problem["credential_generation"], 1);
    assert!(
        received.try_recv().is_err(),
        "the uncertain write must only reach the fixture once"
    );
    upstream.abort();
    let _ = upstream.await;
}

#[tokio::test]
async fn github_access_tracks_hot_grant_and_removal_without_restarting_transport() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
    let startup_environment = fixture.environment.clone();
    let (sent, mut received) = mpsc::channel(3);
    let upstream = tokio::spawn(async move {
        let app = axum::Router::new().fallback(any(move |headers: HeaderMap| {
            let sent = sent.clone();
            async move {
                let credential = headers[header::AUTHORIZATION].clone();
                let granted = credential == "Bearer scoped-fixture-generation-2";
                sent.send(credential).await.unwrap();
                if granted {
                    (StatusCode::OK, r#"{"login":"viewer"}"#)
                } else {
                    (
                        StatusCode::FORBIDDEN,
                        r#"{"message":"service not granted"}"#,
                    )
                }
            }
        }));
        axum::serve(listener, app).await.unwrap();
    });
    let holder = CredentialHolder::new();
    PreparedGithubProxy::bind(&fixture.config)
        .await
        .unwrap()
        .start(holder.clone());
    let client = fixture.client();
    for (generation, expected) in [
        (1, StatusCode::FORBIDDEN),
        (2, StatusCode::OK),
        (3, StatusCode::FORBIDDEN),
    ] {
        install(&holder, generation).await;
        let response = client
            .get("http://api.github.com/user")
            .bearer_auth(PLACEHOLDER)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(
            received.recv().await.unwrap(),
            format!("Bearer scoped-fixture-generation-{generation}")
        );
        assert_eq!(fixture.environment, startup_environment);
    }
    upstream.abort();
    let _ = upstream.await;
}

#[tokio::test]
async fn github_response_cannot_return_scoped_credentials() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
    let upstream = tokio::spawn(async move {
        let app = axum::Router::new().fallback(any(|headers: HeaderMap| async move {
            axum::response::Response::new(Body::from(
                headers[header::AUTHORIZATION].as_bytes().to_vec(),
            ))
        }));
        axum::serve(listener, app).await.unwrap();
    });
    let holder = CredentialHolder::new();
    install(&holder, 1).await;
    PreparedGithubProxy::bind(&fixture.config)
        .await
        .unwrap()
        .start(holder);
    let response = fixture
        .client()
        .get("http://api.github.com/user")
        .send()
        .await;
    // The fail-closed stream may abort before HTTP headers are flushed.
    if let Ok(response) = response {
        assert!(
            response.bytes().await.is_err(),
            "credential-bearing stream must fail closed"
        );
    }
    upstream.abort();
    let _ = upstream.await;
}

#[tokio::test]
#[ignore = "requires the pinned genuine gh executable"]
async fn genuine_cli_status_rest_and_graphql_share_the_current_grant() {
    let executable = std::env::var("CMA_GH_CONTRACT_EXECUTABLE").expect("explicit gh executable");
    let version = tokio::process::Command::new(&executable)
        .arg("--version")
        .output()
        .await
        .unwrap();
    let pinned = include_str!("../../../../api/crates/layer4-runtime/Dockerfile")
        .lines()
        .find_map(|line| line.strip_prefix("ARG GH_VERSION="))
        .expect("runtime GitHub CLI pin");
    assert!(String::from_utf8(version.stdout)
        .unwrap()
        .starts_with(&format!("gh version {pinned} ")));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
    let (sent, mut received) = mpsc::channel(6);
    let upstream = tokio::spawn(async move {
        let app = axum::Router::new().fallback(any(move |request: axum::extract::Request| {
            let sent = sent.clone();
            async move {
                let credential = request.headers()[header::AUTHORIZATION].clone();
                let granted = credential == "Bearer scoped-fixture-generation-2";
                sent.send((request.uri().path().to_string(), credential))
                    .await
                    .unwrap();
                if granted {
                    (
                        StatusCode::OK,
                        [("x-oauth-scopes", "public_repo, read:user")],
                        axum::Json(serde_json::json!({
                            "login":"viewer", "data":{"viewer":{"login":"viewer"}}
                        })),
                    )
                        .into_response()
                } else {
                    (
                        StatusCode::FORBIDDEN,
                        axum::Json(serde_json::json!({
                            "message":"service not granted"
                        })),
                    )
                        .into_response()
                }
            }
        }));
        axum::serve(listener, app).await.unwrap();
    });
    let holder = CredentialHolder::new();
    PreparedGithubProxy::bind(&fixture.config)
        .await
        .unwrap()
        .start(holder.clone());
    for generation in [1, 2, 3] {
        install(&holder, generation).await;
        for (path, args) in [
            ("user", vec!["api", "user", "--jq", ".login"]),
            (
                "graphql",
                vec![
                    "api",
                    "graphql",
                    "-f",
                    "query=query { viewer { login } }",
                    "--jq",
                    ".data.viewer.login",
                ],
            ),
        ] {
            let output = tokio::process::Command::new(&executable)
                .env_clear()
                .envs(&fixture.environment)
                .env("HOME", &fixture.root)
                .args(args)
                .output()
                .await
                .unwrap();
            assert_eq!(
                output.status.success(),
                generation == 2,
                "unexpected gh status: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if generation == 2 {
                assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "viewer");
            } else {
                assert!(String::from_utf8(output.stderr)
                    .unwrap()
                    .contains("HTTP 403"));
            }
            let (observed_path, credential) = received.recv().await.unwrap();
            assert_eq!(observed_path, format!("/api/v1/proxy/s/api-github/{path}"));
            assert_eq!(
                credential,
                format!("Bearer scoped-fixture-generation-{generation}")
            );
        }
        let status = tokio::process::Command::new(&executable)
            .env_clear()
            .envs(&fixture.environment)
            .env("HOME", &fixture.root)
            .args(["auth", "status", "--hostname", "github.com"])
            .output()
            .await
            .unwrap();
        assert_eq!(
            status.status.success(),
            generation == 2,
            "unexpected gh auth status: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        let expected_paths = if generation == 2 {
            assert!(String::from_utf8_lossy(&status.stdout).contains("account viewer"));
            &["graphql", ""][..]
        } else {
            &["graphql"][..]
        };
        for path in expected_paths {
            let (observed_path, credential) = received.try_recv().unwrap();
            assert_eq!(observed_path, format!("/api/v1/proxy/s/api-github/{path}"));
            assert_eq!(
                credential,
                format!("Bearer scoped-fixture-generation-{generation}")
            );
        }
        assert!(received.try_recv().is_err());
    }
    let output = tokio::process::Command::new(&executable)
        .env_clear()
        .envs(&fixture.environment)
        .env("HOME", &fixture.root)
        .args(["api", "https://untrusted.example/user"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert!(received.try_recv().is_err());
    upstream.abort();
    let _ = upstream.await;
}
