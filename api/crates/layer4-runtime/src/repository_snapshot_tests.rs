use super::*;
use axum::{extract::Request, response::IntoResponse, routing::get, Json, Router};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::instrument::WithSubscriber;

#[path = "repository_snapshot_tests/response_hint.rs"]
mod response_hint;

const PRIVATE_BODY: &str = "PRIVATE_RESPONSE_SENTINEL credential=PRIVATE_CREDENTIAL_SENTINEL";

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
    request: WorkspaceRequest,
    reads: Arc<Mutex<Vec<String>>>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        let root =
            PathBuf::from("/tmp").join(format!("cma-source-{}", uuid::Uuid::new_v4().simple()));
        let mut config = RuntimeConfig::for_test(root.join("home"));
        config
            .process_env
            .insert("PRIVATE_ENV".into(), "PRIVATE_ENV_SENTINEL".into());
        std::fs::create_dir_all(&config.home).unwrap();
        let request = WorkspaceRequest::new(
            "PRIVATE_OPERATION_SENTINEL",
            "private-workspace",
            "checkout",
            WorkspaceSource::RepositorySnapshot {
                url: "https://github.com/PRIVATE_OWNER_SENTINEL/PRIVATE_REPOSITORY_SENTINEL".into(),
                reference: Some("PRIVATE_REF_SENTINEL".into()),
            },
        );
        Self {
            root,
            config,
            request,
            reads: Default::default(),
            server: None,
        }
    }

    fn proxy(&mut self) -> tokio::net::UnixListener {
        let proxy = crate::github_proxy::configuration(
            &self.root,
            &mut Default::default(),
            Some(&crate::config::ProxyConfig {
                role: "platform",
                bind: "127.0.0.1:9092".parse().unwrap(),
                upstream: "http://127.0.0.1:9093".into(),
            }),
        )
        .unwrap()
        .unwrap();
        std::fs::create_dir_all(proxy.socket().parent().unwrap()).unwrap();
        let listener = tokio::net::UnixListener::bind(proxy.socket()).unwrap();
        self.config.github_proxy = Some(proxy);
        listener
    }

    fn serve(&mut self, failure: Option<(&'static str, u16)>) {
        self.serve_response(failure.map(|(stage, _)| stage), move || {
            (
                axum::http::StatusCode::from_u16(failure.unwrap().1).unwrap(),
                [("x-private", "PRIVATE_HEADER_SENTINEL")],
                PRIVATE_BODY,
            )
                .into_response()
        });
    }

    fn serve_response(
        &mut self,
        failed_stage: Option<&'static str>,
        failure: impl Fn() -> axum::response::Response + Clone + Send + Sync + 'static,
    ) {
        let listener = self.proxy();
        let reads = self.reads.clone();
        let app = Router::new().fallback(get(move |request: Request| {
            let stage = if request.uri().path().contains("/zipball/") {
                "archive"
            } else if request.uri().path().ends_with("/commits") {
                "commits"
            } else {
                "metadata"
            };
            reads.lock().unwrap().push(stage.into());
            let failure = failure.clone();
            async move {
                if failed_stage == Some(stage) {
                    return failure();
                }
                match stage {
                    "metadata" => {
                        Json(serde_json::json!({"default_branch":"main"})).into_response()
                    }
                    "commits" => Json(serde_json::json!([{"sha":"a".repeat(40)}])).into_response(),
                    _ => {
                        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                        zip.start_file(
                            "source/retained-work",
                            zip::write::SimpleFileOptions::default(),
                        )
                        .unwrap();
                        zip.write_all(b"original source").unwrap();
                        zip.finish().unwrap().into_inner().into_response()
                    }
                }
            }
        }));
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);
impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Log {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync {
        let output = self.clone();
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || output.clone())
            .finish()
    }
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
    fn failure(&self, stage: &str, status: Option<u16>, class: &str, proxy: bool) -> String {
        let text = self.text();
        assert_eq!(text.lines().count(), 1, "one safe failure event: {text}");
        assert!(text.len() < 512, "bounded event: {text}");
        for field in [
            format!("stage=\"{stage}\""),
            format!("error_class=\"{class}\""),
            format!("proxy_present={proxy}"),
        ] {
            assert!(text.contains(&field), "missing {field}: {text}");
        }
        if let Some(status) = status {
            assert!(text.contains(&format!("http_status={status}")), "{text}");
        } else {
            assert!(!text.contains("http_status="), "{text}");
        }
        assert!(!text.contains("PRIVATE_"), "{text}");
        assert!(
            !text.contains("http://") && !text.contains("https://") && !text.contains("/tmp/"),
            "{text}"
        );
        let reference = text
            .split("operation_ref=")
            .nth(1)
            .expect("opaque operation reference")
            .split_whitespace()
            .next()
            .unwrap()
            .trim_matches('"');
        assert_eq!(reference.len(), 64);
        assert!(reference
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        reference.into()
    }
}

#[tokio::test]
async fn missing_proxy_has_distinct_safe_diagnostics_and_unchanged_public_error() {
    let f = Fixture::new();
    let log = Log::default();
    let error = prepare(&f.config, &f.config.home, &f.request)
        .with_subscriber(log.subscriber())
        .await
        .unwrap_err();
    assert_eq!(error.code, "repository_connection_required");
    assert_eq!(
        error.detail,
        "GitHub connection is required before repository preparation"
    );
    assert!(!error.retryable);
    assert!(!f.config.home.join("checkout").exists());
    log.failure("proxy_setup", None, "proxy_missing", false);
}

async fn refused(stage: &'static str) {
    let mut reference = None;
    for status in [401, 403, 404, 409, 422, 429, 503] {
        let mut f = Fixture::new();
        f.serve(Some((stage, status)));
        let log = Log::default();
        let error = prepare(&f.config, &f.config.home, &f.request)
            .with_subscriber(log.subscriber())
            .await
            .unwrap_err();
        let (code, detail, retryable, class) = match status {
            401 | 403 => (
                "repository_connection_required",
                "GitHub authorization is missing or does not allow this repository",
                false,
                "http_refused",
            ),
            404 | 409 | 422 => (
                match stage {
                    "metadata" => "repository_unavailable",
                    "commits" => "repository_ref_not_found",
                    _ => "repository_archive_unavailable",
                },
                if stage == "commits" {
                    "The requested repository ref could not be found"
                } else {
                    "The repository is unavailable or inaccessible to the connected account"
                },
                false,
                "http_refused",
            ),
            _ => (
                "repository_source_unavailable",
                "GitHub source preparation is temporarily unavailable",
                true,
                "http_unavailable",
            ),
        };
        assert_eq!(
            (error.code.as_str(), error.detail.as_str(), error.retryable),
            (code, detail, retryable)
        );
        let observed = log.failure(stage, Some(status), class, true);
        if let Some(reference) = &reference {
            assert_eq!(&observed, reference);
        }
        reference = Some(observed);
        let expected = match stage {
            "metadata" => vec!["metadata"],
            "commits" => vec!["metadata", "commits"],
            _ => vec!["metadata", "commits", "archive"],
        };
        assert_eq!(*f.reads.lock().unwrap(), expected);
        let path = f.config.home.join("checkout");
        if stage == "archive" {
            assert_eq!(
                std::fs::read_to_string(path.join(".cma-source-pin")).unwrap(),
                "a".repeat(40)
            );
            assert!(!path.join(".git").exists());
        } else {
            assert!(!path.exists());
        }
    }
}

#[tokio::test]
async fn metadata_refusals_are_observed_without_body_or_identity() {
    refused("metadata").await;
}
#[tokio::test]
async fn commit_refusals_are_observed_without_body_or_identity() {
    refused("commits").await;
}
#[tokio::test]
async fn archive_refusals_keep_the_original_source_pin_and_safe_diagnostics() {
    refused("archive").await;
}

#[tokio::test]
async fn disconnected_proxy_is_transport_not_an_authorization_refusal() {
    let mut f = Fixture::new();
    drop(f.proxy());
    let log = Log::default();
    let error = prepare(&f.config, &f.config.home, &f.request)
        .with_subscriber(log.subscriber())
        .await
        .unwrap_err();
    assert_eq!(error.code, "repository_source_unavailable");
    assert!(error.retryable);
    log.failure("metadata", None, "transport", true);
}

#[tokio::test(start_paused = true)]
async fn request_timeout_is_observed_without_guessing_the_refusal_source() {
    let mut f = Fixture::new();
    let listener = f.proxy();
    let accepted = Arc::new(tokio::sync::Notify::new());
    let signal = accepted.clone();
    let app = Router::new().fallback(get(move || {
        signal.notify_one();
        async { std::future::pending::<&'static str>().await }
    }));
    f.server = Some(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    let log = Log::default();
    let config = f.config.clone();
    let request = f.request.clone();
    let task = tokio::spawn(
        async move { prepare(&config, &config.home, &request).await }
            .with_subscriber(log.subscriber()),
    );
    accepted.notified().await;
    tokio::time::advance(Duration::from_secs(121)).await;
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.code, "repository_source_unavailable");
    assert!(error.retryable);
    log.failure("metadata", None, "timeout", true);
}

#[tokio::test]
async fn success_and_pinned_recovery_keep_identity_and_do_not_log_failure() {
    for pinned in [false, true] {
        let mut f = Fixture::new();
        if !pinned {
            f.request.source = WorkspaceSource::RepositorySnapshot {
                url: "https://github.com/PRIVATE_OWNER_SENTINEL/PRIVATE_REPOSITORY_SENTINEL".into(),
                reference: None,
            };
        }
        f.serve(None);
        let log = Log::default();
        let receipt = async {
            if pinned {
                prepare_pinned(&f.config, &f.config.home, &f.request, &"a".repeat(40)).await
            } else {
                prepare(&f.config, &f.config.home, &f.request).await
            }
        }
        .with_subscriber(log.subscriber())
        .await
        .unwrap();
        assert!(log.text().is_empty(), "{}", log.text());
        assert_eq!(receipt.operation_id, f.request.operation_id);
        assert_eq!(receipt.request_sha256, f.request.request_sha256);
        assert_eq!(receipt.workspace_id, f.request.workspace_id);
        assert_eq!(
            receipt.source_commit.as_deref(),
            Some("a".repeat(40).as_str())
        );
        assert_eq!(
            source_commit(Path::new(&receipt.path), &f.request.source).unwrap(),
            receipt.source_commit
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&receipt.path).join("retained-work")).unwrap(),
            "original source"
        );
        assert_eq!(
            *f.reads.lock().unwrap(),
            if pinned {
                vec!["archive"]
            } else {
                vec!["metadata", "commits", "archive"]
            }
        );
    }
}

#[tokio::test]
async fn body_transport_failure_retains_received_status_without_logging_the_payload() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut f = Fixture::new();
    let listener = f.proxy();
    f.server = Some(tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\nPRIVATE_RESPONSE_SENTINEL")
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
    }));
    let log = Log::default();
    let error = prepare(&f.config, &f.config.home, &f.request)
        .with_subscriber(log.subscriber())
        .await
        .unwrap_err();
    assert_eq!(error.code, "repository_source_unavailable");
    assert!(error.retryable);
    log.failure("metadata", Some(200), "transport", true);
}

#[tokio::test]
async fn pinned_archive_refusal_never_resolves_a_new_ref_and_references_are_operation_scoped() {
    let mut references = Vec::new();
    for operation in [
        "PRIVATE_OPERATION_SENTINEL",
        "PRIVATE_OTHER_OPERATION_SENTINEL",
    ] {
        let mut f = Fixture::new();
        f.request = WorkspaceRequest::new(
            operation,
            "private-workspace",
            "checkout",
            f.request.source.clone(),
        );
        f.serve(Some(("archive", 403)));
        let log = Log::default();
        let error = prepare_pinned(&f.config, &f.config.home, &f.request, &"b".repeat(40))
            .with_subscriber(log.subscriber())
            .await
            .unwrap_err();
        assert_eq!(error.code, "repository_connection_required");
        assert!(!error.retryable);
        references.push(log.failure("archive", Some(403), "http_refused", true));
        assert_eq!(*f.reads.lock().unwrap(), ["archive"]);
        assert_eq!(
            std::fs::read_to_string(f.config.home.join("checkout/.cma-source-pin")).unwrap(),
            "b".repeat(40)
        );
    }
    assert_ne!(references[0], references[1]);
}

#[tokio::test]
async fn github_scope_refuses_revoked_push_and_archived_repositories_before_archive_read() {
    use layer3_adapter::{
        CredentialFrame, GithubCredentialRoute, GithubRepositoryScope, GithubRepositoryScopeNeed,
    };
    let repository = "example/project";
    for (push, archived) in [(false, false), (true, true)] {
        let mut f = Fixture::new();
        f.request = WorkspaceRequest::new(
            "prepare",
            "workspace",
            "checkout",
            WorkspaceSource::RepositorySnapshot {
                url: format!("https://github.com/{repository}"),
                reference: Some("a".repeat(40)),
            },
        )
        .with_repository_connection(Some("selected".into()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let reads = f.reads.clone();
        let app = Router::new().fallback(get(move |request: Request| {
            assert_eq!(request.uri().query(), Some("_nyxid_via=selected"));
            let path = request.uri().path().to_owned();
            reads.lock().unwrap().push(path.clone());
            async move {
                let metadata = match path.as_str() {
                    "/api/v1/proxy/catalog/repos/example/project" => serde_json::json!({
                        "full_name": repository, "private": false, "disabled": false, "archived": archived,
                        "permissions": {"push":push}, "default_branch":"main",
                    }),
                    _ => serde_json::json!([{"sha":"a".repeat(40)}]),
                };
                ([("x-oauth-scopes", "public_repo")], Json(metadata))
            }
        }));
        f.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        let proxy = crate::github_proxy::configuration(
            &f.root,
            &mut Default::default(),
            Some(&crate::config::ProxyConfig {
                role: "platform",
                bind: "127.0.0.1:9092".parse().unwrap(),
                upstream,
            }),
        )
        .unwrap()
        .unwrap();
        let holder = crate::upstream::CredentialHolder::new();
        holder
            .install(&CredentialFrame {
                operation_id: "scope-generation-2".into(),
                value: "synthetic-scoped-value".into(),
                custody_generation: Some(2),
                github_route: Some(GithubCredentialRoute {
                    service_id: "selected".into(),
                    catalog_id: "catalog".into(),
                    other_services: vec![],
                    repository_scope: Some(GithubRepositoryScope {
                        repository: repository.into(),
                        required_scope: GithubRepositoryScopeNeed::PublicRepo,
                        manage_url: "https://nyx.example/keys/selected".into(),
                    }),
                }),
            })
            .await
            .unwrap();
        crate::github_proxy::PreparedGithubProxy::bind(&proxy)
            .await
            .unwrap()
            .start(holder);
        f.config.github_proxy = Some(proxy);
        let error = prepare(&f.config, &f.config.home, &f.request)
            .await
            .unwrap_err();
        assert_eq!(error.code, "repository_connection_required");
        assert_eq!(
            *f.reads.lock().unwrap(),
            ["/api/v1/proxy/catalog/repos/example/project"]
        );
        assert!(!f.config.home.join("checkout").exists());
    }
}
