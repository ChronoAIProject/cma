use super::*;

#[test]
fn preferences_owner_route_is_exact_and_patch_only() {
    use axum::http::Method;
    for method in [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::OPTIONS,
        Method::PATCH,
    ] {
        assert_eq!(
            needs_owner("/api/v1/agents/agt-example/preferences", &method),
            method == Method::PATCH
        );
    }
    for path in [
        "/api/v1/agents/agt-example/preferences/extra",
        "/api/v1/agents/agt-example/stop",
        "/api/v1/agents/agt-example",
        "/api/v1/sandboxes/sbx-example/preferences",
    ] {
        assert!(!needs_owner(path, &Method::PATCH), "{path}");
    }
}

#[test]
fn settings_owner_route_is_exact_and_patch_only() {
    use axum::http::Method;
    let path = "/api/v1/agents/agt-example/settings";
    for method in [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::OPTIONS,
        Method::PATCH,
    ] {
        assert_eq!(settings_patch(path, &method), method == Method::PATCH);
        assert!(
            !needs_owner(path, &method),
            "existing routes include {method} {path}"
        );
    }
    for (method, path) in [
        (Method::PATCH, "/api/v1/agents/agt-example/settings/extra"),
        (Method::PATCH, "/api/v1/agents//settings"),
        (Method::PATCH, "/api/v2/agents/agt-example/settings"),
        (Method::POST, "/api/v1/agents/agt-example/stop"),
        (Method::DELETE, "/api/v1/agents/agt-example"),
        (Method::POST, "/api/v1/agents/agt-example/input"),
        (Method::POST, "/api/v1/agents/agt-example/steer"),
        (Method::POST, "/api/v1/agui/run"),
        (Method::GET, "/api/v2/agents/agt-example/queued-inputs"),
        (Method::POST, "/api/v2/agents/agt-example/responses"),
    ] {
        assert!(!settings_patch(path, &method), "{method} {path}");
        assert!(!needs_owner(path, &method), "{method} {path}");
    }
}
