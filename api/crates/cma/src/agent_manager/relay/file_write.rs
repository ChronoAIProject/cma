use super::*;
use axum::{extract::Request, http::Method, middleware::Next, response::Response};
use layer2_core::sandbox::agent_filesystem::FileWriteTarget;

pub(super) const NOT_ENTERED_HEADER: &str = "x-cma-file-write-not-entered";

#[derive(serde::Deserialize)]
struct EditPrecondition<'a> {
    expected_sha256: Option<&'a str>,
    #[serde(default)]
    directory: bool,
}

pub(super) fn eligible(path: &str, method: &Method, body: &[u8]) -> bool {
    *method == Method::POST
        && path
            .strip_prefix("/api/v1/agents/")
            .and_then(|suffix| suffix.split_once('/'))
            .is_some_and(|(_, operation)| operation == "files")
        && serde_json::from_slice::<EditPrecondition<'_>>(body).is_ok_and(|input| {
            !input.directory
                && input.expected_sha256.is_some_and(|digest| {
                    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        })
}

pub(super) fn refusal(relay: &Relay, proof: &[u8]) -> Response {
    let mut response = http::unavailable();
    response.headers_mut().insert(
        NOT_ENTERED_HEADER,
        relay.authentication.write_not_entered_tag(proof),
    );
    response
}

pub(super) async fn recover(
    relay: &Relay,
    id: &AgentId,
    mut request: Request,
    next: Next,
    permit: capacity::Permit,
    deadline: tokio::time::Instant,
    target: FileWriteTarget,
) -> Response {
    let response = tokio::time::timeout_at(deadline, async {
        if !relay.coordinator.is_ready() {
            return http::unavailable();
        }
        if let Err(refusal) = http::authorize_public(relay, id, &request, None, false).await {
            return refusal;
        }
        request.extensions_mut().insert(target);
        next.run(request).await
    })
    .await
    .unwrap_or_else(|_| http::unavailable());
    http::retain_until(response, permit, Some(deadline))
}
