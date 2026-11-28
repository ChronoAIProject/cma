//! Share read custody through the authenticated peer transport. Read epochs
//! never enter native effect authorization or acquire a runtime.
use super::*;
use crate::store::agent_observation::{MongoAgentObservation, ObservationDestination};
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use layer2_core::{
    resource_access::CallerContext,
    sandbox::{agent_observation::observation_scope, AgentReadService},
};

pub(super) struct Observation {
    store: Arc<MongoAgentObservation>,
    reads: Arc<AgentReadService>,
    commands: Option<Arc<layer2_core::sandbox::agent_io::AgentCommandService>>,
    client: reqwest::Client,
}
impl Relay {
    pub(crate) fn enable_observation_open_trace(&self) {
        self.observation_open_trace.get_or_init(|| ());
    }

    pub(crate) fn install_observation(
        &self,
        store: Arc<MongoAgentObservation>,
        reads: Arc<AgentReadService>,
        commands: Option<Arc<layer2_core::sandbox::agent_io::AgentCommandService>>,
    ) -> Result<(), String> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| "observation relay client unavailable")?;
        self.observation
            .set(Observation {
                store,
                reads,
                commands,
                client,
            })
            .map_err(|_| "observation already installed".into())
    }
}

pub(super) fn eligible(request: &Request) -> bool {
    let path = request.uri().path();
    let response_read = path.strip_prefix("/api/v2/responses/").is_some_and(|tail| {
        let parts: Vec<_> = tail.split('/').collect();
        !parts[0].is_empty()
            && (parts.len() == 1 || (parts.len() == 2 && matches!(parts[1], "events" | "output")))
    });
    let response_submit = (path == "/api/v2/responses"
        || path
            .strip_prefix("/api/v2/agents/")
            .is_some_and(|tail| tail.ends_with("/responses") && tail.split('/').count() == 2))
        && !request
            .extensions()
            .get::<CallerContext>()
            .is_some_and(|context| {
                context.credential_source == layer2_core::resource_access::CredentialSource::Binding
            });
    (request.method() == Method::GET
        && (response_read
            || path == "/api/v1/agui/events"
            || path
                .strip_prefix("/api/v2/agents/")
                .is_some_and(|tail| tail.ends_with("/events") && tail.split('/').count() == 2)))
        || (request.method() == Method::POST && (path == "/api/v1/agui/run" || response_submit))
}

#[path = "observation_admission.rs"]
mod admission;
#[path = "observation_control.rs"]
mod control;
#[path = "observation_trace.rs"]
mod trace;
use trace::{Decision, OpenTrace};

fn public_response_snapshot(request: &Request) -> bool {
    request.method() == Method::GET
        && request.headers().get(PROOF).is_none()
        && request
            .uri()
            .path()
            .strip_prefix("/api/v2/responses/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

async fn response_snapshot_fallback(response: Response, next: Next, request: Request) -> Response {
    if response.status() != StatusCode::SERVICE_UNAVAILABLE
        || response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .is_none_or(|value| value != "application/problem+json")
    {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, 4096).await else {
        return http::unavailable();
    };
    if serde_json::from_slice::<serde_json::Value>(&bytes)
        .is_ok_and(|value| value["code"] == "agent_owner_unavailable")
    {
        // The public snapshot handler reauthorizes the same request and reads
        // durable history. This path creates no subscription or execution.
        return control::durable(next, request).await;
    }
    Response::from_parts(parts, Body::from(bytes))
}

fn agent(request: &Request, body: &[u8]) -> Result<AgentId, ()> {
    let path = request.uri().path();
    let id = if path == "/api/v2/responses" {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|body| {
                body.get("conversation")
                    .and_then(|value| {
                        value
                            .as_str()
                            .or_else(|| value.get("id").and_then(|id| id.as_str()))
                    })
                    .map(str::to_owned)
            })
    } else if request.method() == Method::POST && path.ends_with("/responses") {
        path.strip_prefix("/api/v2/agents/")
            .and_then(|tail| tail.strip_suffix("/responses"))
            .map(str::to_owned)
    } else if path == "/api/v1/agui/run" {
        serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| {
                value
                    .get("threadId")
                    .and_then(|id| id.as_str())
                    .map(str::to_owned)
            })
    } else if path == "/api/v1/agui/events" {
        let url = reqwest::Url::parse(&format!("http://read.invalid{}", request.uri()))
            .map_err(|_| ())?;
        let ids: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key == "thread_id")
            .map(|(_, value)| value.into_owned())
            .collect();
        (ids.len() == 1).then(|| ids[0].clone())
    } else {
        path.strip_prefix("/api/v2/agents/")
            .and_then(|tail| tail.strip_suffix("/events"))
            .map(str::to_owned)
    }
    .ok_or(())?;
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(());
    }
    Ok(AgentId(id))
}

pub(super) async fn route(relay: &Relay, request: Request, next: Next) -> Response {
    // The dispatcher admits only eligible observation routes here.
    let path = request.uri().path();
    let responses_v2 = path.starts_with("/api/v2/responses") || path.ends_with("/responses");
    if responses_v2 && request.extensions().get::<CallerContext>().is_none() {
        return layer1_public::responses::v2::authentication_required();
    }
    let mut trace = OpenTrace::from_request(relay, &request);
    let Some(observation) = relay.observation.get() else {
        return trace::refuse(trace.as_ref(), Decision::ServiceMissing, None, None);
    };
    if !relay.coordinator.is_ready() {
        return trace::refuse(trace.as_ref(), Decision::CoordinatorNotReady, None, None);
    }
    let limit = if request.method() == Method::GET {
        0
    } else {
        crate::REQUEST_BODY_LIMIT_BYTES
    };
    let Ok(_budget) = relay
        .body_budget
        .clone()
        .try_acquire_many_owned(limit as u32)
    else {
        return trace::refuse(trace.as_ref(), Decision::BodyBudget, None, None);
    };
    let (parts, body) = request.into_parts();
    let bytes = match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, limit)).await {
        Ok(Ok(bytes)) => bytes,
        _ => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let mut request = Request::from_parts(parts, Body::from(bytes.clone()));
    let snapshot_fallback = public_response_snapshot(&request);
    let control_fallback =
        request.headers().get(PROOF).is_none() && control::approval_reply(&request, &bytes);
    let permit = match relay.acquire(capacity::Class::Observer) {
        Ok(permit) => permit,
        Err(_) if control_fallback => {
            return control::durable(next, request).await;
        }
        Err(_) => return capacity::refused(true),
    };
    let Some(context) = request.extensions().get::<CallerContext>() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let credential = request
        .extensions()
        .get::<layer1_public::DelegatedCredential>()
        .map(layer1_public::DelegatedCredential::value);
    let id = if let Some(tail) = request.uri().path().strip_prefix("/api/v2/responses/") {
        let Some(commands) = &observation.commands else {
            return trace::refuse(trace.as_ref(), Decision::CommandServiceMissing, None, None);
        };
        let id = layer2_core::ids::AgentResponseId(
            tail.split('/').next().unwrap_or_default().to_owned(),
        );
        match commands.response_status(context, credential, &id).await {
            Ok(status) => status.receipt.agent_id,
            Err(_) => return public_access::refusal(StatusCode::NOT_FOUND.into()),
        }
    } else {
        match agent(&request, &bytes) {
            Ok(id) => id,
            Err(_) if request.uri().path().starts_with("/api/v2/") => {
                return next.run(request).await;
            }
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        }
    };
    // Both replicas authenticate actual wire credentials independently. The read
    // proof only selects custody; the public handler still authorizes commands.
    let authorized = match observation
        .reads
        .authorize_event_stream(context, credential, &id)
        .await
    {
        Ok(value) => value,
        Err(_) => return public_access::refusal(StatusCode::NOT_FOUND.into()),
    };
    let Some(created) = authorized.incarnation() else {
        return trace::refuse(trace.as_ref(), Decision::IncarnationMissing, None, None);
    };
    let destination = match tokio::time::timeout(
        Duration::from_secs(5),
        observation.store.destination(&id, created),
    )
    .await
    {
        Ok(Ok(value)) => value,
        _ if snapshot_fallback => {
            return http::retain(control::durable(next, request).await, permit);
        }
        Ok(Err(_)) => {
            return trace::refuse(trace.as_ref(), Decision::DestinationUnavailable, None, None);
        }
        Err(_) => {
            return trace::refuse(trace.as_ref(), Decision::DestinationDeadline, None, None);
        }
    };
    let token = wire_token(&destination);
    let peer = request.headers().get(PROOF).cloned();
    if let Some(raw) = &peer {
        if request.headers().get_all(PROOF).iter().count() != 1 {
            return StatusCode::FORBIDDEN.into_response();
        }
        let verified = raw.to_str().ok().and_then(|raw| {
            relay
                .authentication
                .verify(
                    raw,
                    request.method(),
                    request.uri(),
                    ingress::headers(&request),
                    &bytes,
                )
                .ok()
        });
        let Some(verified) = verified else {
            return StatusCode::FORBIDDEN.into_response();
        };
        if let Some(trace) = &mut trace {
            trace.peer_verified = true;
        }
        if verified.owner != token || token.instance_id != relay.coordinator.instance_id {
            return trace::refuse(
                trace.as_ref(),
                Decision::PeerOwnerMismatch,
                Some(&token),
                Some(verified.owner.epoch),
            );
        }
        let membership = match bounded(relay.coordinator.store.membership()).await {
            Ok(value) => value,
            Err(_) => {
                return trace::refuse(
                    trace.as_ref(),
                    Decision::MembershipUnavailable,
                    Some(&token),
                    None,
                );
            }
        };
        if membership.require_compatible().is_err()
            || !membership.members.iter().any(|member| {
                member.instance_id == verified.sender
                    && member.is_live(membership.now_ms)
                    && !member.draining
            })
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        request.headers_mut().remove(PROOF);
    }
    if token.instance_id == relay.coordinator.instance_id {
        // Only a verified peer may request the internal admission envelope.
        let envelope = peer.is_some() && admission::requested(ingress::headers(&request));
        request.headers_mut().remove(admission::HEADER);
        let response = async move {
            let _budget = _budget;
            let response = live::admitted_observation(
                id,
                observation_scope(destination.authority, next.run(request)),
            )
            .await;
            if let Some(trace) = trace {
                trace.opened(&response, &token);
            }
            response
        };
        let response = if envelope {
            admission::acknowledge(response)
        } else {
            response.await
        };
        return http::retain(response, permit);
    }
    if peer.is_some() {
        return trace::refuse(
            trace.as_ref(),
            Decision::PeerReachedNonowner,
            Some(&token),
            None,
        );
    }
    if snapshot_fallback || control_fallback {
        let (parts, body) = request.into_parts();
        let fallback = Request::from_parts(parts.clone(), Body::from(bytes.clone()));
        let response = forward(
            relay,
            observation,
            destination,
            Request::from_parts(parts, body),
            &bytes,
            trace.as_ref(),
        )
        .await;
        if snapshot_fallback {
            return http::retain(
                response_snapshot_fallback(response, next, fallback).await,
                permit,
            );
        }
        let response = http::retain(response, permit);
        return control::remote_fallback(response, next, fallback).await;
    }
    http::retain(
        forward(
            relay,
            observation,
            destination,
            request,
            &bytes,
            trace.as_ref(),
        )
        .await,
        permit,
    )
}

// This envelope is signed over the exact public read URI. It is never converted
// to native Authority; effect routes cannot replay a proof for this URI.
fn wire_token(destination: &ObservationDestination) -> OwnershipToken {
    let token = &destination.authority.token;
    OwnershipToken {
        protocol: layer2_core::agent_ownership::PROTOCOL_VERSION,
        agent_id: token.agent_id.clone(),
        instance_id: token.instance_id.clone(),
        epoch: token.epoch,
    }
}

async fn forward(
    relay: &Relay,
    observation: &Observation,
    destination: ObservationDestination,
    request: Request,
    body: &[u8],
    trace: Option<&OpenTrace>,
) -> Response {
    let mut headers = HeaderMap::new();
    for (name, value) in ingress::headers(&request) {
        if authentication::forwarded(name.as_str()) {
            headers.append(name, value.clone());
        }
    }
    // Negotiation is covered by the same proof as the exact caller and URI.
    headers.insert(admission::HEADER, admission::VERSION.parse().unwrap());
    let token = wire_token(&destination);
    let headers = match relay.authentication.signed_headers(
        &relay.coordinator.instance_id,
        &token,
        request.method(),
        request.uri(),
        headers,
        body,
    ) {
        Ok(value) => value,
        Err(_) => return trace::refuse(trace, Decision::SigningFailed, Some(&token), None),
    };
    let url = format!(
        "{}{}",
        destination.member.relay_address.trim_end_matches('/'),
        request
            .uri()
            .path_and_query()
            .map_or("/", |value| value.as_str())
    );
    let response = match tokio::time::timeout(
        LIVE_ADMISSION_DEADLINE,
        observation
            .client
            .request(request.method().clone(), url)
            .headers(headers)
            .body(body.to_vec())
            .send(),
    )
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => {
            return trace::refuse(trace, Decision::PeerTransportFailed, Some(&token), None)
        }
        Err(_) => return trace::refuse(trace, Decision::PeerAdmissionDeadline, Some(&token), None),
    };
    // The peer has admitted capacity and custody. Cold replay now runs under
    // the existing read/transport bounds, outside the admission deadline.
    match admission::response(response).await {
        Ok(response) => {
            if let Some(trace) = trace {
                trace.forwarded(&response, &token);
            }
            response
        }
        Err(()) => trace::refuse(trace, Decision::AdmissionDecodeFailed, Some(&token), None),
    }
}

#[cfg(test)]
#[path = "observation_tests.rs"]
mod tests;
