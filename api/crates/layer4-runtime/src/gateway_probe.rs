//! Versioned proof that a runtime's command sandbox can reach its local gateway.
//!
//! The supervisor launches a fixed helper through the kernel's configured
//! command-sandbox entry point. The helper performs one numeric resource read,
//! discards the entire response, and emits only a closed outcome. Provider
//! bodies, headers, and credentials never cross this protocol.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::upstream::{
    GATEWAY_OUTCOME_BROKER_TRANSIENT, GATEWAY_OUTCOME_HEADER, GATEWAY_OUTCOME_POLICY_DENIED,
    RUNTIME_GATEWAY_PROBE_HEADER, RUNTIME_GATEWAY_PROBE_HEADER_VALUE,
};

pub const NAME: &str = "runtime_gateway_probe_v2";
pub const VERSION: u32 = 2;
pub const COMMAND_ENV: &str = "CHRONO_RUNTIME_GATEWAY_PROBE_COMMAND";
const HELPER: &str = "runtime-gateway-probe-helper-v2";
pub const TRIGGER_PLATFORM_PROXY_BIND: &str = "127.0.0.2:9092";
pub const TRIGGER_SUPERVISOR_DENIAL_BIND: &str = "127.0.0.2:9090";
pub const TRIGGER_MODEL_PROXY_DENIAL_BIND: &str = "127.0.0.2:9091";
const PLATFORM_GATEWAY: &str = "http://127.0.0.2:9092";
const PRIVATE_DENIAL_PROBE: &str = "http://127.0.0.1:9092/livez";
const PUBLIC_DENIAL_PROBE: &str = "http://example.com";
const SUPERVISOR_ISOLATION_PROBE: &str = "http://127.0.0.2:9090/readyz";
const MODEL_PROXY_ISOLATION_PROBE: &str = "http://127.0.0.2:9091/v1/models";
const PROXY_DENIAL_HEADER: &str = "x-proxy-error";
const PROXY_DENIAL_VALUE: &str = "blocked-by-allowlist";
pub const ISOLATION_DENIAL_HEADER: &str = "x-cma-trigger-loopback-isolation";
pub const ISOLATION_DENIAL_VALUE: &str = "denied";
const PROBE_DEADLINE: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct GatewayProbe {
    command: Option<Arc<Vec<String>>>,
    workspace: PathBuf,
}

impl GatewayProbe {
    pub fn from_environment(workspace: PathBuf) -> Result<Self, String> {
        let Some(raw) = std::env::var(COMMAND_ENV)
            .ok()
            .filter(|raw| !raw.trim().is_empty())
        else {
            return Ok(Self {
                command: None,
                workspace,
            });
        };
        let command: Vec<String> = serde_json::from_str(&raw)
            .map_err(|_| format!("{COMMAND_ENV} must be a JSON string array"))?;
        if command.is_empty()
            || command.len() > 16
            || command.iter().any(|part| {
                part.is_empty() || part.len() > 1_024 || part.as_bytes().contains(&b'\0')
            })
            || command.last().map(String::as_str) != Some(HELPER)
        {
            return Err(format!(
                "{COMMAND_ENV} must be a bounded command ending in {HELPER}"
            ));
        }
        Ok(Self {
            command: Some(Arc::new(command)),
            workspace,
        })
    }

    #[cfg(test)]
    pub fn disabled(workspace: PathBuf) -> Self {
        Self {
            command: None,
            workspace,
        }
    }

    #[cfg(test)]
    fn with_command(workspace: PathBuf, command: Vec<String>) -> Self {
        Self {
            command: Some(Arc::new(command)),
            workspace,
        }
    }

    pub fn supported(&self) -> bool {
        self.command.is_some()
    }

    pub async fn run(&self, request: ProbeRequest) -> ProbeReceipt {
        if request.version != VERSION
            || request.resource_id == 0
            || !valid_service_id(&request.service_id)
            || !valid_service_id(&request.catalog_service_id)
            || !valid_user_service_slug(&request.user_service_slug)
        {
            return ProbeReceipt::new(ProbeOutcome::PolicyDenied);
        }
        let Some(command) = self.command.as_ref() else {
            return ProbeReceipt::new(ProbeOutcome::CommandSandboxInitializationFailed);
        };
        let mut child = tokio::process::Command::new(&command[0]);
        child
            .args(&command[1..])
            .arg(&request.catalog_service_id)
            .arg(&request.service_id)
            .arg(&request.user_service_slug)
            .arg(request.resource_id.to_string())
            .current_dir(&self.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // A command sandbox can include paths or command text in its
            // diagnostics. The typed outcome is sufficient and is all CMA
            // may retain, so diagnostic output is deliberately discarded.
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let spawn = crate::writer_fence::ManagedSpawnPermit::begin();
        let child = match child.spawn() {
            Ok(child) => child,
            Err(_) => return ProbeReceipt::new(ProbeOutcome::CommandSandboxInitializationFailed),
        };
        let managed_child = spawn.register(child.id());
        let output = match tokio::time::timeout(PROBE_DEADLINE, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(_)) | Err(_) => {
                return ProbeReceipt::new(ProbeOutcome::CommandSandboxInitializationFailed)
            }
        };
        drop(managed_child);
        if !output.status.success() {
            return ProbeReceipt::new(ProbeOutcome::CommandSandboxInitializationFailed);
        }
        serde_json::from_slice::<ProbeReceipt>(&output.stdout)
            .ok()
            .filter(|receipt| receipt.version == VERSION)
            .unwrap_or_else(|| ProbeReceipt::new(ProbeOutcome::CommandSandboxInitializationFailed))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeRequest {
    pub version: u32,
    pub catalog_service_id: String,
    pub service_id: String,
    pub user_service_slug: String,
    pub resource_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeReceipt {
    pub version: u32,
    pub outcome: ProbeOutcome,
}

impl ProbeReceipt {
    fn new(outcome: ProbeOutcome) -> Self {
        Self {
            version: VERSION,
            outcome,
        }
    }

    pub(crate) fn policy_denied() -> Self {
        Self::new(ProbeOutcome::PolicyDenied)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    Allowed,
    CommandSandboxInitializationFailed,
    GatewayNetworkInitializationFailed,
    PolicyDenied,
    ProviderDenied,
    TransientBrokerFailure,
}

/// Run the fixed helper subprocess, if this invocation selected it.
pub async fn run_helper_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some(HELPER) {
        return None;
    }
    let Some(catalog_service_id) = args.get(2).filter(|value| valid_service_id(value)) else {
        return Some(2);
    };
    let Some(service_id) = args.get(3).filter(|value| valid_service_id(value)) else {
        return Some(2);
    };
    let Some(_user_service_slug) = args.get(4).filter(|value| valid_user_service_slug(value))
    else {
        return Some(2);
    };
    let Some(resource_id) = args.get(5).and_then(|value| value.parse::<u64>().ok()) else {
        return Some(2);
    };
    if args.len() != 6 || resource_id == 0 {
        return Some(2);
    }
    let outcome = probe_gateway(catalog_service_id, service_id, resource_id).await;
    let receipt = ProbeReceipt::new(outcome);
    match serde_json::to_string(&receipt) {
        Ok(encoded) => {
            println!("{encoded}");
            Some(0)
        }
        Err(_) => Some(2),
    }
}

async fn probe_gateway(
    catalog_service_id: &str,
    service_id: &str,
    resource_id: u64,
) -> ProbeOutcome {
    probe_gateway_at(
        ProbeBinding {
            catalog_service_id,
            service_id,
            resource_id,
        },
        ProbeEndpoints {
            private_denial_probe: PRIVATE_DENIAL_PROBE,
            public_denial_probe: PUBLIC_DENIAL_PROBE,
            supervisor_isolation_probe: SUPERVISOR_ISOLATION_PROBE,
            model_proxy_isolation_probe: MODEL_PROXY_ISOLATION_PROBE,
            platform_gateway: PLATFORM_GATEWAY,
        },
    )
    .await
}

#[derive(Clone, Copy)]
struct ProbeBinding<'a> {
    catalog_service_id: &'a str,
    service_id: &'a str,
    resource_id: u64,
}

#[derive(Clone, Copy)]
struct ProbeEndpoints<'a> {
    private_denial_probe: &'a str,
    public_denial_probe: &'a str,
    supervisor_isolation_probe: &'a str,
    model_proxy_isolation_probe: &'a str,
    platform_gateway: &'a str,
}

async fn probe_gateway_at(
    binding: ProbeBinding<'_>,
    endpoints: ProbeEndpoints<'_>,
) -> ProbeOutcome {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => return ProbeOutcome::GatewayNetworkInitializationFailed,
    };
    // A successful allowed call alone cannot prove least privilege: an
    // unrestricted sandbox can make it too. Require the pinned Codex managed
    // proxy to positively deny both an unlisted private alias and a public
    // destination before testing the one allowlisted loopback literal. Bodies
    // are never read; only the proxy's closed denial header is observed.
    for denied in [
        endpoints.private_denial_probe,
        endpoints.public_denial_probe,
    ] {
        if !managed_proxy_denies(&client, denied).await {
            return ProbeOutcome::GatewayNetworkInitializationFailed;
        }
    }
    for isolated in [
        endpoints.supervisor_isolation_probe,
        endpoints.model_proxy_isolation_probe,
    ] {
        if !isolation_front_denies(&client, isolated).await {
            return ProbeOutcome::GatewayNetworkInitializationFailed;
        }
    }
    let url = format!(
        "{}/api/v1/proxy/{}/repositories/{}?_nyxid_via={}",
        endpoints.platform_gateway,
        binding.catalog_service_id,
        binding.resource_id,
        binding.service_id,
    );
    let response = match client
        .get(url)
        .header(
            RUNTIME_GATEWAY_PROBE_HEADER,
            RUNTIME_GATEWAY_PROBE_HEADER_VALUE,
        )
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return ProbeOutcome::GatewayNetworkInitializationFailed,
    };
    let gateway_outcome = response
        .headers()
        .get(GATEWAY_OUTCOME_HEADER)
        .and_then(|value| value.to_str().ok());
    if gateway_outcome == Some(GATEWAY_OUTCOME_POLICY_DENIED) {
        return ProbeOutcome::PolicyDenied;
    }
    if gateway_outcome == Some(GATEWAY_OUTCOME_BROKER_TRANSIENT) {
        return ProbeOutcome::TransientBrokerFailure;
    }
    let status = response.status();
    if status.is_success() {
        ProbeOutcome::Allowed
    } else if status.as_u16() == 408
        || status.as_u16() == 425
        || status.as_u16() == 429
        || status.is_server_error()
    {
        ProbeOutcome::TransientBrokerFailure
    } else {
        ProbeOutcome::ProviderDenied
    }
}

async fn managed_proxy_denies(client: &reqwest::Client, url: &str) -> bool {
    let Ok(response) = client.get(url).send().await else {
        return false;
    };
    response.status() == reqwest::StatusCode::FORBIDDEN
        && response
            .headers()
            .get(PROXY_DENIAL_HEADER)
            .is_some_and(|value| value == PROXY_DENIAL_VALUE)
}

async fn isolation_front_denies(client: &reqwest::Client, url: &str) -> bool {
    let Ok(response) = client.get(url).send().await else {
        return false;
    };
    response.status() == reqwest::StatusCode::FORBIDDEN
        && response
            .headers()
            .get(ISOLATION_DENIAL_HEADER)
            .is_some_and(|value| value == ISOLATION_DENIAL_VALUE)
}

fn valid_service_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_user_service_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use axum::extract::State;
    use axum::response::IntoResponse;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const CATALOG_SERVICE: &str = "catalog-service-github";

    struct ResponseFixture {
        base: String,
        requests: tokio::task::JoinHandle<Vec<String>>,
    }

    async fn response_fixture(
        responses: Vec<(u16, Vec<(&'static str, &'static str)>)>,
    ) -> ResponseFixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the gateway fixture binds");
        let address = listener.local_addr().expect("the fixture has an address");
        let requests = tokio::spawn(async move {
            let mut observed = Vec::new();
            for (status, headers) in responses {
                let (mut stream, _) = listener.accept().await.expect("the fixture accepts");
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut chunk = [0_u8; 1_024];
                    let read = stream.read(&mut chunk).await.expect("the fixture reads");
                    assert!(read > 0, "the request contains complete headers");
                    request.extend_from_slice(&chunk[..read]);
                }
                observed.push(
                    String::from_utf8_lossy(&request)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
                let mut response =
                    format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n");
                for (name, value) in headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("the fixture responds");
                stream.shutdown().await.expect("the fixture closes");
            }
            observed
        });
        ResponseFixture {
            base: format!("http://{address}"),
            requests,
        }
    }

    fn denial_headers() -> Vec<(&'static str, &'static str)> {
        vec![(PROXY_DENIAL_HEADER, PROXY_DENIAL_VALUE)]
    }

    fn isolation_headers() -> Vec<(&'static str, &'static str)> {
        vec![(ISOLATION_DENIAL_HEADER, ISOLATION_DENIAL_VALUE)]
    }

    #[test]
    fn service_ids_are_bounded_and_url_safe() {
        assert!(valid_service_id("service-1.a_b"));
        for value in ["", "service/1", "service?credential=value", "service 1"] {
            assert!(!valid_service_id(value));
        }
        assert!(valid_user_service_slug("api-github--2"));
        for value in ["", "-api-github", "api-github-", "Api_GitHub"] {
            assert!(!valid_user_service_slug(value));
        }
    }

    #[tokio::test]
    async fn a_disabled_probe_returns_only_the_closed_sandbox_failure() {
        let receipt = GatewayProbe::disabled(PathBuf::from("/tmp"))
            .run(ProbeRequest {
                version: VERSION,
                catalog_service_id: CATALOG_SERVICE.into(),
                service_id: "service-1".into(),
                user_service_slug: "api-github-2".into(),
                resource_id: 30,
            })
            .await;
        assert_eq!(
            receipt,
            ProbeReceipt::new(ProbeOutcome::CommandSandboxInitializationFailed)
        );
        let rendered = format!("{receipt:?}");
        assert!(!rendered.contains("credential") && !rendered.contains("provider body"));
    }

    #[tokio::test]
    async fn malformed_requests_and_command_failure_return_only_closed_outcomes() {
        let command = vec![
            "/bin/sh".into(),
            "-c".into(),
            "exit 9".into(),
            HELPER.into(),
        ];
        let probe = GatewayProbe::with_command(PathBuf::from("/tmp"), command);
        let failed = probe
            .run(ProbeRequest {
                version: VERSION,
                catalog_service_id: CATALOG_SERVICE.into(),
                service_id: "service-1".into(),
                user_service_slug: "api-github-2".into(),
                resource_id: 30,
            })
            .await;
        assert_eq!(
            failed,
            ProbeReceipt::new(ProbeOutcome::CommandSandboxInitializationFailed)
        );
        for request in [
            ProbeRequest {
                version: VERSION + 1,
                catalog_service_id: CATALOG_SERVICE.into(),
                service_id: "service-1".into(),
                user_service_slug: "api-github-2".into(),
                resource_id: 30,
            },
            ProbeRequest {
                version: VERSION,
                catalog_service_id: CATALOG_SERVICE.into(),
                service_id: "service/credential=secret".into(),
                user_service_slug: "api-github-2".into(),
                resource_id: 30,
            },
            ProbeRequest {
                version: VERSION,
                catalog_service_id: "catalog/credential=secret".into(),
                service_id: "service-1".into(),
                user_service_slug: "api-github-2".into(),
                resource_id: 30,
            },
            ProbeRequest {
                version: VERSION,
                catalog_service_id: CATALOG_SERVICE.into(),
                service_id: "service-1".into(),
                user_service_slug: "Api_GitHub".into(),
                resource_id: 30,
            },
            ProbeRequest {
                version: VERSION,
                catalog_service_id: CATALOG_SERVICE.into(),
                service_id: "service-1".into(),
                user_service_slug: "api-github-2".into(),
                resource_id: 0,
            },
        ] {
            assert_eq!(
                probe.run(request).await,
                ProbeReceipt::new(ProbeOutcome::PolicyDenied)
            );
        }
        let rendered = format!("{failed:?}");
        assert!(!rendered.contains("exit 9") && !rendered.contains("credential=secret"));
    }

    #[tokio::test]
    async fn both_private_alias_and_public_destinations_must_be_denied_by_the_managed_proxy() {
        let private_not_denied = response_fixture(vec![(200, Vec::new())]).await;
        assert_eq!(
            probe_gateway_at(
                ProbeBinding {
                    catalog_service_id: CATALOG_SERVICE,
                    service_id: "service-1",
                    resource_id: 30,
                },
                ProbeEndpoints {
                    private_denial_probe: &format!("{}/private", private_not_denied.base),
                    public_denial_probe: &format!("{}/public", private_not_denied.base),
                    supervisor_isolation_probe: &format!("{}/supervisor", private_not_denied.base),
                    model_proxy_isolation_probe: &format!("{}/model", private_not_denied.base),
                    platform_gateway: &private_not_denied.base,
                },
            )
            .await,
            ProbeOutcome::GatewayNetworkInitializationFailed
        );
        assert_eq!(private_not_denied.requests.await.unwrap().len(), 1);

        let public_not_denied =
            response_fixture(vec![(403, denial_headers()), (200, Vec::new())]).await;
        assert_eq!(
            probe_gateway_at(
                ProbeBinding {
                    catalog_service_id: CATALOG_SERVICE,
                    service_id: "service-1",
                    resource_id: 30,
                },
                ProbeEndpoints {
                    private_denial_probe: &format!("{}/private", public_not_denied.base),
                    public_denial_probe: &format!("{}/public", public_not_denied.base),
                    supervisor_isolation_probe: &format!("{}/supervisor", public_not_denied.base),
                    model_proxy_isolation_probe: &format!("{}/model", public_not_denied.base),
                    platform_gateway: &public_not_denied.base,
                },
            )
            .await,
            ProbeOutcome::GatewayNetworkInitializationFailed
        );
        assert_eq!(public_not_denied.requests.await.unwrap().len(), 2);

        let privileged_listener_exposed = response_fixture(vec![
            (403, denial_headers()),
            (403, denial_headers()),
            (200, Vec::new()),
        ])
        .await;
        assert_eq!(
            probe_gateway_at(
                ProbeBinding {
                    catalog_service_id: CATALOG_SERVICE,
                    service_id: "service-1",
                    resource_id: 30,
                },
                ProbeEndpoints {
                    private_denial_probe: &format!("{}/private", privileged_listener_exposed.base),
                    public_denial_probe: &format!("{}/public", privileged_listener_exposed.base),
                    supervisor_isolation_probe: &format!(
                        "{}/supervisor",
                        privileged_listener_exposed.base
                    ),
                    model_proxy_isolation_probe: &format!(
                        "{}/model",
                        privileged_listener_exposed.base
                    ),
                    platform_gateway: &privileged_listener_exposed.base,
                },
            )
            .await,
            ProbeOutcome::GatewayNetworkInitializationFailed
        );
        assert_eq!(privileged_listener_exposed.requests.await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn the_allowed_gateway_response_is_classified_without_retaining_its_body() {
        for (status, gateway_header, expected) in [
            (200, None, ProbeOutcome::Allowed),
            (
                403,
                Some(GATEWAY_OUTCOME_POLICY_DENIED),
                ProbeOutcome::PolicyDenied,
            ),
            (
                503,
                Some(GATEWAY_OUTCOME_BROKER_TRANSIENT),
                ProbeOutcome::TransientBrokerFailure,
            ),
            (403, None, ProbeOutcome::ProviderDenied),
            (429, None, ProbeOutcome::TransientBrokerFailure),
        ] {
            let mut gateway_headers = Vec::new();
            if let Some(value) = gateway_header {
                gateway_headers.push((GATEWAY_OUTCOME_HEADER, value));
            }
            let fixture = response_fixture(vec![
                (403, denial_headers()),
                (403, denial_headers()),
                (403, isolation_headers()),
                (403, isolation_headers()),
                (status, gateway_headers),
            ])
            .await;
            let outcome = probe_gateway_at(
                ProbeBinding {
                    catalog_service_id: CATALOG_SERVICE,
                    service_id: "service-1",
                    resource_id: 30,
                },
                ProbeEndpoints {
                    private_denial_probe: &format!("{}/private", fixture.base),
                    public_denial_probe: &format!("{}/public", fixture.base),
                    supervisor_isolation_probe: &format!("{}/supervisor", fixture.base),
                    model_proxy_isolation_probe: &format!("{}/model", fixture.base),
                    platform_gateway: &fixture.base,
                },
            )
            .await;
            assert_eq!(outcome, expected);
            let requests = fixture.requests.await.unwrap();
            assert_eq!(requests.len(), 5);
            assert!(requests[4].contains(
                "/api/v1/proxy/catalog-service-github/repositories/30?_nyxid_via=service-1"
            ));
        }
    }

    #[tokio::test]
    async fn readiness_advertises_the_exact_probe_contract_and_support_state() {
        for (probe, supported) in [
            (GatewayProbe::disabled(PathBuf::from("/tmp")), false),
            (
                GatewayProbe::with_command(
                    PathBuf::from("/tmp"),
                    vec!["/bin/true".into(), HELPER.into()],
                ),
                true,
            ),
        ] {
            let mut state = crate::AppState::for_test(&[]);
            state.gateway_probe = probe;
            let response = crate::ready(State(state)).await.into_response();
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("readiness is bounded JSON");
            let ready: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(ready["runtimeGatewayProbe"]["name"], NAME);
            assert_eq!(ready["runtimeGatewayProbe"]["version"], VERSION);
            assert_eq!(ready["runtimeGatewayProbe"]["supported"], supported);
        }
    }
}
