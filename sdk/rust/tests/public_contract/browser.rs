use super::context::{Case, Context};
use cma_sdk::{
    Error,
    auth::{Identity, LoginOptions, SignupOptions},
};

pub const BROWSER_CASES: &[&str] = &[
    "auth-trusted-login-return",
    "auth-trusted-register-return",
    "auth-trusted-callback",
    "auth-trusted-logout",
    "auth-expired-browser-session",
    "auth-visitor-session",
    "auth-visitor-logout",
    "auth-revoked-browser-session",
    "auth-missing-authority-denied",
    "auth-signup-ambient_failure",
    "auth-signup-ambient_failure-cached",
    "auth-session-store-unavailable",
    "auth-login-store-unavailable",
    "auth-callback-store-unavailable",
    "auth-logout-store-unavailable",
    "auth-login-open-redirect-refused",
    "auth-login-retired-return-refused",
    "auth-login-discovery-unavailable",
    "auth-callback-missing-state",
    "auth-callback-missing-binding",
    "auth-callback-invalid-state",
    "auth-callback-expired-state",
    "auth-callback-cancelled",
    "auth-callback-code-missing",
    "triggers-open-without-user-activation",
    "triggers-entry-intent-without-click-evidence",
    "triggers-entry-intent-cross-origin",
];

pub fn owns(case: &Case) -> bool {
    BROWSER_CASES.contains(&case.id.as_str())
}

// Cookie authority and OAuth callbacks belong to the browser. Validate their
// shared response models and navigation helpers without issuing a replacement request.
pub fn verify(c: &Context<'_>) {
    match c.case.operation.as_str() {
        "GET /api/v1/triggers/public/{public_id}/open/{placement_id}" => {
            assert_eq!(c.case.response.status, 303);
            assert_eq!(
                c.case.response.headers["cache-control"],
                "private, no-store"
            );
            assert!(c.case.response.headers["set-cookie"].contains("Max-Age=0"));
            let parts: Vec<_> = c.case.request.path.split('/').collect();
            assert_eq!(
                c.case.response.headers["location"],
                format!("/t/{}/{}", parts[5], parts[7])
            );
        }
        "POST /api/v1/triggers/public/{public_id}/entry-intent" => {
            // The browser owns Origin, Fetch metadata and the HttpOnly evidence cookie.
            assert!(c.case.request.headers.contains_key("origin"));
            match c.case.response.status {
                200 => {
                    assert_eq!(c.case.response.body, serde_json::json!({"accepted": false}));
                    assert_eq!(
                        c.case.response.headers["cache-control"],
                        "private, no-store"
                    );
                    assert!(c.case.response.headers["set-cookie"].contains("Max-Age=0"));
                }
                403 => {
                    assert_eq!(c.case.response.body_kind, "empty");
                    assert!(c.case.response.body.is_null());
                }
                _ => panic!("unexpected entry intent status {}", c.case.response.status),
            }
        }
        "GET /auth/login" => {
            let result = c.client.auth().login_url(&LoginOptions {
                return_to: c.q("return_to"),
                register: c.q("screen").as_deref() == Some("register"),
            });
            if c.case.response.status == 400 {
                assert!(
                    matches!(result, Err(Error::Validation { .. })),
                    "{}",
                    c.case.id
                );
            } else {
                let target = result.unwrap();
                assert!(target.path().ends_with("/auth/login"));
                let query: std::collections::BTreeMap<_, _> = target
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                assert_eq!(query, c.query);
            }
        }
        "GET /auth/logout" => {
            let target = c.client.auth().logout_url().unwrap();
            assert!(target.path().ends_with("/auth/logout"));
            assert!(target.query().is_none());
        }
        "GET /auth/callback" => assert!(c.case.request.path.starts_with("/auth/callback")),
        "GET /auth/session" => {
            assert!(c.case.request.headers.contains_key("cookie"));
            match (
                c.case.response.status,
                c.case.response.body["authenticated"].as_bool(),
            ) {
                (200, Some(true)) => {
                    let _: Identity = serde_json::from_value(c.case.response.body.clone()).unwrap();
                }
                (401, Some(false)) => assert_eq!(
                    c.case.response.body,
                    serde_json::json!({"authenticated":false})
                ),
                (503, None) => {}
                _ => panic!("invalid browser identity document {}", c.case.id),
            }
        }
        "GET /auth/signup-options" => {
            assert!(c.case.request.headers.contains_key("cookie"));
            let _: SignupOptions = serde_json::from_value(c.case.response.body.clone()).unwrap();
        }
        _ => panic!("unexpected browser-owned operation {}", c.case.operation),
    }
    if c.case.response.body.get("detail").is_some() {
        super::check_error(
            c.case,
            Error::from_http_problem(
                c.case.response.status,
                None,
                None,
                &serde_json::to_vec(&c.case.response.body).unwrap(),
            ),
        );
    } else if c.case.response.status == 303 {
        assert!(
            c.case
                .response
                .headers
                .get("location")
                .is_some_and(|value| value.starts_with('/'))
        );
        assert_eq!(c.case.response.body_kind, "empty");
    }
}
