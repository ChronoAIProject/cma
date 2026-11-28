//! Durable public HTTP audit admission and disposition.
use super::{classify, outcome, ActivityRecorder};
use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use cma_am_auth::request_audit::{self, VerifiedActor};
use layer2_core::{
    ActivityChannel, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, DetailKey,
    StoreError, ACTIVITY_OUTCOME_UNCONFIRMED,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tower_http::request_id::RequestId;

const STORAGE_TIMEOUT: Duration = Duration::from_secs(2);
const IN_FLIGHT: usize = 64;

#[derive(Clone)]
pub(super) struct AuditedPeerHop;

pub(crate) struct Admission {
    activity: Arc<ActivityService>,
    slots: Semaphore,
    relay: Option<Arc<crate::agent_manager::relay::Relay>>,
}

impl Admission {
    pub(crate) fn new(
        recorder: &ActivityRecorder,
        relay: Option<&Arc<crate::agent_manager::relay::Relay>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            activity: recorder.activity.clone(),
            slots: Semaphore::new(IN_FLIGHT),
            relay: relay.cloned(),
        })
    }
}

struct Attempt {
    activity: Arc<ActivityService>,
    record: ActivityRecord,
    target: Option<String>,
    reported_channel: Option<ActivityChannel>,
}

#[async_trait::async_trait]
impl request_audit::Attempt for Attempt {
    async fn bind(&self, actor: VerifiedActor) -> Result<(), StoreError> {
        let mut record = self.record.clone().by(actor.subject);
        if let Some(target) = &self.target {
            record.target = Some(ActivityTarget::Resource(target.clone()));
        }
        let channel = match actor.channel {
            ActivityChannel::Api => self.reported_channel.unwrap_or(actor.channel),
            channel => channel,
        };
        record
            .detail
            .record(DetailKey::ActivityChannel, channel.as_str());
        record
            .detail
            .record(DetailKey::CredentialKind, actor.credential.as_str());
        record.detail.record(DetailKey::ActorScope, actor.scope);
        tokio::time::timeout(STORAGE_TIMEOUT, self.activity.identify_attempt(&record))
            .await
            .map_err(|_| StoreError::Unavailable("activity identity write timed out".into()))?
    }
}

pub(crate) async fn audit(
    State(admission): State<Arc<Admission>>,
    request: Request,
    next: Next,
) -> Response {
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return next.run(request).await;
    }
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unknown", MatchedPath::as_str);
    let target = matched_target(route, request.uri().path());
    let route = route.to_owned();
    let method = request.method().clone();
    let key = uuid::Uuid::new_v4().hyphenated().to_string();
    let header = HeaderValue::from_str(&key).expect("UUID is an HTTP header value");
    let reported_channel = match request
        .headers()
        .get("x-cma-client")
        .and_then(|value| value.to_str().ok())
    {
        Some("cli") => Some(ActivityChannel::Cli),
        Some("sdk") => Some(ActivityChannel::Sdk),
        _ => None,
    };
    let ingress = match &admission.relay {
        Some(relay) => match relay.classify_audit_hop(request).await {
            Ok((mut request, true)) => {
                let id = layer1_public::rest::request_id_from_headers(request.headers())
                    .map(str::to_owned);
                request.extensions_mut().insert(AuditedPeerHop);
                return match id {
                    Some(id) => layer2_core::activity::with_request_id(id, next.run(request)).await,
                    None => next.run(request).await,
                };
            }
            Ok((request, false)) => Ok(request),
            Err(status) => Err(status),
        },
        None => Ok(request),
    };
    let Ok(_slot) = admission.slots.try_acquire() else {
        return with_id(unavailable(), header);
    };
    let mut record = ActivityRecord::new(
        classify(&method, &route),
        ActivityOutcome::Failed(ACTIVITY_OUTCOME_UNCONFIRMED.into()),
    )
    .on(ActivityTarget::Resource(route.clone()));
    record.detail.record(DetailKey::RequestId, key.clone());
    record
        .detail
        .record(DetailKey::RequestMethod, method.as_str());
    record.detail.record(DetailKey::RequestRoute, route);
    record.detail.record(
        DetailKey::ActivityChannel,
        ActivityChannel::Unknown.as_str(),
    );
    let reserved = tokio::time::timeout(STORAGE_TIMEOUT, admission.activity.record(&record)).await;
    if !matches!(reserved, Ok(Ok(()))) {
        metrics::counter!("cma_audit_write_total", "stage" => "reserve", "result" => "unavailable")
            .increment(1);
        return with_id(unavailable(), header);
    }
    let attempt = Arc::new(Attempt {
        activity: admission.activity.clone(),
        record,
        target,
        reported_channel,
    });
    let response = match ingress {
        Ok(mut request) => {
            request
                .headers_mut()
                .insert(crate::X_REQUEST_ID, header.clone());
            request
                .extensions_mut()
                .insert(RequestId::new(header.clone()));
            layer2_core::activity::with_request_id(
                key,
                request_audit::scope(attempt.clone(), next.run(request)),
            )
            .await
        }
        Err(status) => status.into_response(),
    };
    let status = outcome(response.status());
    let finished = tokio::time::timeout(
        STORAGE_TIMEOUT,
        admission
            .activity
            .finish_attempt(&attempt.record.id, &status, response.status().as_u16()),
    )
    .await;
    if !matches!(finished, Ok(Ok(()))) {
        metrics::counter!("cma_audit_write_total", "stage" => "finish", "result" => "unconfirmed")
            .increment(1);
    }
    with_id(response, header)
}

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [("content-type", "application/problem+json")],
        axum::Json(serde_json::json!({
            "type":"about:blank", "title":"Audit persistence unavailable",
            "status":503, "code":"activity_admission_unavailable"
        })),
    )
        .into_response()
}

fn with_id(mut response: Response, id: HeaderValue) -> Response {
    response.headers_mut().insert(crate::X_REQUEST_ID, id);
    response
}

fn matched_target(route: &str, path: &str) -> Option<String> {
    let mut previous = "";
    for (template, value) in route.split('/').zip(path.split('/')) {
        let prefix = match (previous, template) {
            ("agents", "{id}" | "{agent_id}") => Some("agt_"),
            ("sandboxes", "{id}" | "{sandbox_id}") => Some("sbx_"),
            ("workspaces", "{id}" | "{workspace_id}") => Some("wks_"),
            _ => None,
        };
        if let Some(prefix) = prefix {
            if value.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            }) {
                return Some(value.to_owned());
            }
        }
        previous = template;
    }
    None
}

#[cfg(test)]
#[path = "ingress_tests.rs"]
pub(super) mod tests;
