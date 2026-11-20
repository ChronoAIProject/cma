use super::*;

#[derive(Clone, Copy)]
pub(super) enum UnavailableReason {
    AcceptTimeout,
    AuthenticationUnavailable,
}

pub(super) fn unavailable(reason: UnavailableReason) -> Response {
    let mut response = http_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Agent access service is temporarily unavailable",
    );
    response.extensions_mut().insert(reason);
    response
}

pub(super) fn observe(response: &mut Response) {
    let Some(reason) = response.extensions_mut().remove::<UnavailableReason>() else {
        return;
    };
    let reason = match reason {
        UnavailableReason::AcceptTimeout => "accept_timeout",
        UnavailableReason::AuthenticationUnavailable => "authentication_unavailable",
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request_id).expect("generated UUID is a valid header"),
    );
    let span =
        tracing::warn_span!(target: "cma::agent_access", parent: None, "agent_access_availability");
    let _entered = span.enter();
    tracing::warn!(
        target: "cma::agent_access",
        parent: None,
        reason,
        request_id,
        "Agent access request unavailable"
    );
}
