use std::{error::Error as _, fmt, sync::Arc, time::Duration};

use axum::{
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
        )
    } else if error.is_timeout() {
        failure(
            StatusCode::GATEWAY_TIMEOUT,
            "github_proxy_deadline",
            "The GitHub write received no HTTP response before its deadline. Read the remote state before retrying.",
            false,
            route,
            generation,
        )
    } else {
        failure(
            StatusCode::BAD_GATEWAY,
            "github_proxy_response_lost",
            "The GitHub write lost its HTTP response. Read the remote state before retrying.",
            false,
            route,
            generation,
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
) -> Response {
    (
        status,
        Json(json!({
            "code": code,
            "message": message,
            "retryable": retryable,
            "connection_id": route.map(|route| route.service_id.as_str()),
            "credential_generation": generation,
            "service": "api-github",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

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
