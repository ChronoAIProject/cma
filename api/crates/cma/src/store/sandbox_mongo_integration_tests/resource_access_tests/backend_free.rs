use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse as _, Response},
    routing::{get, post},
    Form, Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{collections::HashMap, net::SocketAddr, time::Duration};
use tower::ServiceExt as _;

use super::*;
use crate::{
    auth::{
        actor_links::ActorLoginIntent,
        persistence::{AuthStore as _, SecretDigest},
    },
    config::{BackendFreeConfig, StartupConfig},
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn backend_free_http_uses_durable_owner_reads_without_resuming_pending_work() {
    let (client, name) = database("backend_free").await;
    let (store, target) = fixture(&client, &name).await;
    let ResourceTarget::Agent { agent_id } = target else {
        panic!("retained Agent fixture required");
    };
    let mut pending = provisioning_sandbox("sbx_pending_backend_free");
    pending.name = "pending backend-free fixture".into();
    store.create_sandbox(pending.clone()).await.unwrap();
    let db = client.database(&name);
    let sandbox_records = db.collection::<Document>(SANDBOXES);
    let pending_key = doc! {"_id": pending.id.as_str()};
    let before = sandbox_records
        .find_one(pending_key.clone())
        .await
        .unwrap()
        .unwrap();
    let execution_collections = [
        crate::store::sandbox_mongo::SANDBOX_ADMISSIONS,
        crate::store::trigger_definitions::LAUNCHES,
        crate::store::agent_lifecycle_operations::COLLECTION,
    ];
    let mut work_counts = Vec::new();
    for name in execution_collections {
        let collection = db.collection::<Document>(name);
        work_counts.push((
            collection.clone(),
            collection.count_documents(doc! {}).await.unwrap(),
        ));
    }
    drop(store);

    let owner_config = config(&name, Some("owner"));
    let (owner, owner_refresh) = crate::composition::backend_free::compose(&owner_config)
        .await
        .unwrap();
    let (foreign, foreign_refresh) =
        crate::composition::backend_free::compose(&config(&name, Some("other-owner")))
            .await
            .unwrap();
    let identity = Identity::start().await;
    let mut provider = config(&name, None);
    if let crate::auth::AuthMode::Provider { base_url, .. } = &mut provider.authentication.auth_mode
    {
        *base_url = identity.origin.clone();
    }
    let (unauthenticated, unauthenticated_refresh) =
        crate::composition::backend_free::compose(&provider)
            .await
            .unwrap();
    assert_actor_callback_is_refused(&unauthenticated, &provider, &client, &name).await;

    assert_eq!(
        request(&owner, Method::GET, "/livez", None).await.0,
        StatusCode::OK
    );
    let ready = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let (status, ready) = request(&owner, Method::GET, "/ready", None).await;
            if status == StatusCode::OK {
                return ready;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(ready["sandbox"]["available"], false);
    assert_eq!(ready["workers"], json!([]));
    assert_eq!(
        request(&owner, Method::GET, "/auth/session", None).await.0,
        StatusCode::OK
    );

    let (status, inventory) = request(&owner, Method::GET, "/api/v1/agents", None).await;
    assert_eq!(status, StatusCode::OK, "{inventory}");
    assert_eq!(inventory["items"][0]["id"], agent_id.as_str());
    let (status, inventory) = request(&foreign, Method::GET, "/api/v1/agents", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inventory["items"], json!([]));

    let operator = browser_session(&unauthenticated, "fixture-operator-code").await;
    let (status, settings) = request(&operator, Method::GET, "/api/v1/admin/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    let update = json!({
        "expected_revision": settings["revision"].as_u64().unwrap(),
        "usage_reporting": {"measured_visible": true},
    });
    assert_eq!(
        request(
            &operator,
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(update.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &operator,
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(update)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &unauthenticated,
            Method::GET,
            "/api/v1/admin/settings",
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    let desktop = format!("/api/v1/agents/{agent_id}/desktop/ws");
    let events = format!("/api/v1/agents/{agent_id}/events");
    let agui = format!("/api/v1/agui/events?thread_id={agent_id}");
    let agent_events_v2 = format!("/api/v2/agents/{agent_id}/events");
    for (method, path) in [
        (Method::POST, "/api/v1/sandboxes"),
        (Method::GET, &desktop),
        (Method::GET, &events),
        (Method::GET, &agent_events_v2),
        (Method::GET, &agui),
    ] {
        let (status, problem) = request(&owner, method.clone(), path, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {problem}");
        assert_eq!(problem["code"], "sandbox_backend_unavailable");
        let (status, _) = request(&unauthenticated, method, path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    assert_eq!(
        request(&foreign, Method::GET, &desktop, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&foreign, Method::GET, &agui, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &owner,
            Method::GET,
            "/api/v1/agui/launches/absent/events",
            None,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&owner, Method::PATCH, &desktop, None).await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        request(
            &owner,
            Method::GET,
            "/api/v1/unknown-backend-free-route",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let signed_in = browser_session(&unauthenticated, "fixture-owner-code").await;
    assert_eq!(
        request(&signed_in, Method::GET, "/api/v1/agents", None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&signed_in, Method::GET, "/api/v1/admin/settings", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&signed_in, Method::GET, &desktop, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&signed_in, Method::POST, "/api/v1/sandboxes", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    drop(signed_in);

    drop((owner, foreign, unauthenticated, operator));
    for handle in [owner_refresh, foreign_refresh, unauthenticated_refresh] {
        handle.abort();
        let _ = handle.await;
    }
    assert_eq!(
        sandbox_records
            .find_one(pending_key)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(sandbox_records.count_documents(doc! {}).await.unwrap(), 2);
    for (collection, before) in work_counts {
        assert_eq!(collection.count_documents(doc! {}).await.unwrap(), before);
    }
    db.drop().await.unwrap();
}

struct Identity {
    origin: String,
    server: tokio::task::JoinHandle<()>,
}

impl Identity {
    async fn start() -> Self {
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

async fn browser_session(app: &Router, code: &str) -> Router {
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

async fn assert_actor_callback_is_refused(
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

fn config(database: &str, trusted: Option<&str>) -> BackendFreeConfig {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let selected = StartupConfig::from_lookup(|key| {
        Ok(match key {
            "CHRONO_SANDBOX_PROVIDER" => Some("none".into()),
            "CHRONO_ENVIRONMENT" => Some("staging".into()),
            "CHRONO_MONGODB_URI" => Some(uri.clone()),
            "CHRONO_MONGODB_DATABASE" => Some(database.into()),
            "CHRONO_DEPLOYMENT_ID" => Some("fixture".into()),
            "CHRONO_AUTH_ENCRYPTION_KEY" => Some(STANDARD.encode([9; 32])),
            "CHRONO_AUTH_MODE" => trusted.map(|_| "trusted".into()),
            "CHRONO_TRUSTED_SUBJECT" => trusted.map(str::to_owned),
            "CHRONO_NYXID_BASE_URL" | "CHRONO_NYXID_PUBLIC_URL" => {
                Some("https://identity.example.test".into())
            }
            "CHRONO_NYXID_CLIENT_ID" => Some("fixture".into()),
            "CHRONO_NYXID_CLIENT_SECRET" => Some(uuid::Uuid::new_v4().simple().to_string()),
            _ => None,
        })
    })
    .unwrap();
    let StartupConfig::BackendFree(config) = selected else {
        panic!("backend-free mode not selected");
    };
    *config
}

async fn request(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()));
    if path.ends_with("/desktop/ws") {
        request = request
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(
                header::SEC_WEBSOCKET_KEY,
                STANDARD.encode(uuid::Uuid::new_v4().as_bytes()),
            );
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
