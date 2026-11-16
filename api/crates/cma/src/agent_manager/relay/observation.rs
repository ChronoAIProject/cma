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
use futures_util::StreamExt;
use layer2_core::sandbox::{agent_observation::observation_scope, AgentReadService};

pub(super) struct Observation {
    store: Arc<MongoAgentObservation>,
    reads: Arc<AgentReadService>,
    commands: Option<Arc<layer2_core::sandbox::agent_io::AgentCommandService>>,
    client: reqwest::Client,
}
impl Relay {
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
            .get::<layer2_core::resource_access::CallerContext>()
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
    let Some(observation) = relay.observation.get() else {
        return http::unavailable();
    };
    if !relay.coordinator.is_ready() {
        return http::unavailable();
    }
    let Ok(permit) = relay.slots.clone().try_acquire_owned() else {
        return http::unavailable();
    };
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
        return http::unavailable();
    };
    let (parts, body) = request.into_parts();
    let bytes = match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, limit)).await {
        Ok(Ok(bytes)) => bytes,
        _ => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let mut request = Request::from_parts(parts, Body::from(bytes.clone()));
    let Some(context) = request
        .extensions()
        .get::<layer2_core::resource_access::CallerContext>()
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let credential = request
        .extensions()
        .get::<layer1_public::DelegatedCredential>()
        .map(layer1_public::DelegatedCredential::value);
    let id = if let Some(tail) = request.uri().path().strip_prefix("/api/v2/responses/") {
        let Some(commands) = &observation.commands else {
            return http::unavailable();
        };
        let id = layer2_core::ids::AgentResponseId(
            tail.split('/').next().unwrap_or_default().to_owned(),
        );
        match commands.response_status(context, credential, &id).await {
            Ok(status) => status.receipt.agent_id,
            Err(_) => return public_access::refusal(StatusCode::NOT_FOUND),
        }
    } else {
        match agent(&request, &bytes) {
            Ok(id) => id,
            Err(_) if request.uri().path().starts_with("/api/v2/") => {
                return next.run(request).await
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
        Err(_) => return public_access::refusal(StatusCode::NOT_FOUND),
    };
    let Some(created) = authorized.incarnation() else {
        return http::unavailable();
    };
    let destination = match tokio::time::timeout(
        Duration::from_secs(5),
        observation.store.destination(&id, created),
    )
    .await
    {
        Ok(Ok(value)) => value,
        _ => return http::unavailable(),
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
        if verified.owner != token || token.instance_id != relay.coordinator.instance_id {
            return http::unavailable();
        }
        let membership = match bounded(relay.coordinator.store.membership()).await {
            Ok(value) => value,
            Err(_) => return http::unavailable(),
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
        return observation_scope(destination.authority, next.run(request)).await;
    }
    if peer.is_some() {
        return http::unavailable();
    }
    forward(relay, observation, destination, request, &bytes, permit).await
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
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Response {
    let mut headers = HeaderMap::new();
    for (name, value) in ingress::headers(&request) {
        if authentication::forwarded(name.as_str()) {
            headers.append(name, value.clone());
        }
    }
    let proof = match relay.authentication.sign(
        &relay.coordinator.instance_id,
        &wire_token(&destination),
        request.method(),
        request.uri(),
        &headers,
        body,
    ) {
        Ok(value) => value,
        Err(_) => return http::unavailable(),
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
        Duration::from_secs(60),
        observation
            .client
            .request(request.method().clone(), url)
            .headers(headers)
            .header(PROOF, proof)
            .body(body.to_vec())
            .send(),
    )
    .await
    {
        Ok(Ok(value)) => value,
        _ => return http::unavailable(),
    };
    let status = response.status();
    let headers = response.headers().clone();
    // No prefetch queue. Slow callers apply transport backpressure; each next
    // data frame is authorized by the read owner when the body resumes.
    let stream = futures_util::stream::unfold(
        (response.bytes_stream(), permit),
        |(mut stream, permit)| async move {
            stream.next().await.map(|value| {
                (
                    value.map_err(|_| std::io::Error::other("read relay ended")),
                    (stream, permit),
                )
            })
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
        ) {
            result.headers_mut().append(name, value.clone());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_reads_and_concrete_submissions_use_only_the_documented_paths() {
        for path in [
            "/api/v2/responses/resp_one",
            "/api/v2/responses/resp_one/events",
            "/api/v2/responses/resp_one/output",
        ] {
            assert!(eligible(&Request::get(path).body(Body::empty()).unwrap()));
        }
        for path in [
            "/api/v2/responses/resp_one/events/extra",
            "/api/v2/responses/resp_one/cancel",
        ] {
            assert!(!eligible(&Request::get(path).body(Body::empty()).unwrap()));
        }
        let direct = Request::post("/api/v2/responses")
            .body(Body::empty())
            .unwrap();
        assert!(eligible(&direct));
        for body in [
            br#"{"conversation":"agt_one"}"#.as_slice(),
            br#"{"conversation":{"id":"agt_one"}}"#,
        ] {
            assert_eq!(agent(&direct, body).unwrap(), AgentId("agt_one".into()));
        }
        for body in [
            b"{}".as_slice(),
            b"malformed",
            br#"{"conversation":"invalid/id"}"#,
        ] {
            assert!(agent(&direct, body).is_err());
        }
        let alias = Request::post("/api/v2/agents/agt_one/responses")
            .body(Body::empty())
            .unwrap();
        assert!(eligible(&alias));
        assert_eq!(agent(&alias, b"{}").unwrap(), AgentId("agt_one".into()));
    }
    #[test]
    fn observation_routes_bind_exact_agent_and_refuse_ambiguous_or_malformed_input() {
        for path in [
            "/api/v1/agui/events?thread_id=agt_one",
            "/api/v2/agents/agt_one/events",
        ] {
            let request = Request::get(path).body(Body::empty()).unwrap();
            assert!(eligible(&request));
            assert_eq!(agent(&request, &[]).unwrap(), AgentId("agt_one".into()));
        }
        for path in [
            "/api/v1/agui/events?thread_id=agt_one&thread_id=agt_two",
            "/api/v1/agui/events",
            "/api/v2/agents/invalid%2Fid/events",
        ] {
            let request = Request::get(path).body(Body::empty()).unwrap();
            assert!(agent(&request, &[]).is_err());
        }
        let post = Request::post("/api/v1/agui/run")
            .body(Body::empty())
            .unwrap();
        assert!(eligible(&post));
        assert_eq!(
            agent(&post, br#"{"threadId":"agt_one"}"#).unwrap(),
            AgentId("agt_one".into())
        );
        for body in [br#"{}"#.as_slice(), br#"{"threadId":9}"#, b"malformed"] {
            assert!(agent(&post, body).is_err());
        }
        assert!(!eligible(
            &Request::get("/api/v2/agents/agt_one/events/snapshot")
                .body(Body::empty())
                .unwrap()
        ));
        assert!(!eligible(
            &Request::post("/api/v2/agents/agt_one/events")
                .body(Body::empty())
                .unwrap()
        ));
    }
}
