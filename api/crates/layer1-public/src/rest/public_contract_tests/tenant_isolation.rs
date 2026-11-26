//! Replay valid private-resource requests as a different, fully capable tenant.
//! Owner setup/captures and subsequent exact owner responses remain unchanged.
use super::{Case, Scenario, Symbols};
use axum::body::{to_bytes, Body};
use axum::http::Request;
use layer2_core::{Authorization, Capability, SubjectId};
use serde_json::Value;
use tower::ServiceExt;
mod context;
mod inventory;
mod receipts;
pub(super) fn included(case: &Case) -> bool {
    inventory::included(case)
}

pub(super) fn selected(case: &Case) -> bool {
    // These compositions use real core services and take their caller from the
    // request. Context-bearing sharing fixtures have separate revoked/foreign
    // contracts; changing only their Subject would test a mismatched context.
    if matches!(
        case.id.as_str(),
        "terminal-unsupported" | "launch-review-stream"
    ) {
        return true;
    }
    let private = matches!(
        case.scenario.as_str(),
        "agents_owner"
            | "read_projection_owner"
            | "read_projection_missing"
            | "agents_stopped"
            | "agents_operating"
            | "agents_approval"
            | "agents_paused"
            | "instances_owner"
            | "retained_recovery_owner"
            | "retained_reopen_owner"
            | "instances_paused"
            | "instances_lifecycle"
            | "source_manual"
            | "source_workspace"
            | "attachment_owner"
            | "agent_customization_owner"
            | "agent_customization_admission"
            | "usage_owner"
    ) || case.scenario.starts_with("document_owner__");
    private && (200..300).contains(&case.response.status) && case.operation.contains("/{")
}

pub(super) async fn probe(scene: &Scenario, symbols: &Symbols, case: &Case) {
    let path = symbols.resolve_text(&case.request.path);
    let (_, template) = case.operation.split_once(' ').unwrap();
    let index = template
        .split('/')
        .position(|part| part.starts_with('{'))
        .unwrap();
    let (resource_path, query) = path.split_once('?').unwrap_or((&path, ""));
    let mut parts: Vec<String> = resource_path.split('/').map(str::to_owned).collect();
    let prefix = parts[index].split('_').next().unwrap();
    parts[index] = format!("{prefix}_00000000000000000000000000000000");
    let absent = format!(
        "{}{}{}",
        parts.join("/"),
        if query.is_empty() { "" } else { "?" },
        query
    );
    for method in [
        Some(case.request.method.as_str()),
        (case.request.method == "GET").then_some("HEAD"),
    ]
    .into_iter()
    .flatten()
    {
        let mut first_body = None;
        for target in [&path, &absent] {
            let mut request = Request::builder().method(method).uri(target);
            for (name, value) in &case.request.headers {
                request = request.header(name, symbols.resolve_text(value));
            }
            let mut request = request
                .body(Body::from(super::replay::encode_request_body(
                    &case.request,
                    symbols,
                )))
                .unwrap();
            request
                .extensions_mut()
                .insert(crate::AuthenticatedSubject(SubjectId::new(
                    "isolation-foreign-tenant",
                )));
            // A denial must depend on ownership, not missing ordinary capability.
            request.extensions_mut().insert(Authorization::new([
                Capability::UseOwnSandboxes,
                Capability::CreateSandbox,
                Capability::DeleteOwnSandbox,
            ]));
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                scene.router.clone().oneshot(request),
            )
            .await
            .unwrap()
            .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            let body: Value = if method == "HEAD" {
                assert!(bytes.is_empty(), "{} HEAD must not expose bytes", case.id);
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap_or(Value::Null)
            };
            assert_eq!(
                status.as_u16(),
                404,
                "{} foreign {method} status; problem={}",
                case.id,
                body["code"]
            );
            if method != "HEAD" {
                let code = body["code"].as_str().expect("typed denial code");
                assert!(
                    inventory::known_not_found(code),
                    "{} unknown concealment code: {code}",
                    case.id
                );
                assert!(
                    body.as_object()
                        .unwrap()
                        .keys()
                        .all(|key| ["type", "title", "detail", "status", "code"]
                            .contains(&key.as_str())),
                    "denial contains only the typed problem, never protected fields"
                );
            }
            if let Some(expected) = &first_body {
                assert_eq!(&body, expected, "{} existence disclosure", case.id);
            } else {
                first_body = Some(body);
            }
        }
    }
}

#[tokio::test]
async fn private_requests_cannot_be_replayed_by_another_owner() {
    super::run_contracts(true).await;
}
