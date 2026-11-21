//! Shared OAuth cookies must remain isolated from tokens and other service sessions.
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use cma_service_auth::{browser::BrowserAuth, NyxIdAuth, Service};
use mongodb::{
    bson::{doc, Document},
    Client,
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};
#[tokio::test]
#[ignore = "requires the authenticated image-manager Mongo contract fixture"]
async fn oauth_state_is_one_use_cookie_is_opaque_and_csrf_and_revocation_are_enforced() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let mongo = Client::with_uri_str(uri).await.unwrap();
    let db = mongo.database(&format!(
        "cma_auth_contract_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let result = exercise(&db).await;
    db.drop().await.unwrap();
    result.unwrap();
}
async fn exercise(db: &mongodb::Database) -> Result<(), Box<dyn std::error::Error>> {
    cma_service_auth::browser::migrate(db, "im_auth_sessions").await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let issuer = format!("http://{}", listener.local_addr()?);
    let revoked = Arc::new(AtomicBool::new(false));
    let state = (issuer.clone(), revoked.clone());
    let token = uuid::Uuid::new_v4().to_string();
    let granted = token.clone();
    let identity=Router::new().route("/oauth/token",post(move |axum::Form(form):axum::Form<HashMap<String,String>>|{let granted=granted.clone();async move{
   assert_eq!(form["client_id"],"image-client");assert_eq!(form["redirect_uri"],"https://cma.example/api/image-manager/auth/callback");assert_eq!(form["code_verifier"].len(),43);
   Json(json!({"access_token":granted,"token_type":"Bearer"}))
 }})).route("/oauth/introspect",post(|State((issuer,revoked)):State<(String,Arc<AtomicBool>)>|async move{
  Json(json!({"active":!revoked.load(Ordering::SeqCst),"sub":"alice","iss":issuer,"exp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+3600,"token_type":"access","resource":[format!("{issuer}/api/v1/proxy/s/cma-image-manager")],"permissions":["cma-im:images:read"]}))
 })).with_state(state);
    let identity_task = tokio::spawn(async move { axum::serve(listener, identity).await.unwrap() });
    let auth = NyxIdAuth::new(
        &issuer,
        Service::ImageManager,
        "image-client",
        "unused".into(),
    )?;
    let browser = Arc::new(BrowserAuth::new(
        auth,
        db,
        "im_auth_sessions",
        "https://cma.example",
        "/api/image-manager",
        "/image-manager",
        &[7; 32],
    )?);
    let app = browser.protect(
        browser
            .routes()
            .route("/v1/write", post(|| async { "ok" }))
            .route(
                "/v1/echo",
                get(|headers: HeaderMap| async move {
                    headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                }),
            ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let login = http.get(format!("{base}/auth/login")).send().await?;
    let transaction = login.headers()["set-cookie"]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let redirect = url::Url::parse(login.headers()["location"].to_str()?)?;
    let Query(query): Query<HashMap<String, String>> =
        Query::try_from_uri(&format!("/?{}", redirect.query().unwrap()).parse()?).unwrap();
    assert_eq!(query["code_challenge_method"], "S256");
    assert!(query["scope"].contains("roles"));
    let callback = format!(
        "{base}/auth/callback?code=one-time-code&state={}",
        query["state"]
    );
    assert_eq!(http.get(&callback).send().await?.status(), 401);
    let response = http
        .get(&callback)
        .header("cookie", &transaction)
        .send()
        .await?;
    assert_eq!(response.status(), 303);
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|h| h.to_str().ok())
        .find(|h| !h.starts_with("__Host-cma-image-manager-login="))
        .unwrap();
    assert!(!cookie.contains(&token));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Secure"));
    let cookie = cookie.split(';').next().unwrap().to_owned();
    assert_eq!(
        http.get(&callback)
            .header("cookie", &transaction)
            .send()
            .await?
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{base}/auth/session"))
            .header("cookie", &cookie)
            .send()
            .await?
            .status(),
        200
    );
    assert_eq!(
        http.get(format!("{base}/v1/echo"))
            .header("cookie", &cookie)
            .send()
            .await?
            .text()
            .await?,
        format!("Bearer {token}")
    );
    for origin in [None, Some("https://attacker.example")] {
        let mut request = http
            .post(format!("{base}/v1/write"))
            .header("cookie", &cookie);
        if let Some(origin) = origin {
            request = request.header("origin", origin)
        }
        assert_eq!(request.send().await?.status(), 403);
    }
    assert_eq!(
        http.post(format!("{base}/v1/write"))
            .header("cookie", &cookie)
            .header("origin", "https://cma.example")
            .send()
            .await?
            .status(),
        200
    );
    let row = db
        .collection::<Document>("im_auth_sessions")
        .find_one(doc! {"kind":"session"})
        .await?
        .unwrap();
    assert!(!format!("{row:?}").contains(&token));
    revoked.store(true, Ordering::SeqCst);
    assert_eq!(
        http.get(format!("{base}/auth/session"))
            .header("cookie", &cookie)
            .send()
            .await?
            .status(),
        401
    );
    assert_eq!(
        http.post(format!("{base}/auth/logout"))
            .header("cookie", &cookie)
            .header("origin", "https://cma.example")
            .send()
            .await?
            .status(),
        204
    );
    assert!(db
        .collection::<Document>("im_auth_sessions")
        .find_one(doc! {"kind":"session"})
        .await?
        .is_none());
    task.abort();
    identity_task.abort();
    Ok(())
}
