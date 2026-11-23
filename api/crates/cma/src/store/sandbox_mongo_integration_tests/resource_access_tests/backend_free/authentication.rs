use std::collections::HashMap;

use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, HeaderMap, Request, StatusCode},
    response::{IntoResponse as _, Response},
    routing::{get, post},
    Form, Json, Router,
};
use serde_json::{json, Value};
use tower::ServiceExt as _;

use super::{Arc, BackendFreeConfig, Client, SocketAddr};
use crate::auth::{
    actor_links::ActorLoginIntent,
    persistence::{AuthStore as _, SecretDigest},
};

pub(super) struct Identity {
    pub(super) origin: String,
    server: tokio::task::JoinHandle<()>,
}

impl Identity {
    pub(super) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let discovery = Arc::new(json!({
            "authorization_endpoint": format!("{origin}/authorize"),
            "token_endpoint": format!("{origin}/token"),
            "userinfo_endpoint": format!("{origin}/userinfo"),
        }));
        let app = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let discovery = discovery.clone();
                    async move { Json((*discovery).clone()) }
                }),
            )
            .route(
                "/token",
                post(|Form(form): Form<HashMap<String, String>>| async move {
                    match form.get("code").map(String::as_str) {
                        Some("fixture-operator-code") => {
                            Json(json!({"access_token":"fixture-operator"})).into_response()
                        }
                        Some("fixture-owner-code") => {
                            Json(json!({"access_token":"fixture-owner"})).into_response()
                        }
                        _ => StatusCode::UNAUTHORIZED.into_response(),
                    }
                }),
            )
            .route(
                "/userinfo",
                get(|headers: HeaderMap| async move {
                    match headers
                        .get(header::AUTHORIZATION)
                        .and_then(|header| header.to_str().ok())
                    {
                        Some("Bearer fixture-operator") => {
                            Json(json!({"sub":"operator","permissions":["cma:admin:settings"]}))
                                .into_response()
                        }
                        Some("Bearer fixture-owner") => {
                            Json(json!({"sub":"owner","permissions":["cma:sandbox:use"]}))
                                .into_response()
                        }
                        _ => StatusCode::UNAUTHORIZED.into_response(),
                    }
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { origin, server }
    }
}

impl Drop for Identity {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub(super) async fn browser_session(app: &Router, code: &str) -> Router {
    let (state, binding) = begin_browser_login(app).await;
    let callback = browser_get(
        app,
        &format!("/auth/callback?state={state}&code={code}"),
        Some(&binding),
    )
    .await;
    assert!(callback.status().is_redirection(), "{}", callback.status());
    let cookie: axum::http::HeaderValue = callback
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|value| {
            value
                .to_str()
                .ok()?
                .split(';')
                .next()
                .filter(|pair| pair.starts_with(crate::auth::AUTH_COOKIE))
                .map(|pair| pair.parse().unwrap())
        })
        .expect("successful callback issues a browser session");
    app.clone().layer(axum::middleware::map_request(
        move |mut request: axum::extract::Request| {
            let cookie = cookie.clone();
            async move {
                request.headers_mut().insert(header::COOKIE, cookie);
                request
            }
        },
    ))
}

async fn begin_browser_login(app: &Router) -> (String, String) {
    let login = browser_get(app, "/auth/login", None).await;
    assert!(login.status().is_redirection());
    let redirect =
        reqwest::Url::parse(login.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let (_, state) = redirect
        .query_pairs()
        .find(|(name, _)| name == "state")
        .unwrap();
    let binding = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    (state.into_owned(), binding.to_owned())
}

pub(super) async fn assert_actor_callback_is_refused(
    app: &Router,
    config: &BackendFreeConfig,
    client: &Client,
    database: &str,
) {
    let (state, binding) = begin_browser_login(app).await;
    let (_, binding_value) = binding.split_once('=').unwrap();
    let auth = crate::store::MongoAuthStore::open(
        client,
        database,
        &config.database.deployment_id,
        &config.encryption_key,
    )
    .await
    .unwrap();
    let mut attempt = auth
        .take_login_attempt(
            &SecretDigest::identify("oidc-login-attempt", &state),
            &SecretDigest::identify("oidc-browser-binding", binding_value),
        )
        .await
        .unwrap()
        .unwrap();
    // Simulate a predecessor that initiated an actor link before changing modes.
    attempt.actor_link_intent = Some(ActorLoginIntent {
        link_id: "fixture-link".into(),
        actor_key: "fixture-actor".into(),
        revision: 1,
    });
    auth.put_login_attempt(attempt).await.unwrap();
    let response = browser_get(
        app,
        &format!("/auth/callback?state={state}&code=fixture-operator-code"),
        Some(&binding),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "sandbox_backend_unavailable");
}

async fn browser_get(app: &Router, path: &str, cookie: Option<&str>) -> Response {
    let mut request =
        Request::get(path).extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()));
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}
