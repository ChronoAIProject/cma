use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use layer2_core::{agent_ownership::Member, binding_diagnostics::BindingHoldSample};
use std::{sync::Arc, time::Duration};

use super::{authentication::PROOF_HEADER, valid_sample, BindingDiagnostics, MAX_BODY, PATH};

pub(super) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "binding observation client is unavailable".into())
}

pub(super) async fn read(
    service: &BindingDiagnostics,
    member: &Member,
    principal: &str,
) -> Result<BindingHoldSample, ()> {
    let _slot = service.peer_slots.acquire().await.map_err(|_| ())?;
    let mut url = url::Url::parse(&member.relay_address).map_err(|_| ())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(());
    }
    url.set_path(PATH);
    let (proof, signed) =
        service
            .authentication
            .request(&service.instance, &member.instance_id, principal)?;
    let response = service
        .client
        .get(url)
        .header(PROOF_HEADER, signed)
        .send()
        .await
        .map_err(|_| ())?;
    if response.status() != StatusCode::OK
        || response.headers().get_all(PROOF_HEADER).iter().count() != 1
        || response
            .content_length()
            .is_some_and(|length| length > MAX_BODY as u64)
    {
        return Err(());
    }
    let signature = response
        .headers()
        .get(PROOF_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(())?
        .to_owned();
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > MAX_BODY)
        {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    service
        .authentication
        .verify_response(&proof, &bytes, &signature)?;
    let sample: BindingHoldSample = serde_json::from_slice(&bytes).map_err(|_| ())?;
    valid_sample(&sample)?;
    Ok(sample)
}

pub(super) async fn serve(
    State(service): State<Arc<BindingDiagnostics>>,
    request: Request,
) -> Response {
    let result = tokio::time::timeout(Duration::from_secs(5), serve_inner(&service, request)).await;
    let mut response = match result {
        Ok(Ok(response)) => response,
        Ok(Err(status)) => status.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

async fn serve_inner(
    service: &BindingDiagnostics,
    request: Request,
) -> Result<Response, StatusCode> {
    if !(service.ready)()
        || request.method() != axum::http::Method::GET
        || request.uri().query().is_some()
        || request.headers().get_all(PROOF_HEADER).iter().count() != 1
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let proof = service
        .authentication
        .verify_request(
            request
                .headers()
                .get(PROOF_HEADER)
                .and_then(|value| value.to_str().ok())
                .ok_or(StatusCode::FORBIDDEN)?,
            &service.instance,
        )
        .map_err(|_| StatusCode::FORBIDDEN)?;
    let _slot = service
        .slots
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let body = to_bytes(request.into_body(), 1)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if !body.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    service
        .membership
        .check(&proof.sender, &proof.recipient)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    let sample = service.local.sample();
    valid_sample(&sample).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    service
        .membership
        .check(&proof.sender, &proof.recipient)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    if !(service.ready)() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let bytes = serde_json::to_vec(&sample).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if bytes.len() > MAX_BODY {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let signed = service
        .authentication
        .response(&proof, &bytes)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(PROOF_HEADER, signed)
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
