use super::*;
use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use environment_diagnostics::EnvironmentRead;
use futures_util::StreamExt as _;
use tokio::time::Instant;

pub(super) enum Failure {
    Signature,
    Connect,
    Transport,
}

pub(super) fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

pub(super) fn send(
    relay: &Relay,
    destination: Destination,
    request: &Request,
    body: &[u8],
    deadline: Instant,
    environment: Option<&mut EnvironmentRead>,
) -> impl std::future::Future<Output = Result<reqwest::Response, Failure>> + use<> {
    let outgoing = prepare(relay, destination, request, body, environment);
    async move {
        match tokio::time::timeout_at(deadline, outgoing?.send()).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) if error.is_connect() => Err(Failure::Connect),
            _ => Err(Failure::Transport),
        }
    }
}

fn prepare(
    relay: &Relay,
    destination: Destination,
    request: &Request,
    body: &[u8],
    environment: Option<&mut EnvironmentRead>,
) -> Result<reqwest::RequestBuilder, Failure> {
    let mut headers = HeaderMap::new();
    for (name, value) in ingress::headers(request) {
        if name != file_read::HANDOFF_HEADER && authentication::forwarded(name.as_str()) {
            headers.append(name, value.clone());
        }
    }
    let headers = relay
        .authentication
        .signed_headers(
            &relay.coordinator.instance_id,
            &destination.token,
            request.method(),
            request.uri(),
            headers,
            body,
        )
        .map_err(|_| Failure::Signature)?;
    if let Some(environment) = environment {
        environment.correlate_hop(headers[PROOF].as_bytes());
    }
    let url = format!(
        "{}{}",
        destination.member.relay_address.trim_end_matches('/'),
        request
            .uri()
            .path_and_query()
            .map_or("/", |value| value.as_str())
    );
    Ok(relay
        .client
        .request(request.method().clone(), url)
        .headers(headers)
        .body(body.to_vec()))
}

pub(super) fn handoff(outcome: &Result<reqwest::Response, Failure>) -> Option<&'static str> {
    match outcome {
        Err(Failure::Connect) => Some("peer_connection_failed"),
        Ok(response)
            if response.status() == StatusCode::SERVICE_UNAVAILABLE
                && file_read::reported(response.headers()) =>
        {
            Some("peer_owner_handoff")
        }
        _ => None,
    }
}

pub(super) fn finish(
    outcome: Result<reqwest::Response, Failure>,
    permit: capacity::Permit,
    deadline: Instant,
    environment: Option<&EnvironmentRead>,
) -> Response {
    let response = match outcome {
        Ok(response) => response,
        Err(Failure::Signature) => {
            return http::unavailable_with(environment, Refusal::PeerSignature)
        }
        Err(Failure::Connect | Failure::Transport) => {
            return http::unavailable_with(environment, Refusal::PeerTransport);
        }
    };
    let status = response.status();
    if let Some(environment) =
        environment.filter(|_| status.is_client_error() || status.is_server_error())
    {
        environment.record(Refusal::PeerResponse);
    }
    let headers = response.headers().clone();
    // Keep the original relay deadline and capacity permit through the body.
    let stream = futures_util::stream::unfold(
        Some((response.bytes_stream(), permit)),
        move |state| async move {
            let (mut stream, permit) = state?;
            let next = tokio::time::timeout_at(deadline, stream.next()).await;
            let result = match next {
                Ok(Some(result)) => result.map_err(|_| std::io::Error::other("owner relay ended")),
                Ok(None) => return None,
                Err(_) => {
                    return Some((
                        Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "owner relay deadline exceeded",
                        )),
                        None,
                    ))
                }
            };
            Some((result, Some((stream, permit))))
        },
    );
    let mut result = Response::new(Body::from_stream(stream));
    *result.status_mut() = status;
    for (name, value) in &headers {
        if !matches!(
            name.as_str(),
            "connection"
                | "transfer-encoding"
                | "keep-alive"
                | "upgrade"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "trailer"
                | file_read::HANDOFF_HEADER
        ) {
            result.headers_mut().append(name, value.clone());
        }
    }
    result
}
