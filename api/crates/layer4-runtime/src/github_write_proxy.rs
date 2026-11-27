use std::{error::Error as _, fmt, sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse as _, Response},
    Json,
};
use layer3_adapter::GithubCredentialRoute;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde_json::json;

const APPROVAL_WAIT: &str = "x-nyxid-approval-wait-seconds";
const DEFAULT_APPROVAL_SECONDS: u64 = 80;
const RESPONSE_MARGIN_SECONDS: u64 = 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DNS_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn is_write(
    host: Option<&str>,
    model_proxy: bool,
    method: &Method,
    uri: &Uri,
    route: Option<&GithubCredentialRoute>,
) -> bool {
    !method.is_safe()
        && !model_proxy
        && match host {
            Some("api.github.com") => true,
            None => {
                let path = uri.path();
                path.starts_with("/api/v1/proxy/s/api-github/")
                    || path.starts_with("/proxy/s/api-github/")
                    || route.is_some_and(|route| {
                        super::github_scope::is_selected_route(host, model_proxy, uri, route)
                    })
            }
            _ => false,
        }
}

pub(super) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(CONNECT_TIMEOUT)
        .dns_resolver(Arc::new(ProxyDns))
        .build()
        .expect("GitHub write proxy HTTP client builds")
}

pub(super) fn set_deadline(request: &mut reqwest::Request) {
    let value = request
        .headers_mut()
        .entry(APPROVAL_WAIT)
        // Keep approvals inside the connected request so the caller receives
        // the provider result or the approval refusal before the local deadline.
        .or_insert(HeaderValue::from_static("80"));
    let seconds = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (10..=300).contains(seconds))
        .unwrap_or(DEFAULT_APPROVAL_SECONDS);
    *request.timeout_mut() = Some(Duration::from_secs(seconds + RESPONSE_MARGIN_SECONDS));
}

#[derive(Debug)]
struct DnsFailure;

impl fmt::Display for DnsFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GitHub proxy DNS lookup failed")
    }
}

impl std::error::Error for DnsFailure {}

struct ProxyDns;

impl Resolve for ProxyDns {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addresses = tokio::time::timeout(
                DNS_TIMEOUT,
                tokio::net::lookup_host((name.as_str().to_owned(), 0)),
            )
            .await
            .map_err(|_| DnsFailure)?
            .map_err(|_| DnsFailure)?;
            Ok(Box::new(addresses) as Addrs)
        })
    }
}

pub(super) fn transport_error(
    error: &reqwest::Error,
    route: Option<&GithubCredentialRoute>,
    generation: Option<u64>,
) -> Response {
    let mut cause = error.source();
    while let Some(error) = cause {
        if error.is::<DnsFailure>() {
            return failure(
                StatusCode::BAD_GATEWAY,
                "github_proxy_dns",
                "DNS lookup for the GitHub connection proxy failed. Check service availability and retry.",
                true,
                route,
                generation,
                None,
            );
        }
        cause = error.source();
    }
    if error.is_connect() {
        failure(
            StatusCode::BAD_GATEWAY,
            "github_proxy_connect",
            "The GitHub connection proxy could not be reached. Check service availability and retry.",
            true,
            route,
            generation,
            None,
        )
    } else if error.is_timeout() {
        failure(
            StatusCode::GATEWAY_TIMEOUT,
            "github_proxy_deadline",
            "The GitHub write received no HTTP response before its deadline. Read the remote state before retrying.",
            false,
            route,
            generation,
            None,
        )
    } else {
        failure(
            StatusCode::BAD_GATEWAY,
            "github_proxy_response_lost",
            "The GitHub write lost its HTTP response. Read the remote state before retrying.",
            false,
            route,
            generation,
            None,
        )
    }
}

pub(super) fn failure(
    status: StatusCode,
    code: &str,
    message: &str,
    retryable: bool,
    route: Option<&GithubCredentialRoute>,
    generation: Option<u64>,
    upstream_status: Option<StatusCode>,
) -> Response {
    (
        status,
        Json(json!({
            "code": code,
            "message": message,
            "retryable": retryable,
            "connection_id": route.map(|route| route.service_id.as_str()),
            "credential_generation": generation,
            "upstream_http_status": upstream_status.map(|status| status.as_u16()),
            "service": "api-github",
        })),
    )
        .into_response()
}

pub(super) async fn complete_response(
    mut upstream: reqwest::Response,
    credential: &[u8],
    route: Option<&GithubCredentialRoute>,
    generation: Option<u64>,
) -> Response {
    let upstream_status = upstream.status();
    let problem = |status, code, message| {
        failure(
            status,
            code,
            message,
            false,
            route,
            generation,
            Some(upstream_status),
        )
    };
    let too_large = || {
        problem(
        StatusCode::BAD_GATEWAY,
        "github_proxy_response_too_large",
        "The GitHub write response exceeded the proxy limit. Read the remote state before retrying.",
    )
    };
    if upstream
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
    {
        return too_large();
    }

    // Wait for the complete response before returning the provider's status.
    let mut body = Vec::new();
    loop {
        match upstream.chunk().await {
            Ok(Some(chunk)) if chunk.len() <= MAX_RESPONSE_BYTES - body.len() => {
                body.extend_from_slice(&chunk);
            }
            Ok(Some(_)) => return too_large(),
            Ok(None) => break,
            Err(error) if error.is_timeout() => return problem(
                StatusCode::GATEWAY_TIMEOUT,
                "github_proxy_response_body_deadline",
                "The GitHub write response body did not finish before the deadline. Read the remote state before retrying.",
            ),
            Err(_) => return problem(
                StatusCode::BAD_GATEWAY,
                "github_proxy_response_body_lost",
                "The GitHub write response body ended prematurely. Read the remote state before retrying.",
            ),
        }
    }
    if crate::upstream_guard::contains(&body, credential) {
        tracing::error!("upstream response contained protected credential material");
        return problem(
            StatusCode::BAD_GATEWAY,
            "github_proxy_response_refused",
            "The GitHub write response was refused at the credential boundary. Read the remote state before retrying.",
        );
    }

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = upstream_status;
    super::copy_response_headers(upstream.headers(), response.headers_mut());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_body_deadline_preserves_the_observed_upstream_status() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(
                    b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\nContent-Type: application/json\r\n\r\n{",
                )
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let response = client()
            .post(format!("http://{address}/write"))
            .timeout(Duration::from_secs(1))
            .send()
            .await
            .unwrap();
        tokio::time::pause();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            complete_response(response, b"synthetic-fixture", None, Some(1)),
        )
        .await;
        tokio::time::resume();
        upstream.abort();
        let result = result.unwrap();
        assert_eq!(result.status(), StatusCode::GATEWAY_TIMEOUT);
        let body = axum::body::to_bytes(result.into_body(), 4096)
            .await
            .unwrap();
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "github_proxy_response_body_deadline");
        assert_eq!(problem["upstream_http_status"], 201);
        assert_eq!(problem["retryable"], false);
    }

    #[tokio::test]
    async fn transport_errors_distinguish_dns_and_connection_refusal() {
        struct FailedLookup;

        impl Resolve for FailedLookup {
            fn resolve(&self, _name: Name) -> Resolving {
                Box::pin(async {
                    Err(Box::new(DnsFailure) as Box<dyn std::error::Error + Send + Sync>)
                })
            }
        }

        let client = reqwest::Client::builder()
            .no_proxy()
            .dns_resolver(Arc::new(FailedLookup))
            .build()
            .unwrap();
        let dns = client
            .post("http://github-proxy-fixture.invalid/write")
            .send()
            .await
            .unwrap_err();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let connect = client
            .post(format!("http://{address}/write"))
            .send()
            .await
            .unwrap_err();

        for (error, code) in [(dns, "github_proxy_dns"), (connect, "github_proxy_connect")] {
            let response = transport_error(&error, None, Some(1));
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(problem["code"], code);
            assert_eq!(problem["retryable"], true);
            assert_eq!(problem["connection_id"], serde_json::Value::Null);
            assert!(!problem["message"].as_str().unwrap().contains(".invalid"));
        }
    }
}
