//! Service-only Event Gateway audit writes into the retained Activity store.
use super::{classify, outcome, ActivityRecorder};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{Method, StatusCode},
    middleware::from_fn_with_state,
    routing::post,
    Extension, Json, Router,
};
use cma_am_auth::auth::VerifiedAuditWriter;
use cma_service_auth::request_audit::{Client, Credential, Event, Identity, Scope, PATH};
use layer2_core::{
    ActivityId, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, DetailKey,
    SubjectId, ACTIVITY_OUTCOME_UNCONFIRMED,
};
use std::{sync::Arc, time::Duration};

struct Writer {
    principal: SubjectId,
    activity: Arc<ActivityService>,
}

pub(crate) fn router(
    principal: SubjectId,
    recorder: &ActivityRecorder,
    auth: Arc<crate::auth::AuthState>,
) -> Router {
    Router::new()
        .route(PATH, post(write))
        .layer(DefaultBodyLimit::max(2048))
        .layer(from_fn_with_state(auth, crate::auth::attach_subject))
        .with_state(Arc::new(Writer {
            principal,
            activity: recorder.activity.clone(),
        }))
}

async fn write(
    State(writer): State<Arc<Writer>>,
    service: Option<Extension<VerifiedAuditWriter>>,
    Json(event): Json<Event>,
) -> StatusCode {
    if service.is_none_or(|service| service.subject() != &writer.principal) {
        return StatusCode::FORBIDDEN;
    }
    tokio::time::timeout(Duration::from_secs(2), writer.apply(event))
        .await
        .unwrap_or(StatusCode::SERVICE_UNAVAILABLE)
}

impl Writer {
    async fn apply(&self, event: Event) -> StatusCode {
        let request_id = event.request_id();
        if layer1_public::rest::normalize_request_id(request_id) != Some(request_id) {
            return StatusCode::BAD_REQUEST;
        }
        let id = ActivityId(format!("act_eg_{}", request_id));
        let stored = match self.activity.attempt(&id).await {
            Ok(Some(row))
                if row.detail.get(DetailKey::GatewayAuditWriter)
                    == Some(self.principal.as_str()) =>
            {
                Some(row)
            }
            Ok(Some(_)) => return StatusCode::CONFLICT,
            Ok(None) => None,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE,
        };
        match event {
            Event::Reserve {
                request_id,
                method,
                route,
            } => {
                let Ok(verb) = Method::from_bytes(method.as_bytes()) else {
                    return StatusCode::BAD_REQUEST;
                };
                if !matches!(
                    verb,
                    Method::GET
                        | Method::HEAD
                        | Method::POST
                        | Method::PATCH
                        | Method::PUT
                        | Method::DELETE
                ) || !safe_route(&route)
                {
                    return StatusCode::BAD_REQUEST;
                }
                if let Some(row) = stored {
                    return disposition(
                        row.detail.get(DetailKey::RequestMethod) == Some(method.as_str())
                            && row.detail.get(DetailKey::RequestRoute) == Some(route.as_str()),
                    );
                }
                let mut row = ActivityRecord::new(classify(&verb, &route), pending_outcome())
                    .on(ActivityTarget::Resource(route.clone()));
                row.id = id;
                for (key, value) in [
                    (DetailKey::RequestId, request_id.as_str()),
                    (DetailKey::RequestMethod, method.as_str()),
                    (DetailKey::RequestRoute, route.as_str()),
                    (DetailKey::ActivityChannel, "unknown"),
                    (DetailKey::GatewayAuditWriter, self.principal.as_str()),
                ] {
                    row.detail.record(key, value);
                }
                if self.activity.record(&row).await.is_ok() {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            }
            Event::Bind { identity, .. } => {
                let Some(mut row) = stored else {
                    return StatusCode::NOT_FOUND;
                };
                let Some(values) = identity_values(&identity) else {
                    return StatusCode::BAD_REQUEST;
                };
                let subject = SubjectId::new(&identity.subject);
                if let Some(actor) = &row.actor {
                    return disposition(
                        actor == &subject
                            && row.detail.get(DetailKey::ScopeOwner)
                                == identity.organization.as_deref()
                            && values
                                .iter()
                                .all(|(key, expected)| row.detail.get(*key) == Some(*expected)),
                    );
                }
                if row.outcome != pending_outcome() {
                    return StatusCode::CONFLICT;
                }
                row.actor = Some(subject);
                for (key, value) in values {
                    row.detail.record(key, value);
                }
                if let Some(owner) = identity.organization {
                    row.detail.record(DetailKey::ScopeOwner, owner);
                }
                if self.activity.identify_attempt(&row).await.is_ok() {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            }
            Event::Finish { status, .. } => {
                let Some(row) = stored else {
                    return StatusCode::NOT_FOUND;
                };
                let Ok(code) = StatusCode::from_u16(status) else {
                    return StatusCode::BAD_REQUEST;
                };
                if !matches!(status, 101 | 200..=599) {
                    return StatusCode::BAD_REQUEST;
                }
                let result = outcome(code);
                if row.outcome != pending_outcome() {
                    return disposition(
                        row.outcome == result
                            && row.detail.get(DetailKey::HttpStatus)
                                == Some(status.to_string().as_str()),
                    );
                }
                if self
                    .activity
                    .finish_attempt(&id, &result, status)
                    .await
                    .is_ok()
                {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            }
        }
    }
}

fn disposition(exact: bool) -> StatusCode {
    if exact {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CONFLICT
    }
}

fn pending_outcome() -> ActivityOutcome {
    ActivityOutcome::Failed(ACTIVITY_OUTCOME_UNCONFIRMED.into())
}

fn safe_route(route: &str) -> bool {
    route == "unknown"
        || (route.len() <= 256
            && route.starts_with('/')
            && route
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/{}*-._".contains(&byte)))
}

fn identity_values(identity: &Identity) -> Option<[(DetailKey, &'static str); 3]> {
    let valid_subject = |subject: &str| {
        !subject.is_empty()
            && subject.len() <= 256
            && subject.trim() == subject
            && !subject.chars().any(char::is_control)
    };
    if !valid_subject(&identity.subject)
        || (identity.scope == Scope::Organization) != identity.organization.is_some()
        || identity
            .organization
            .as_ref()
            .is_some_and(|owner| !valid_subject(owner))
        || (identity.client == Client::Web) != (identity.credential == Credential::WebSession)
    {
        return None;
    }
    Some([
        (
            DetailKey::ActivityChannel,
            match identity.client {
                Client::Web => "web",
                Client::Api => "api",
                Client::Cli => "cli",
                Client::Sdk => "sdk",
            },
        ),
        (
            DetailKey::CredentialKind,
            match identity.credential {
                Credential::WebSession => "web_session",
                Credential::Bearer => "bearer",
                Credential::OwnerKey => "agent_key",
            },
        ),
        (
            DetailKey::ActorScope,
            match identity.scope {
                Scope::Personal => "personal",
                Scope::Organization => "organization",
            },
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ingress::tests::Store;

    #[tokio::test]
    async fn exact_gateway_attempt_replays_and_a_late_outcome_cannot_restore_an_erased_row() {
        let store = Arc::new(Store::default());
        let writer = Writer {
            principal: SubjectId::new("fixture-gateway-machine"),
            activity: ActivityService::new(store.clone()),
        };
        let request_id = "00000000-0000-4000-8000-000000000001".to_owned();
        let reserve = |route: &str| Event::Reserve {
            request_id: request_id.clone(),
            method: "POST".into(),
            route: route.to_owned(),
        };
        let identify = |subject: &str| Event::Bind {
            request_id: request_id.clone(),
            identity: Identity {
                subject: subject.to_owned(),
                scope: Scope::Organization,
                organization: Some("fixture-organization".to_owned()),
                credential: Credential::Bearer,
                client: Client::Api,
            },
        };
        let finish = |status| Event::Finish {
            request_id: request_id.clone(),
            status,
        };
        let route = "/v1/channels/{channel_id}/outbound-messages";
        assert_eq!(
            writer.apply(identify("fixture-user")).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(writer.apply(reserve(route)).await, StatusCode::NO_CONTENT);
        assert_eq!(writer.apply(reserve(route)).await, StatusCode::NO_CONTENT);
        assert_eq!(
            writer.apply(reserve("/v1/channels")).await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            writer.apply(identify("fixture-user")).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            writer.apply(identify("fixture-user")).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            writer.apply(identify("different-user")).await,
            StatusCode::CONFLICT
        );
        assert_eq!(writer.apply(finish(202)).await, StatusCode::NO_CONTENT);
        assert_eq!(writer.apply(finish(202)).await, StatusCode::NO_CONTENT);
        assert_eq!(writer.apply(finish(403)).await, StatusCode::CONFLICT);
        {
            let rows = store.rows.lock().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].actor, Some(SubjectId::new("fixture-user")));
            assert_eq!(
                rows[0].detail.get(DetailKey::ActorScope),
                Some("organization")
            );
        }
        store.rows.lock().unwrap().clear();
        assert_eq!(writer.apply(finish(202)).await, StatusCode::NOT_FOUND);
        assert!(store.rows.lock().unwrap().is_empty());
    }
}
