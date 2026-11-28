//! Serve independent control-plane routes without constructing runtime writers.
use super::*;
use axum::{extract::DefaultBodyLimit, middleware, Router};

pub(super) async fn compose(config: &Config, stores: Arc<MongoStores>) -> Result<Router, String> {
    let (_deletions, standing) = account_deletion::compose(config, &stores).await?;
    let activity = ActivityService::new(stores.activity.clone());
    let users = UserDirectory::new(stores.users.clone());
    let settings = profiles::platform_settings(config, &stores, &activity);
    let outcomes = creation_outcomes::open(config, &stores).await?;
    let auth = credentials::authentication(config, &stores, &users, &activity, settings.clone())?
        .with_account_standing(standing)
        .with_creation_outcomes_enabled(outcomes.is_some());
    let policy = profile_route_policy();
    profiles::seed_profiles(&stores, config, &policy).await?;
    let operations: Arc<dyn OperationStore> = stores.operations.clone();
    let resources = rest::admin::platform_settings_router(settings.clone())
        .merge(rest::profiles_router(profiles::route_state(
            &stores,
            &operations,
            &settings,
            policy,
        )))
        .merge(rest::legal_consent::legal_consent_router(
            legal_consent::service(config, &stores),
        ));
    let resources = creation_outcomes::routes(resources, outcomes.as_deref())
        .merge(rest::router())
        .layer(middleware::from_fn(crate::rest_links));
    let limits = crate::control_admission::limiters(config, stores.request_limits.clone());
    let public = resources
        .merge(crate::auth::account_router(auth.clone()))
        .merge(crate::auth::service_authorization::router(auth.clone()))
        .layer(middleware::from_fn_with_state(
            limits.clone(),
            crate::rate_limit::enforce,
        ))
        .layer(middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ));
    Ok(router(public, crate::health::backend_free_router(stores))
        .layer(middleware::from_fn_with_state(
            limits,
            crate::rate_limit::admit,
        ))
        .layer(DefaultBodyLimit::max(crate::REQUEST_BODY_LIMIT_BYTES)))
}

fn router(public: Router, health: Router) -> Router {
    public.merge(health).fallback(unavailable)
}

async fn unavailable() -> rest::HttpProblem {
    rest::HttpProblem::SandboxBackendUnavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn release_quarantine_serves_discovery_and_refuses_execution() {
        let app = router(
            rest::router(),
            Router::new().route("/livez", get(crate::health::liveness)),
        );
        for (method, path, status) in [
            ("GET", "/livez", StatusCode::OK),
            ("GET", "/api/v1/conformance", StatusCode::OK),
            ("POST", "/api/v1/sandboxes", StatusCode::SERVICE_UNAVAILABLE),
            ("POST", "/api/v2/responses", StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{method} {path}");
        }
    }
}
