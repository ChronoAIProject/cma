use super::*;
use axum::{
    body::HttpBody as _,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method},
    middleware::Next,
    response::Response,
};

pub(super) const HANDOFF_HEADER: &str = "x-cma-file-owner-handoff";

pub(super) fn eligible(path: &str, method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
        && path
            .strip_prefix("/api/v1/agents/")
            .and_then(|path| path.split_once('/'))
            .is_some_and(|(_, suffix)| {
                matches!(suffix, "files/tree" | "files/content" | "files/download")
            })
}

pub(super) fn refusal(reason: Refusal) -> Response {
    tracing::debug!(
        target: "cma::agent_manager::file_read_relay",
        ?reason,
        "Verified file read reached an owner during handoff"
    );
    let mut response = http::unavailable();
    mark(&mut response);
    response
}

fn mark(response: &mut Response) {
    response
        .headers_mut()
        .insert(HANDOFF_HEADER, HeaderValue::from_static("1"));
}

impl Relay {
    pub(crate) fn mark_draining_file_read(&self, request: &Request, response: &mut Response) {
        if !eligible(request.uri().path(), request.method())
            || request.body().size_hint().exact() != Some(0)
            || request.headers().get_all(PROOF).iter().count() != 1
        {
            return;
        }
        let Some(agent) = http::agent_path(request.uri().path(), false) else {
            return;
        };
        let verified = request.headers()[PROOF].to_str().ok().and_then(|proof| {
            self.authentication
                .verify(
                    proof,
                    request.method(),
                    request.uri(),
                    request.headers(),
                    &[],
                )
                .ok()
        });
        if verified.is_some_and(|proof| {
            proof.reports_file_handoff()
                && proof.owner.agent_id == agent
                && proof.owner.instance_id == self.coordinator.instance_id
                && proof.owner.protocol == layer2_core::agent_ownership::PROTOCOL_VERSION
        }) {
            mark(response);
        }
    }
}

pub(super) fn reported(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(HANDOFF_HEADER).iter();
    values.next().is_some_and(|value| value == "1") && values.next().is_none()
}

pub(super) async fn recover(
    relay: &Relay,
    id: &AgentId,
    request: Request,
    next: Next,
    permit: capacity::Permit,
    deadline: tokio::time::Instant,
    reason: &'static str,
) -> Response {
    let response = tokio::time::timeout_at(deadline, async {
        if !relay.coordinator.is_ready() {
            return http::unavailable();
        }
        if let Err(refusal) = http::authorize_public(relay, id, &request, None).await {
            return refusal;
        }
        next.run(request).await
    })
    .await
    .unwrap_or_else(|_| http::unavailable());
    tracing::debug!(
        target: "cma::agent_manager::file_read_relay",
        reason,
        status = response.status().as_u16(),
        "File read handoff handled at relay ingress"
    );
    http::retain_until(response, permit, Some(deadline))
}
