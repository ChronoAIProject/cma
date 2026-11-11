//! Identity inspection and explicit browser authentication navigation URLs.

use crate::{
    Client, Error, Result,
    http::{Request, parse_retry_after, receive, url},
};
use reqwest::StatusCode;
use serde::Deserialize;

/// Complete encoded absolute login URL budget. Normative wire contract:
/// api/crates/layer1-public/tests/fixtures/browser-auth-limits.json (4 KiB).
pub const MAX_LOGIN_URL_BYTES: usize = 4096;

string_enum! { /// Public entitlement reported by the identity endpoint.
    pub enum Capability {
        CreateSandbox => "create_sandbox", UseOwnSandboxes => "use_own_sandboxes", DeleteOwnSandbox => "delete_own_sandbox",
        ManageOwnProfiles => "manage_own_profiles", ManageOwnBots => "manage_own_bots", PublishTriggers => "publish_triggers", LaunchTriggers => "launch_triggers",
        ReadAllSandboxes => "read_all_sandboxes", ControlAnySandbox => "control_any_sandbox", ReadRuntimeInventory => "read_runtime_inventory",
        ReadEventLog => "read_event_log", ReadActivityLog => "read_activity_log", ReadUserDirectory => "read_user_directory", ConfigurePlatform => "configure_platform"
    }
}

/// Effective presentation policy for every authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UsageReporting {
    /// Whether measured usage may be displayed; independent of collection.
    pub measured_visible: bool,
}

/// Public profile and capability evidence for an authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Identity {
    /// Absent on older servers; treat absence as false.
    pub usage_reporting: Option<UsageReporting>,
    /// Authenticated subject identity.
    pub subject: String,
    /// Explicit allowed operations.
    pub capabilities: Vec<Capability>,
    /// Whether the caller has the visitor-only entitlement set.
    pub visitor: bool,
    /// Optional public display name.
    pub display_name: Option<String>,
    /// Optional owner-visible email.
    pub email: Option<String>,
    /// Optional public avatar URL.
    pub avatar_url: Option<String>,
}

/// Browser or bearer identity state, distinguished from dependency failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSession {
    /// No identity is authenticated.
    SignedOut,
    /// Server-validated identity and capabilities.
    SignedIn(Identity),
}

/// Anonymous signup choices, with no connection credentials.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SignupOptions {
    /// Available social provider identifiers.
    pub social_providers: Vec<String>,
    /// Whether email signup is available.
    pub email_signup: bool,
    /// Whether registration requires an invitation.
    pub invite_code_required: bool,
    /// Whether registration is currently available.
    pub register_available: bool,
}

/// Parameters for explicit authentication navigation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginOptions {
    /// Canonical CMA workflow path to return to after authentication.
    pub return_to: Option<String>,
    /// Open the supported registration screen instead of ordinary login.
    pub register: bool,
}

/// Identity reads and navigation URL construction; never initiates Launch work.
pub struct Authentication<'a> {
    client: &'a Client,
}

impl Authentication<'_> {
    /// Inspect authenticated identity; a valid 401 signed-out response is not an API failure.
    pub async fn read_session(&self) -> Result<AuthSession> {
        let mut request = Request::get(&["auth", "session"]);
        request.root = true;
        request.statuses = vec![
            StatusCode::OK,
            StatusCode::UNAUTHORIZED,
            StatusCode::SERVICE_UNAVAILABLE,
        ];
        let response = receive(&self.client.inner, request).await?;
        let body: serde_json::Value = serde_json::from_slice(&response.body)
            .map_err(|_| Error::protocol("invalid authentication response document"))?;
        match (
            response.status,
            body.get("authenticated")
                .and_then(serde_json::Value::as_bool),
        ) {
            (StatusCode::UNAUTHORIZED, Some(false)) => Ok(AuthSession::SignedOut),
            (StatusCode::OK, Some(true)) => serde_json::from_value(body)
                .map(AuthSession::SignedIn)
                .map_err(|_| Error::protocol("invalid authenticated identity document")),
            (StatusCode::SERVICE_UNAVAILABLE, Some(false)) if body["code"] == "store_failed" => {
                Err(Error::Api {
                    status: 503,
                    code: Some("store_failed".into()),
                    message: "Browser identity could not be read.".into(),
                    request_id: response
                        .headers
                        .get("x-request-id")
                        .and_then(|value| value.to_str().ok())
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_owned),
                    retry_after: response
                        .headers
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        .and_then(parse_retry_after),
                    extensions: Default::default(),
                })
            }
            (StatusCode::SERVICE_UNAVAILABLE, _) => Err(crate::http::problem_error(
                response.status,
                response
                    .headers
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
                response
                    .headers
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after),
                &response.body,
            )),
            _ => Err(Error::protocol(
                "authentication status did not match its body",
            )),
        }
    }

    /// Read public signup choices without sending the configured credential.
    pub async fn read_signup_options(&self) -> Result<SignupOptions> {
        let mut request = Request::get(&["auth", "signup-options"]);
        request.root = true;
        request.anonymous = true;
        self.client.request(request).await
    }

    /// Build an explicit login URL without making an HTTP request or following OAuth.
    pub fn login_url(&self, options: &LoginOptions) -> Result<reqwest::Url> {
        let mut target = url(&self.client.inner, &["auth", "login"], true)?;
        if let Some(return_to) = &options.return_to {
            validate_return_target(return_to)?;
            target.query_pairs_mut().append_pair("return_to", return_to);
        }
        if options.register {
            target.query_pairs_mut().append_pair("screen", "register");
        }
        if target.as_str().len() > MAX_LOGIN_URL_BYTES {
            return Err(Error::validation(
                "encoded login URL exceeds the public navigation budget",
            ));
        }
        Ok(target)
    }

    /// Build the explicit logout navigation URL; constructing it does not revoke identity.
    pub fn logout_url(&self) -> Result<reqwest::Url> {
        url(&self.client.inner, &["auth", "logout"], true)
    }
}

fn validate_return_target(value: &str) -> Result<()> {
    let (path, query) = value
        .split_once('?')
        .map_or((value, None), |(path, query)| (path, Some(query)));
    if value.len() > MAX_LOGIN_URL_BYTES
        || !value.is_ascii()
        || value.contains(['#', '\\'])
        || value.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        return Err(Error::validation(
            "return_to must be a canonical CMA workflow path",
        ));
    }
    if let Some(query) = query {
        let mut keys = std::collections::HashSet::new();
        let mut parsed =
            reqwest::Url::parse("https://return.invalid/").expect("fixed parsing base");
        parsed.set_query(Some(query));
        for (key, _) in parsed.query_pairs() {
            if !keys.insert(key.clone())
                || !((path == "/bots/new" && key == "setup")
                    || (path.starts_with("/sandboxes/") && key == "tab"))
            {
                return Err(Error::validation(
                    "return_to must be a canonical CMA workflow path",
                ));
            }
        }
    }
    if path == "/" {
        return Ok(());
    }
    if path.len() > 256 || path.contains('%') {
        return Err(Error::validation(
            "return_to must be a canonical CMA workflow path",
        ));
    }
    let segments = path
        .strip_prefix('/')
        .ok_or_else(|| Error::validation("return_to must be a canonical CMA workflow path"))?
        .split('/')
        .collect::<Vec<_>>();
    if segments.iter().any(|segment| !opaque_path_segment(segment)) {
        return Err(Error::validation(
            "return_to must be a canonical CMA workflow path",
        ));
    }
    match segments.as_slice() {
        ["agent-manager" | "event-gateway" | "image-manager"] => {}
        ["t", public_id] if opaque_path_segment(public_id) => {}
        ["t", public_id, placement_id]
            if opaque_path_segment(public_id) && opaque_path_segment(placement_id) => {}
        ["triggers", "manage"] => {}
        ["triggers", "manage", trigger_id] if opaque_path_segment(trigger_id) => {}
        ["triggers", "manage", trigger_id, "embed"] if opaque_path_segment(trigger_id) => {}
        ["agents", agent_id] if opaque_path_segment(agent_id) => {}
        ["sandboxes", sandbox_id] if opaque_path_segment(sandbox_id) => {}
        ["bots"] => {}
        ["bots", bot_id] if opaque_path_segment(bot_id) => {}
        ["auth", "bot-father", "link", intent, "complete"] if opaque_path_segment(intent) => {}
        ["profiles"] | ["marketplace"] => {}
        ["profiles" | "marketplace", kind, profile_id]
            if matches!(*kind, "agent" | "workspace" | "sandbox")
                && opaque_path_segment(profile_id) => {}
        _ => {
            return Err(Error::validation(
                "return_to must be a canonical CMA workflow path",
            ));
        }
    }
    Ok(())
}

impl Client {
    /// Access authenticated identity and explicit login navigation.
    pub fn auth(&self) -> Authentication<'_> {
        Authentication { client: self }
    }
}

fn opaque_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
}

#[cfg(test)]
mod navigation_tests {
    use super::*;
    #[test]
    fn full_encoded_url_and_ascii_match_the_public_budget() {
        let spec: serde_json::Value = serde_json::from_str(include_str!(
            "../../../api/crates/layer1-public/tests/fixtures/browser-auth-limits.json"
        ))
        .unwrap();
        assert_eq!(
            MAX_LOGIN_URL_BYTES,
            spec["max_login_url_bytes"].as_u64().unwrap() as usize
        );
        let client = Client::builder("https://cma.example")
            .unwrap()
            .build()
            .unwrap();
        let base = "/bots/new?setup=";
        let overhead = client
            .auth()
            .login_url(&LoginOptions {
                return_to: Some(base.into()),
                register: true,
            })
            .unwrap()
            .as_str()
            .len();
        for length in [
            MAX_LOGIN_URL_BYTES - 1,
            MAX_LOGIN_URL_BYTES,
            MAX_LOGIN_URL_BYTES + 1,
        ] {
            let result = client.auth().login_url(&LoginOptions {
                return_to: Some(format!("{base}{}", "a".repeat(length - overhead))),
                register: true,
            });
            if length <= MAX_LOGIN_URL_BYTES {
                assert_eq!(result.unwrap().as_str().len(), length);
            } else {
                assert!(result.is_err());
            }
        }
        for value in ["/bots/new?setup=界", "/sandboxes/s?tab=é"] {
            assert!(validate_return_target(value).is_err());
        }
        assert!(
            client
                .auth()
                .login_url(&LoginOptions {
                    return_to: Some(format!("{base}{}", "%22".repeat(1500))),
                    register: false
                })
                .is_err()
        );
    }
}
