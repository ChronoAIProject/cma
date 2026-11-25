use super::*;
use crate::auth::{
    persistence::{AuthStore, BrowserLogin, DurableSubjectTokens, SecretDigest, SubjectAuthState},
    profile::Profile,
    AuthMode, AuthState,
};
use axum::{
    http::HeaderMap,
    routing::{get, post},
    Form, Json, Router,
};
use serde_json::json;
use std::time::{Duration, SystemTime};

pub(super) struct Identity {
    base: String,
    server: tokio::task::JoinHandle<()>,
    store: Arc<crate::store::MongoAuthStore>,
    choices: Arc<Mutex<Option<Vec<serde_json::Value>>>>,
}
impl Drop for Identity {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Identity {
    pub async fn new(client: &Client, name: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"userinfo_endpoint":format!("{base}/userinfo"),"introspection_endpoint":format!("{base}/introspect")});
        let choices = Arc::new(Mutex::new(None::<Vec<serde_json::Value>>));
        let listed_choices = choices.clone();
        let membership_choices = choices.clone();
        let app = Router::new()
            .route("/api/v1/orgs", get(move || async move {
                Json(json!({"orgs": listed_choices.lock().unwrap().as_deref().unwrap_or_default()}))
            }))
            .route("/api/v1/orgs/{id}/authorization", get(move |axum::extract::Path(id): axum::extract::Path<String>| async move {
                let choices = membership_choices.lock().unwrap();
                let membership = match choices.as_ref() {
                    Some(choices) => choices.iter().find(|choice| choice["id"] == id).cloned(),
                    None if id == "owner" || id == "org" => Some(json!({"id": id, "your_role":"viewer"})),
                    None => None,
                };
                membership.map(Json).ok_or(axum::http::StatusCode::FORBIDDEN)
            }))
            .route(
                "/.well-known/openid-configuration",
                get(move || async move { Json(metadata) }),
            )
            .route(
                "/userinfo",
                get(|headers: HeaderMap| async move {
                    let token = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .strip_prefix("Bearer ")
                        .unwrap_or("")
                        .to_owned();
                    Json(json!({"sub":token_owner(&token).as_str(),"permissions":["cma:*"]}))
                }),
            )
            .route(
                "/introspect",
                post(|Form(form): Form<std::collections::HashMap<String,String>>| async move {
                    Json(json!({"active":true,"sub":token_owner(form.get("token").map(String::as_str).unwrap_or("")).as_str(),"permissions":["cma:*"]}))
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Arc::new(
            crate::store::MongoAuthStore::open(client, name, "agent-list-test", &encryption_key())
                .await
                .unwrap(),
        );
        let identity = Self {
            base,
            server,
            store,
            choices,
        };
        identity.add_reader(0).await;
        identity
    }
    pub async fn add_reader(&self, reader: usize) {
        self.store
            .put_browser_login(BrowserLogin {
                id: SecretDigest::identify("browser-login", &cookie(reader)),
                subject: owner(reader),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            })
            .await
            .unwrap();
        self.store
            .insert_subject(SubjectAuthState::new(
                owner(reader),
                crate::permissions::capabilities_from_permissions(["cma:*"]),
                Profile::default(),
                Some(DurableSubjectTokens {
                    access_token: SecretValue::new(format!("agent-list-access-{reader}")),
                    refresh_token: Some(SecretValue::new(format!("agent-list-refresh-{reader}"))),
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                }),
            ))
            .await
            .unwrap();
    }
    pub fn auth(&self, joined: bool) -> Arc<AuthState> {
        AuthState::with_store(
            AuthMode::Provider {
                base_url: self.base.clone(),
                client_id: "agent-list-client".into(),
                client_secret: "agent-list-fixture".into(),
                public_url: "https://example.test".into(),
            },
            None,
            self.store.clone(),
        )
        .with_browser_session_join(joined)
    }
    pub(super) fn set_organization_choices(&self, choices: Vec<serde_json::Value>) {
        *self.choices.lock().unwrap() = Some(choices);
    }
    pub async fn revoke(&self, reader: usize) {
        self.store
            .revoke_browser_binding(&SecretDigest::identify("browser-login", &cookie(reader)))
            .await
            .unwrap();
    }
}

pub(super) fn owner(reader: usize) -> SubjectId {
    SubjectId::new(if reader == 0 {
        "owner".into()
    } else {
        format!("owner-{reader}")
    })
}
pub(super) fn cookie(reader: usize) -> String {
    if reader == 0 {
        "agent-list-cookie".into()
    } else {
        format!("agent-list-cookie-{reader}")
    }
}
fn token_owner(token: &str) -> SubjectId {
    owner(
        token
            .strip_prefix("agent-list-access-")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
    )
}
