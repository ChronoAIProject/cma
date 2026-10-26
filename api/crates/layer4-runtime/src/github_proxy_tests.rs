use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
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
    let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
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
async fn genuine_cli_uses_managed_socket_for_rest_and_graphql() {
    let executable = std::env::var("CMA_GH_CONTRACT_EXECUTABLE").expect("explicit gh executable");
    let version = tokio::process::Command::new(&executable)
        .arg("--version")
        .output()
        .await
        .unwrap();
    let pinned = include_str!("../../../../infra/runtime/Dockerfile")
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
                        axum::Json(serde_json::json!({
                            "login":"viewer", "data":{"viewer":{"login":"viewer"}}
                        })),
                    )
                } else {
                    (
                        StatusCode::FORBIDDEN,
                        axum::Json(serde_json::json!({
                            "message":"service not granted"
                        })),
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
