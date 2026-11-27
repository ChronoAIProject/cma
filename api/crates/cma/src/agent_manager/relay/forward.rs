use super::*;
use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use futures_util::StreamExt as _;
use relay_diagnostics::{RecoveryTrigger, RelayDiagnostic};
use reqwest::RequestBuilder;
use tokio::time::Instant;

pub(super) enum Failure {
    Signature,
    Connect,
    Transport,
    WriteNotEntered,
}

pub(super) fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

pub(super) fn send(
    relay: &Relay,
    destination: Destination,
    request: &Request,
    body: &[u8],
    browser_claim: Option<file_identity::ProjectedIdentity>,
    deadline: Instant,
    diagnostic: Option<&mut RelayDiagnostic>,
) -> impl std::future::Future<Output = Result<reqwest::Response, Failure>> + use<> {
    let outgoing = prepare(relay, destination, request, body, browser_claim, diagnostic);
    async move {
        let (request, write_tag) = outgoing?;
        match tokio::time::timeout_at(deadline, request.send()).await {
            Ok(Ok(response))
                if response.status() == StatusCode::SERVICE_UNAVAILABLE
                    && write_tag
                        .as_ref()
                        .is_some_and(|expected| expected.verify(response.headers())) =>
            {
                Err(Failure::WriteNotEntered)
            }
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
    browser_claim: Option<file_identity::ProjectedIdentity>,
    diagnostic: Option<&mut RelayDiagnostic>,
) -> Result<(RequestBuilder, Option<authentication::WriteNotEntered>), Failure> {
    let mut headers = HeaderMap::new();
    for (name, value) in ingress::headers(request) {
        if !matches!(
            name.as_str(),
            file_read::HANDOFF_HEADER | file_write::NOT_ENTERED_HEADER | file_identity::HEADER
        ) && authentication::forwarded(name.as_str())
        {
            headers.append(name, value.clone());
        }
    }
    let headers = match browser_claim {
        Some(claim) => relay.authentication.signed_file_preview_headers(
            &relay.coordinator.instance_id,
            &destination.token,
            request.uri(),
            headers,
            claim,
        ),
        None => relay.authentication.signed_headers(
            &relay.coordinator.instance_id,
            &destination.token,
            request.method(),
            request.uri(),
            headers,
            body,
        ),
    }
    .map_err(|_| Failure::Signature)?;
    if let Some(diagnostic) = diagnostic {
        diagnostic.correlate_hop(headers[PROOF].as_bytes());
    }
    let write_tag = (relay.authentication.file_write_handoff_enabled()
        && file_write::eligible(request.uri().path(), request.method(), body))
    .then(|| {
        relay
            .authentication
            .write_not_entered_verifier(headers[PROOF].as_bytes())
    });
    let url = format!(
        "{}{}",
        destination.member.relay_address.trim_end_matches('/'),
        request
            .uri()
            .path_and_query()
            .map_or("/", |value| value.as_str())
    );
    Ok((
        relay
            .client
            .request(request.method().clone(), url)
            .headers(headers)
            .body(body.to_vec()),
        write_tag,
    ))
}

pub(super) fn handoff(outcome: &Result<reqwest::Response, Failure>) -> Option<RecoveryTrigger> {
    match outcome {
        Err(Failure::Connect) => Some(RecoveryTrigger::PeerConnectionFailed),
        Ok(response)
            if response.status() == StatusCode::SERVICE_UNAVAILABLE
                && file_read::reported(response.headers()) =>
        {
            Some(RecoveryTrigger::PeerOwnerHandoff)
        }
        _ => None,
    }
}

pub(super) fn finish(
    outcome: Result<reqwest::Response, Failure>,
    permit: capacity::Permit,
    deadline: Instant,
    diagnostic: Option<&RelayDiagnostic>,
) -> Response {
    let response = match outcome {
        Ok(response) => response,
        Err(Failure::Signature) => {
            return http::unavailable_with(diagnostic, Refusal::PeerSignature)
        }
        Err(Failure::Connect | Failure::Transport) => {
            return http::unavailable_with(diagnostic, Refusal::PeerTransport);
        }
        Err(Failure::WriteNotEntered) => {
            return http::unavailable_with(diagnostic, Refusal::PeerResponse);
        }
    };
    let status = response.status();
    if let Some(diagnostic) =
        diagnostic.filter(|_| status.is_client_error() || status.is_server_error())
    {
        diagnostic.record(Refusal::PeerResponse);
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
                | file_write::NOT_ENTERED_HEADER
        ) {
            result.headers_mut().append(name, value.clone());
        }
    }
    result
}

pub(super) async fn forward(
    relay: &Relay,
    destination: Destination,
    request: Request,
    body: &[u8],
    permit: capacity::Permit,
    mut diagnostic: Option<RelayDiagnostic>,
) -> Response {
    let deadline = deadline();
    let outcome = send(
        relay,
        destination,
        &request,
        body,
        None,
        deadline,
        diagnostic.as_mut(),
    )
    .await;
    finish(outcome, permit, deadline, diagnostic.as_ref())
}
