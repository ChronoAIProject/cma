use super::*;
use axum::{extract::Query, response::Redirect};
use base64::Engine;
use sha2::{Digest, Sha256};

pub(super) struct Grant {
    token: String,
    redirect_uri: String,
    challenge: String,
}

// Synthetic consent selects an identity; the application still performs its
// real state/binding checks, PKCE exchange and encrypted browser-custody writes.
pub(super) async fn authorize(
    State(state): State<Arc<StateData>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], "revocation-contract");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(
        query["redirect_uri"],
        "https://cma.example.test/auth/callback"
    );
    let subject = &query["contract_subject"];
    assert!(matches!(subject.as_str(), "owner" | "other-owner"));
    let token = uuid::Uuid::new_v4().to_string();
    state.identities.write().await.insert(
        token.clone(),
        Identity {
            subject: subject.clone(),
            active: true,
            allowed: true,
        },
    );
    let code = uuid::Uuid::new_v4().to_string();
    state.grants.write().await.insert(
        code.clone(),
        Grant {
            token,
            redirect_uri: query["redirect_uri"].clone(),
            challenge: query["code_challenge"].clone(),
        },
    );
    let mut callback = reqwest::Url::parse(&query["redirect_uri"]).unwrap();
    callback
        .query_pairs_mut()
        .extend_pairs([("state", &query["state"]), ("code", &code)]);
    Redirect::to(callback.as_str()).into_response()
}

pub(super) async fn token(
    State(state): State<Arc<StateData>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    state.exchanges.fetch_add(1, Ordering::SeqCst);
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["client_id"], "revocation-contract");
    assert!(form
        .get("client_secret")
        .is_some_and(|s| s == &state.client_secret));
    let Some(grant) = state.grants.write().await.remove(&form["code"]) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_grant"})),
        )
            .into_response();
    };
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(form["code_verifier"].as_bytes()));
    assert_eq!(challenge, grant.challenge);
    assert_eq!(form["redirect_uri"], grant.redirect_uri);
    Json(json!({"access_token":grant.token,"expires_in":3600})).into_response()
}
