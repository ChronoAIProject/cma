//! Contract tests for declaration-only interface composition and reporting.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::any,
    Router,
};
use layer1_public::rest::HttpProblem;
use tower::ServiceExt;

use super::{interface_is_served, interfaces};

const API_A2A_DOCUMENTATION: &str = include_str!("../../../../docs/API_A2A.md");
const API_MCP_DOCUMENTATION: &str = include_str!("../../../../docs/API_MCP.md");
const API_OVERVIEW_DOCUMENTATION: &str = include_str!("../../../../docs/API_Overview.md");
const NOT_BUILT_BANNER: &str = "**This interface is not built yet.** Nothing below is served today; it is the design the implementation will be held to, and the endpoint refuses with `501` until then.";

#[test]
fn only_implemented_interfaces_are_reported_as_served() {
    let served: Vec<_> = interfaces()
        .into_iter()
        .filter(|interface| interface_is_served(interface.as_ref()))
        .map(|interface| interface.interface_id())
        .collect();
    assert_eq!(served, ["agui", "responses", "rest", "pty"]);

    let declared_only: Vec<_> = interfaces()
        .into_iter()
        .filter(|interface| !interface_is_served(interface.as_ref()))
        .map(|interface| interface.interface_id())
        .collect();
    assert_eq!(declared_only, ["mcp", "a2a"]);
}

#[tokio::test]
async fn declaration_only_routes_precede_the_rest_not_found_scope() {
    let app = Router::new()
        .route("/api/v1/{*path}", any(|| async { HttpProblem::NotFound }))
        .method_not_allowed_fallback(|| async { HttpProblem::MethodNotAllowed })
        .merge(layer1_public::mcp::router())
        .merge(layer1_public::a2a::router());

    for request in [
        Request::post("/api/v1/mcp").body(Body::empty()).unwrap(),
        Request::get("/.well-known/agent-card.json")
            .body(Body::empty())
            .unwrap(),
        Request::post("/api/v1/a2a").body(Body::empty()).unwrap(),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(
            response.headers().get(axum::http::header::CACHE_CONTROL),
            Some(&"no-store".parse().unwrap())
        );
    }

    let response = app
        .oneshot(
            Request::post("/api/v1/a2a/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn declaration_only_documentation_stays_honest() {
    for (name, document) in [
        ("A2A", API_A2A_DOCUMENTATION),
        ("MCP", API_MCP_DOCUMENTATION),
    ] {
        let priority = document
            .find("**Priority ")
            .expect("the interface document names its priority");
        let banner = document
            .find(NOT_BUILT_BANNER)
            .unwrap_or_else(|| panic!("{name} must retain the exact not-built banner"));
        let first_section = document
            .find("\n---\n")
            .expect("the interface document has a first section divider");

        assert_eq!(
            document.matches(NOT_BUILT_BANNER).count(),
            1,
            "{name} must carry one unambiguous not-built banner"
        );
        assert!(
            priority < banner && banner < first_section,
            "{name} must disclose its absence before any design content"
        );
    }

    assert!(API_OVERVIEW_DOCUMENTATION.contains("serves four public interfaces"));
    assert!(API_OVERVIEW_DOCUMENTATION.contains("MCP and A2A are designed but not built"));
    assert!(!API_OVERVIEW_DOCUMENTATION.contains("exposes five public interfaces"));

    let mcp_priority_row = API_OVERVIEW_DOCUMENTATION
        .lines()
        .find(|line| line.starts_with("| 5 | MCP"))
        .expect("the overview retains the MCP priority row");
    let a2a_priority_row = API_OVERVIEW_DOCUMENTATION
        .lines()
        .find(|line| line.starts_with("| 6 | A2A"))
        .expect("the overview retains the A2A priority row");
    assert!(mcp_priority_row.contains("designed, not yet served"));
    assert!(a2a_priority_row.contains("designed, not yet served"));

    let matrix_header = API_OVERVIEW_DOCUMENTATION
        .lines()
        .find(|line| line.starts_with("| Operation |") && line.contains("MCP"))
        .expect("the overview retains the operation matrix");
    assert!(matrix_header.contains("MCP[^designed-only]"));
    assert!(matrix_header.contains("A2A[^designed-only]"));
    let matrix_legend = API_OVERVIEW_DOCUMENTATION
        .lines()
        .find(|line| line.contains("observed but not initiated"))
        .expect("the overview retains the operation-matrix legend");
    assert!(matrix_legend.contains("projected"));
    assert!(!matrix_legend.contains(" served "));
    assert!(API_OVERVIEW_DOCUMENTATION
        .contains("[^designed-only]: MCP and A2A are designed, not yet served."));

    let declaration_section = API_OVERVIEW_DOCUMENTATION
        .split("## 5. Capability declaration")
        .nth(1)
        .and_then(|tail| tail.split("\n---\n").next())
        .expect("the overview retains its capability-declaration section");
    assert!(declaration_section.contains("A2A (designed, not yet served)"));
    assert!(declaration_section.contains("MCP (designed, not yet served)"));
    assert!(declaration_section.contains("future declaration mechanisms"));

    assert!(!API_A2A_DOCUMENTATION.contains("are implemented in full"));
    assert!(!API_A2A_DOCUMENTATION.contains("conformance suite is run in CI"));
    assert!(API_A2A_DOCUMENTATION.contains("The future implementation must serve"));
    assert!(API_A2A_DOCUMENTATION.contains("conformance suite must be added to CI"));
}
