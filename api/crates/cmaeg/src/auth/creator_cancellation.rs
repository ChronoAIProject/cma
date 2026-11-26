//! Cancel-create authority binds an exact human creator to the verified issuer.
use super::{bearer, read_subject, service_error, CreatorAuthError, CreatorAuthenticator};
use axum::http::HeaderMap;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CancelCreateOwner {
    pub issuer: String,
    pub subject: String,
}

impl CreatorAuthenticator {
    pub(crate) async fn authenticate_create_cancellation(
        &self,
        headers: &HeaderMap,
    ) -> Result<CancelCreateOwner, CreatorAuthError> {
        if headers.contains_key("x-cmaeg-owner") {
            return Err(CreatorAuthError::Forbidden);
        }
        let token = bearer(headers).ok_or(CreatorAuthError::NotAuthenticated)?;
        let service = self
            .service
            .as_ref()
            .ok_or(CreatorAuthError::ProviderUnavailable)?;
        let principal = service.authenticate(headers).await.map_err(service_error)?;
        principal
            .require("cma-eg:channels:delete")
            .map_err(service_error)?;

        let issuer = service.issuer();
        let human = tokio::time::timeout(Duration::from_secs(3), async {
            let profile = self
                .client
                .get(format!("{issuer}/api/v1/users/me"))
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| CreatorAuthError::ProviderUnavailable)?;
            read_subject(profile).await
        })
        .await
        .map_err(|_| CreatorAuthError::ProviderUnavailable)??;
        if human.subject != principal.subject || human.user_type.as_deref() != Some("person") {
            return Err(CreatorAuthError::Forbidden);
        }
        Ok(CancelCreateOwner {
            issuer: issuer.to_owned(),
            subject: principal.subject,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::header::AUTHORIZATION,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Responses {
        introspection: Arc<Mutex<Value>>,
        profile: Arc<Mutex<Value>>,
    }

    #[tokio::test]
    async fn cancellation_requires_the_delete_grant_and_the_same_person_from_nyxid() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let expiry = time::OffsetDateTime::now_utc().unix_timestamp() + 60;
        let responses = Responses {
            introspection: Arc::new(Mutex::new(json!({
                "active": true,
                "sub": "fixture-person",
                "iss": issuer,
                "token_type": "access",
                "exp": expiry,
                "permissions": ["cma-eg:channels:delete"]
            }))),
            profile: Arc::new(Mutex::new(json!({
                "id": "fixture-person", "user_type": "person"
            }))),
        };
        let routes = Router::new()
            .route(
                "/oauth/introspect",
                post(|State(responses): State<Responses>| async move {
                    Json(responses.introspection.lock().unwrap().clone())
                }),
            )
            .route(
                "/api/v1/users/me",
                get(|State(responses): State<Responses>| async move {
                    Json(responses.profile.lock().unwrap().clone())
                }),
            )
            .with_state(responses.clone());
        let server = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
        let service = cma_service_auth::NyxIdAuth::new(
            &issuer,
            cma_service_auth::Service::EventGateway,
            "fixture-client",
            "fixture-value".into(),
        )
        .unwrap();
        let auth = CreatorAuthenticator::new(&issuer)
            .unwrap()
            .with_service(service);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer fixture-bearer".parse().unwrap());

        assert_eq!(
            auth.authenticate_create_cancellation(&headers)
                .await
                .unwrap(),
            CancelCreateOwner {
                issuer,
                subject: "fixture-person".into()
            }
        );
        for profile in [
            json!({"id":"other-person", "user_type":"person"}),
            json!({"id":"fixture-person", "user_type":"org"}),
            json!({"id":"fixture-person"}),
        ] {
            *responses.profile.lock().unwrap() = profile;
            assert_eq!(
                auth.authenticate_create_cancellation(&headers).await,
                Err(CreatorAuthError::Forbidden)
            );
        }
        *responses.profile.lock().unwrap() = json!({"id":"fixture-person", "user_type":"person"});
        responses.introspection.lock().unwrap()["permissions"] = json!(["cma-eg:channels:create"]);
        assert_eq!(
            auth.authenticate_create_cancellation(&headers).await,
            Err(CreatorAuthError::Forbidden)
        );
        server.abort();
    }
}
