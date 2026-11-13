//! Browser identity inspection and logout under the retained `/auth/session` boundary.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use layer1_public::{AuthenticatedCaller, AuthenticatedResourceCaller};
use layer2_core::{account_deletion::OwnerSessionAuthority, Authorization, Capability};

use super::{profile::Profile, AuthState, AUTH_COOKIE};

pub(super) async fn read(
    State(auth): State<Arc<AuthState>>,
    caller: Option<AuthenticatedCaller>,
    profile: Option<Extension<Profile>>,
    authority: Option<Extension<OwnerSessionAuthority>>,
    resource: Option<AuthenticatedResourceCaller>,
    headers: HeaderMap,
) -> Response {
    // Middleware already verified this request. A second resolution repeats
    // provider IO without adding a new authorization boundary.
    match caller {
        Some(AuthenticatedCaller(caller)) => {
            let erasing = match auth.standing.erasing(&caller.subject).await {
                Ok(erasing) => erasing,
                Err(_) => return super::account_standing::unavailable(),
            };
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
            let api_commands = !erasing && resource.as_ref().is_some_and(|r| {
                r.credential.as_ref().is_some_and(|credential| {
                    use layer2_core::resource_access::CredentialSource;
                    // 中间件已核验这个凭据；只有普通 Key 可以没有过期时间。
                    let lifetime = match r.context.credential_source {
                        CredentialSource::Independent { expires_at: Some(expiry) } => expiry > std::time::SystemTime::now(),
                        CredentialSource::Independent { expires_at: None } => super::agent_keys::is_agent_key(credential.value()),
                        _ => false,
                    };
                    lifetime && auth.permits_oauth_or_key_commands(credential.value())
                })
                    && caller.authorization.allows(Capability::UseOwnSandboxes)
            });
            let mut body = serde_json::json!({
                "authenticated": true,
                "api_commands": api_commands,
                "usage_reporting": usage_reporting,
                "subject": caller.subject.as_str(),
                "capabilities": auth.session_entitlements(&caller, authority.as_ref().map(|Extension(authority)| authority)),
                "visitor": is_visitor(&caller.authorization),
            });
            if !erasing {
                let profile = profile
                    .map(|Extension(profile)| profile)
                    .unwrap_or_default();
                if let (Some(map), Ok(serde_json::Value::Object(profile))) =
                    (body.as_object_mut(), serde_json::to_value(profile))
                {
                    map.extend(profile);
                }
            }
            Json(body).into_response()
        }
        None => (
            StatusCode::UNAUTHORIZED,
            // Advertised so a client only attempts `/auth/login?prompt=none`
            // against a control plane that implements it.
            Json(serde_json::json!({ "authenticated": false, "silent_sign_in": auth.silent_sign_in_enabled && !super::silent_preference::suppressed(&auth,&headers).await })),
        )
            .into_response(),
    }
}

fn is_visitor(authorization: &Authorization) -> bool {
    authorization.allows(Capability::LaunchTriggers)
        && !authorization.allows(Capability::CreateSandbox)
}

pub(super) async fn logout(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    auth.identity_store.clear();
    // Every pending callback carries its binding cookie even after consuming
    // its one-use state. Revoking the same custody row fences session issuance.
    let suppress = super::silent_preference::same_origin(&auth, &headers);
    let mut failed = std::collections::HashSet::new();
    for (name, value) in request_cookies(&headers) {
        if name == AUTH_COOKIE || name.starts_with(super::flow::BINDING_COOKIE_PREFIX) {
            let revoked = auth
                .store
                .logout_browser_custody(
                    &super::persistence::SecretDigest::identify(super::AUTH_COOKIE_DOMAIN, value),
                    name != AUTH_COOKIE,
                    suppress,
                )
                .await;
            if let Err(error) = revoked {
                tracing::error!(error = %error, "could not revoke browser authentication");
                failed.insert(name);
            }
        }
    }
    if suppress {
        if let Some((binding, _)) = super::silent_preference::reference(&headers) {
            if let Err(error) = auth.store.revoke_browser_binding(&binding).await {
                tracing::error!(error=%error,"could not revoke sign-in preference");
                failed.insert("cma_signed_in");
            }
        }
    }
    let mut response = if !failed.is_empty() {
        super::middleware::store_failure()
    } else {
        StatusCode::SEE_OTHER.into_response()
    };
    if !failed.contains(AUTH_COOKIE) {
        response.headers_mut().insert(
            header::SET_COOKIE,
            expired_cookie(AUTH_COOKIE, auth.secure_cookies()),
        );
    }
    super::silent_preference::record_sign_out(&mut response, &headers, &auth);
    for (name, _) in request_cookies(&headers) {
        if name.starts_with(super::flow::BINDING_COOKIE_PREFIX) && !failed.contains(name) {
            response.headers_mut().append(
                header::SET_COOKIE,
                expired_cookie(name, auth.secure_cookies()),
            );
        }
    }
    if failed.is_empty() {
        response
            .headers_mut()
            .insert(header::LOCATION, HeaderValue::from_static("/"));
    }
    response
}

pub(super) fn request_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    request_cookies(headers).find_map(|(candidate, value)| (candidate == name).then_some(value))
}

fn request_cookies(headers: &HeaderMap) -> impl Iterator<Item = (&str, &str)> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|part| part.trim().split_once('='))
        .filter(|(_, value)| !value.is_empty())
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
