//! Browser identity inspection and logout under the retained `/auth/session` boundary.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use layer1_public::AuthenticatedCaller;
use layer2_core::{Authorization, Capability};

use super::{entitlements, profile::Profile, AuthState, AUTH_COOKIE};

pub(super) async fn read(
    State(auth): State<Arc<AuthState>>,
    caller: Option<AuthenticatedCaller>,
    profile: Option<Extension<Profile>>,
) -> Response {
    // Middleware already verified this request. A second resolution repeats
    // provider IO without adding a new authorization boundary.
    match caller {
        Some(AuthenticatedCaller(caller)) => {
            let usage_reporting = match auth.usage_reporting().await {
                Ok(settings) => settings,
                Err(_) => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        [(header::CONTENT_TYPE, "application/problem+json")],
                        Json(serde_json::json!({
                            "type": "urn:chrono:problem:settings-unavailable",
                            "title": "Service Unavailable", "status": 503,
                            "detail": "Usage presentation settings could not be read.",
                            "code": "settings_unavailable",
                        })),
                    )
                        .into_response()
                }
            };
            let mut body = serde_json::json!({
                "authenticated": true,
                "usage_reporting": usage_reporting,
                "subject": caller.subject.as_str(),
                "capabilities": entitlements::reported(&caller.authorization),
                "visitor": is_visitor(&caller.authorization),
            });
            let profile = profile
                .map(|Extension(profile)| profile)
                .unwrap_or_default();
            if let (Some(map), Ok(serde_json::Value::Object(profile))) =
                (body.as_object_mut(), serde_json::to_value(profile))
            {
                map.extend(profile);
            }
            Json(body).into_response()
        }
        None => (
            StatusCode::UNAUTHORIZED,
            // Advertised so a client only attempts `/auth/login?prompt=none`
            // against a control plane that implements it.
            Json(serde_json::json!({ "authenticated": false, "silent_sign_in": true })),
        )
            .into_response(),
    }
}

fn is_visitor(authorization: &Authorization) -> bool {
    authorization.allows(Capability::LaunchTriggers)
        && !authorization.allows(Capability::CreateSandbox)
}

pub(super) async fn logout(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    if let Some(value) = request_cookie(&headers, AUTH_COOKIE) {
        if let Err(error) = auth.revoke_browser_login(value).await {
            tracing::error!(error = %error, "could not revoke browser authentication");
            return super::middleware::store_failure();
        }
    }
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        expired_cookie(AUTH_COOKIE, auth.secure_cookies()),
    );

    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("/"));
    response
}

pub(super) fn request_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(candidate, value)| (candidate == name && !value.is_empty()).then_some(value))
}

pub(super) fn issued_cookie(name: &str, value: &str, secure: bool, max_age: u64) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={value}; Path=/; HttpOnly{secure}; SameSite=Lax; Max-Age={max_age}")
        .parse()
        .expect("validated cookie components form a header")
}

fn expired_cookie(name: &str, secure: bool) -> HeaderValue {
    issued_cookie(name, "", secure, 0)
}

pub(super) fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_are_host_only_http_only_and_conditionally_secure() {
        let local = issued_cookie("name", "value", false, 60);
        assert_eq!(
            local,
            "name=value; Path=/; HttpOnly; SameSite=Lax; Max-Age=60"
        );
        assert!(issued_cookie("name", "value", true, 60)
            .to_str()
            .unwrap()
            .contains("; Secure"));
    }

    #[test]
    fn url_encoding_uses_rfc_3986_unreserved_bytes() {
        assert_eq!(urlencode("http://a/b?c=d"), "http%3A%2F%2Fa%2Fb%3Fc%3Dd");
        assert_eq!(urlencode("plain-Value_1.0~"), "plain-Value_1.0~");
    }

    #[test]
    fn visitor_requires_launch_without_direct_sandbox_creation() {
        assert!(is_visitor(&Authorization::new([
            Capability::LaunchTriggers,
            Capability::UseOwnSandboxes,
        ])));
        assert!(!is_visitor(&Authorization::new([
            Capability::LaunchTriggers,
            Capability::CreateSandbox,
        ])));
        assert!(!is_visitor(&Authorization::new([
            Capability::UseOwnSandboxes,
        ])));
    }
}
