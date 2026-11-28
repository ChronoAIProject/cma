//! Durable Gateway request admissions through the configured machine writer.
use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use cma_service_auth::request_audit::{Client, Credential, Event, Identity, Scope, PATH};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};

const DEADLINE: Duration = Duration::from_secs(2);

tokio::task_local! {
    static ATTEMPT: Attempt;
}

pub(crate) struct Writer {
    endpoint: String,
    client: reqwest::Client,
    auth: crate::service_auth::ServiceAuth,
    slots: Semaphore,
}

struct Attempt {
    writer: Arc<Writer>,
    request_id: String,
    client: Client,
    identity: Arc<Mutex<Option<Identity>>>,
}

impl Writer {
    pub(crate) fn configured(
        config: &crate::Config,
        auth: &crate::service_auth::ServiceAuth,
    ) -> Result<Option<Arc<Self>>, Box<dyn std::error::Error + Send + Sync>> {
        let enabled = match std::env::var("CMAEG_REQUEST_AUDIT_ENABLED").as_deref() {
            Err(std::env::VarError::NotPresent) | Ok("false") => false,
            Ok("true") => true,
            _ => return Err("CMAEG_REQUEST_AUDIT_ENABLED must be true or false".into()),
        };
        if !enabled {
            return Ok(None);
        }
        if !matches!(auth, crate::service_auth::ServiceAuth::Account(_)) {
            return Err("Gateway request audit requires the configured service account".into());
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(DEADLINE)
            .build()?;
        Ok(Some(Arc::new(Self {
            endpoint: format!(
                "{}{PATH}",
                config.management.cma_base_url.trim_end_matches('/')
            ),
            client,
            auth: auth.clone(),
            slots: Semaphore::new(64),
        })))
    }

    async fn write(&self, event: Event) -> bool {
        tokio::time::timeout(DEADLINE, async {
            let Ok(bearer) = self.auth.bearer().await else {
                return false;
            };
            for _ in 0..2 {
                match self
                    .client
                    .post(&self.endpoint)
                    .bearer_auth(bearer.as_str())
                    .json(&event)
                    .send()
                    .await
                {
                    Ok(response) if response.status() == StatusCode::NO_CONTENT => return true,
                    Ok(response) if !response.status().is_server_error() => return false,
                    _ => {}
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }
}

pub(crate) fn active() -> bool {
    ATTEMPT.try_with(|_| ()).is_ok()
}

pub(crate) async fn bind(
    actor: &str,
    organization: Option<&str>,
    credential: Credential,
) -> Result<(), crate::auth::CreatorAuthError> {
    let Ok((writer, request_id, client, bound)) = ATTEMPT.try_with(|attempt| {
        (
            attempt.writer.clone(),
            attempt.request_id.clone(),
            attempt.client,
            attempt.identity.clone(),
        )
    }) else {
        return Ok(());
    };
    // Persist independently verified identity before the business handler runs.
    let identity = Identity {
        subject: actor.to_owned(),
        scope: if organization.is_some() {
            Scope::Organization
        } else {
            Scope::Personal
        },
        organization: organization.map(str::to_owned),
        credential,
        client: if credential == Credential::WebSession {
            Client::Web
        } else {
            client
        },
    };
    let mut bound = bound.lock().await;
    if let Some(saved) = bound.as_ref() {
        return if saved == &identity {
            Ok(())
        } else {
            Err(crate::auth::CreatorAuthError::Forbidden)
        };
    }
    if !writer
        .write(Event::Bind {
            request_id,
            identity: identity.clone(),
        })
        .await
    {
        return Err(crate::auth::CreatorAuthError::ProviderUnavailable);
    }
    *bound = Some(identity);
    Ok(())
}

pub(crate) async fn audit(
    State(writer): State<Arc<Writer>>,
    mut request: Request,
    next: Next,
) -> Response {
    if !eligible(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    let Ok(_slot) = writer.slots.try_acquire() else {
        return unavailable();
    };
    let id = uuid::Uuid::new_v4().hyphenated().to_string();
    let header = HeaderValue::from_str(&id).expect("UUID is a header value");
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unknown", MatchedPath::as_str);
    let client = match request
        .headers()
        .get("x-cma-client")
        .and_then(|v| v.to_str().ok())
    {
        Some("cli") => Client::Cli,
        Some("sdk") => Client::Sdk,
        _ => Client::Api,
    };
    if !writer
        .write(Event::Reserve {
            request_id: id.clone(),
            method: request.method().to_string(),
            route: route.to_owned(),
        })
        .await
    {
        let mut response = unavailable();
        response.headers_mut().insert("x-request-id", header);
        return response;
    }
    request.headers_mut().insert("x-request-id", header.clone());
    let attempt = Attempt {
        writer: writer.clone(),
        request_id: id.clone(),
        client,
        identity: Arc::new(Mutex::new(None)),
    };
    let mut response = ATTEMPT.scope(attempt, next.run(request)).await;
    if !writer
        .write(Event::Finish {
            request_id: id,
            status: response.status().as_u16(),
        })
        .await
    {
        metrics::counter!("cmaeg_request_audit_unconfirmed_total").increment(1);
    }
    response.headers_mut().insert("x-request-id", header);
    response
}

fn eligible(method: &Method, path: &str) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) && !matches!(path, "/v1/healthz" | "/v1/readyz")
        && !path.starts_with("/v1/internal/")
        && (path.starts_with("/v1/")
            || path.starts_with("/auth/")
            || path == "/console"
            || (path.starts_with("/console/") && !path.starts_with("/console/assets/")))
}

fn unavailable() -> Response {
    crate::http::problems::problem_document(cmaeg_core::problem::Problem::new(
        cmaeg_core::problem::ProblemCode::ProviderUnavailable,
        503,
        "The request could not be admitted to the Activity log.",
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Administrator, Creator, CreatorAuthState, CreatorAuthenticator};
    use axum::{
        body::Body,
        routing::{get, post},
        Form, Json, Router,
    };
    use tower::ServiceExt;

    #[derive(Clone)]
    struct Auth(CreatorAuthenticator);

    impl CreatorAuthState for Auth {
        fn creator_auth(&self) -> &CreatorAuthenticator {
            &self.0
        }
    }

    #[tokio::test]
    async fn verified_creator_and_admin_refusals_keep_the_actor() {
        let events = Arc::new(std::sync::Mutex::new(Vec::<Event>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mock = Router::new()
            .route(
                PATH,
                post({
                    let events = events.clone();
                    move |Json(event): Json<Event>| {
                        let events = events.clone();
                        async move {
                            events.lock().unwrap().push(event);
                            StatusCode::NO_CONTENT
                        }
                    }
                }),
            )
            .route(
                "/oauth/introspect",
                post({
                    let issuer = origin.clone();
                    move |Form(fields): Form<std::collections::BTreeMap<String, String>>| {
                        let issuer = issuer.clone();
                        async move {
                            let permissions = if fields.get("token").is_some_and(|token| token == "fixture-member") {
                                vec!["cma-eg:channels:create", "cma-eg:admin"]
                            } else {
                                Vec::new()
                            };
                            Json(serde_json::json!({
                                "active": true, "sub": "fixture-verified", "iss": issuer,
                                "token_type": "access", "exp": 4102444800_u64, "permissions": permissions,
                            }))
                        }
                    }
                }),
            )
            .route("/api/v1/users/me", get(|| async {
                Json(serde_json::json!({"subject": "fixture-verified", "user_type": "person"}))
            }))
            .route("/api/v1/orgs/{owner}/authorization", get(|| async { StatusCode::NOT_FOUND }));
        let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        let writer = Arc::new(Writer {
            endpoint: format!("{origin}{PATH}"),
            client: reqwest::Client::new(),
            auth: crate::service_auth::ServiceAuth::Static(cmaeg_core::channel::Secret::new(
                "fixture",
            )),
            slots: Semaphore::new(1),
        });
        let auth = Auth(
            CreatorAuthenticator::new(&origin).unwrap().with_service(
                cma_service_auth::NyxIdAuth::new(
                    &origin,
                    cma_service_auth::Service::EventGateway,
                    "fixture",
                    "fixture".to_owned(),
                )
                .unwrap(),
            ),
        );
        let app = Router::new()
            .route(
                "/v1/channels",
                post(|_: Creator| async { StatusCode::NO_CONTENT }),
            )
            .route(
                "/v1/admin/change",
                post(|_: Administrator| async { StatusCode::NO_CONTENT }),
            )
            .with_state(auth)
            .layer(axum::middleware::from_fn_with_state(writer, audit));
        for (path, bearer, expected) in [
            (
                "/v1/channels",
                Some("Bearer fixture-user"),
                StatusCode::FORBIDDEN,
            ),
            (
                "/v1/admin/change",
                Some("Bearer fixture-user"),
                StatusCode::FORBIDDEN,
            ),
            (
                "/v1/channels",
                Some("Bearer fixture-member"),
                StatusCode::UNAUTHORIZED,
            ),
            (
                "/v1/admin/change",
                Some("Bearer fixture-member"),
                StatusCode::UNAUTHORIZED,
            ),
            ("/v1/channels", None, StatusCode::UNAUTHORIZED),
        ] {
            let mut request = Request::post(path).header("x-cmaeg-owner", "unverified-owner");
            if let Some(bearer) = bearer {
                request = request.header("authorization", bearer);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            let id = response.headers()["x-request-id"].to_str().unwrap();
            let recorded = events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.request_id() == id)
                .cloned()
                .collect::<Vec<_>>();
            assert!(
                matches!(recorded.first(), Some(Event::Reserve { route, .. }) if route == path)
            );
            assert!(
                matches!(recorded.last(), Some(Event::Finish { status, .. }) if *status == expected.as_u16())
            );
            if bearer.is_some() {
                assert!(
                    matches!(recorded.as_slice(), [_, Event::Bind { identity, .. }, _]
                    if identity.subject == "fixture-verified" && identity.scope == Scope::Personal && identity.organization.is_none())
                );
            } else {
                assert_eq!(recorded.len(), 2);
            }
        }
        server.abort();
    }
}
