use std::sync::Arc;
use std::time::SystemTime;

use mongodb::bson::{Bson, Document};

use super::*;

pub(super) async fn stores() -> (Client, String, Arc<MongoAuthStore>, Arc<MongoAuthStore>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a disposable replica set");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let other_client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_refresh_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let first = Arc::new(
        MongoAuthStore::open(&client, &name, "refresh-test", &key)
            .await
            .unwrap(),
    );
    let second = Arc::new(
        MongoAuthStore::open(&other_client, &name, "refresh-test", &key)
            .await
            .unwrap(),
    );
    (client, name, first, second)
}

fn initial() -> SubjectAuthState {
    SubjectAuthState::new(
        SubjectId::new("refresh-owner"),
        Authorization::none(),
        Profile::default(),
        Some(DurableSubjectTokens {
            access_token: SecretValue::new("fixture-access".into()),
            refresh_token: Some(SecretValue::new("fixture-refresh".into())),
            expires_at: SystemTime::UNIX_EPOCH,
        }),
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_refresh_cas_fences_replicas_and_incarnations_without_upsert() {
    let (client, name, first, second) = stores().await;
    let original = initial();
    assert!(first.insert_subject(original.clone()).await.unwrap());
    assert!(!second.insert_subject(original.clone()).await.unwrap());
    let claim = |holder: &str| {
        let mut state = original.clone();
        state.revision += 1;
        state.refresh_lease = Some(SubjectRefreshLease {
            holder: holder.into(),
            expires_at: SystemTime::now() + Duration::from_secs(45),
        });
        state
    };
    let (one, two) = tokio::join!(
        first.replace_subject(
            &original.incarnation,
            original.revision,
            claim("first-replica")
        ),
        second.replace_subject(
            &original.incarnation,
            original.revision,
            claim("second-replica")
        ),
    );
    assert_ne!(one.unwrap(), two.unwrap());
    let claimed = first.subject(&original.subject).await.unwrap().unwrap();
    assert_eq!(claimed.revision, original.revision + 1);
    assert_eq!(claimed.token_generation, original.token_generation);
    assert!(claimed.refresh_lease.is_some());
    assert!(!first
        .replace_subject(
            &original.incarnation,
            original.revision,
            claim("stale-replica")
        )
        .await
        .unwrap());

    let mut login = initial();
    login.revision = claimed.revision + 1;
    login.token_generation = claimed.token_generation + 1;
    login.authorization = Authorization::new([layer2_core::Capability::LaunchTriggers]);
    login.profile.display_name = Some("New Login".into());
    assert!(second
        .replace_subject(&claimed.incarnation, claimed.revision, login.clone())
        .await
        .unwrap());
    let mut stale = claimed.clone();
    stale.revision = login.revision + 1;
    stale.tokens = None;
    stale.refresh_lease = None;
    assert!(!first
        .replace_subject(&claimed.incarnation, login.revision, stale)
        .await
        .unwrap());
    let retained = first.subject(&original.subject).await.unwrap().unwrap();
    assert_eq!(retained.incarnation, login.incarnation);
    assert_eq!(retained.tokens, login.tokens);
    assert_eq!(retained.authorization, login.authorization);
    assert_eq!(retained.profile, login.profile);

    let mut absent = initial();
    absent.subject = SubjectId::new("not-created");
    absent.revision = 2;
    assert!(!first
        .replace_subject(&original.incarnation, 1, absent.clone())
        .await
        .unwrap());
    assert!(first.subject(&absent.subject).await.unwrap().is_none());
    let mut exhausted = login.clone();
    exhausted.revision = i64::MAX as u64 + 1;
    assert!(matches!(
        first
            .replace_subject(&login.incarnation, i64::MAX as u64, exhausted)
            .await,
        Err(StoreError::Malformed(_))
    ));
    client.database(&name).drop().await.unwrap();
}

fn unfenced(store: &MongoAuthStore) -> Document {
    let mut document =
        mongodb::bson::serialize_to_document(&store.subject_to_doc(initial()).unwrap()).unwrap();
    for field in [
        "incarnation",
        "revision",
        "token_generation",
        "refresh_lease",
    ] {
        document.remove(field);
    }
    document
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_unfenced_login_replacement_has_one_winner_and_never_overwrites_fenced_state() {
    let (client, name, first, second) = stores().await;
    let raw = client.database(&name).collection::<Document>(SUBJECTS);
    raw.insert_one(unfenced(&first)).await.unwrap();
    let owner = SubjectId::new("refresh-owner");
    assert!(first.subject(&owner).await.is_err());
    let one = initial();
    let two = initial();
    let (one_won, two_won) = tokio::join!(
        first.replace_unfenced_subject(one.clone()),
        second.replace_unfenced_subject(two.clone())
    );
    let one_won = one_won.unwrap();
    assert_ne!(one_won, two_won.unwrap());
    let winner = if one_won { one } else { two };
    assert!(!first.replace_unfenced_subject(initial()).await.unwrap());
    assert_eq!(
        first.subject(&owner).await.unwrap().unwrap().incarnation,
        winner.incarnation
    );
    let mut absent = initial();
    absent.subject = SubjectId::new("not-created");
    assert!(!first
        .replace_unfenced_subject(absent.clone())
        .await
        .unwrap());
    assert!(first.subject(&absent.subject).await.unwrap().is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_partial_malformed_and_wrong_schema_fences_are_not_login_legacy() {
    let (client, name, first, _) = stores().await;
    let raw = client.database(&name).collection::<Document>(SUBJECTS);
    for (index, (field, value)) in [
        ("incarnation", Bson::String("existing".into())),
        ("revision", Bson::Int64(1)),
        ("token_generation", Bson::Int64(1)),
        ("refresh_lease", Bson::Null),
        (
            "document_schema_version",
            Bson::Int64(DOCUMENT_SCHEMA_VERSION + 1),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut document = unfenced(&first);
        let owner = SubjectId::new(format!("partial-{index}"));
        document.insert("_id", owner.as_str());
        document.insert(field, value);
        raw.insert_one(document).await.unwrap();
        assert!(first.subject(&owner).await.is_err());
        let mut login = initial();
        login.subject = owner;
        assert!(!first.replace_unfenced_subject(login).await.unwrap());
    }
    let mut malformed =
        mongodb::bson::serialize_to_document(&first.subject_to_doc(initial()).unwrap()).unwrap();
    malformed.insert("revision", "not-a-counter");
    raw.insert_one(malformed).await.unwrap();
    assert!(first
        .subject(&SubjectId::new("refresh-owner"))
        .await
        .is_err());
    assert!(!first.replace_unfenced_subject(initial()).await.unwrap());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
async fn all_fence_fields_are_required_and_refresh_lease_round_trips() {
    let store = super::tests::store().await;
    let mut state = initial();
    state.refresh_lease = Some(SubjectRefreshLease {
        holder: "fixture-holder".into(),
        expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
    });
    let document = store.subject_to_doc(state.clone()).unwrap();
    let restored = store.subject_from_doc(document.clone()).unwrap();
    assert_eq!(restored.refresh_lease, state.refresh_lease);
    for field in [
        "incarnation",
        "revision",
        "token_generation",
        "refresh_lease",
    ] {
        let mut raw = mongodb::bson::serialize_to_document(&document).unwrap();
        raw.remove(field);
        assert!(
            mongodb::bson::deserialize_from_document::<SubjectDoc>(raw).is_err(),
            "missing {field} must not create a fence"
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn browser_login_repairs_unfenced_subject_without_accepting_stale_cookie_authority() {
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use tower::ServiceExt as _;

    use crate::auth::{attach_subject, AuthMode, AuthState, ProviderMetadata};

    let (client, name, store, _) = stores().await;
    client
        .database(&name)
        .collection::<Document>(SUBJECTS)
        .insert_one(unfenced(&store))
        .await
        .unwrap();
    let owner = SubjectId::new("refresh-owner");
    store
        .put_browser_login(BrowserLogin {
            id: SecretDigest::identify("browser-login", "unfenced-browser"),
            subject: owner.clone(),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route(
                    "/token",
                    post(|| async {
                        Json(json!({
                            "access_token": "new-fixture-access",
                            "refresh_token": "new-fixture-refresh",
                            "expires_in": 3600,
                        }))
                    }),
                )
                .route(
                    "/userinfo",
                    get(|| async {
                        Json(json!({
                            "sub": "refresh-owner", "name": "Recovered Owner",
                            "permissions": ["cma:trigger:launch"],
                        }))
                    }),
                ),
        )
        .await
        .unwrap();
    });
    let auth = AuthState::with_store(
        AuthMode::Provider {
            base_url: base.clone(),
            client_id: "fixture-client".into(),
            client_secret: "fixture-value".into(),
            public_url: "https://cma.example".into(),
        },
        None,
        store.clone(),
    );
    *auth.fixture_metadata().write().await = Some(ProviderMetadata {
        authorization_endpoint: format!("{base}/authorize"),
        token_endpoint: format!("{base}/token"),
        userinfo_endpoint: Some(format!("{base}/userinfo")),
        introspection_endpoint: None,
    });
    let app = crate::auth::router(auth.clone())
        .layer(axum::middleware::from_fn_with_state(auth, attach_subject));
    let request = |path: &str, binding: Option<&str>| {
        let mut cookies = "cma_session=unfenced-browser".to_owned();
        if let Some(binding) = binding {
            cookies.push_str("; ");
            cookies.push_str(binding);
        }
        Request::get(path)
            .header(header::COOKIE, cookies)
            .body(Body::empty())
            .unwrap()
    };
    let unavailable = app
        .clone()
        .oneshot(request("/auth/session", None))
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    let login = app
        .clone()
        .oneshot(request("/auth/login?return_to=/t/pub_recovery", None))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let authorization =
        reqwest::Url::parse(login.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let state = authorization
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
        .unwrap();
    let binding = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let callback = format!("/auth/callback?state={state}&code=fixture-code");
    let unbound = app.clone().oneshot(request(&callback, None)).await.unwrap();
    assert_eq!(unbound.status(), StatusCode::BAD_REQUEST);
    assert!(store.subject(&owner).await.is_err());
    let completed = app
        .clone()
        .oneshot(request(&callback, Some(binding)))
        .await
        .unwrap();
    assert_eq!(completed.status(), StatusCode::SEE_OTHER);
    assert_eq!(completed.headers()[header::LOCATION], "/t/pub_recovery");
    let recovered = store.subject(&owner).await.unwrap().unwrap();
    recovered.validate().unwrap();
    assert_eq!(
        recovered.profile.display_name.as_deref(),
        Some("Recovered Owner")
    );
    assert_eq!(
        recovered.tokens.unwrap().access_token.expose(),
        "new-fixture-access"
    );
    let replay = app
        .clone()
        .oneshot(request(&callback, Some(binding)))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    let current = app.oneshot(request("/auth/session", None)).await.unwrap();
    assert_eq!(current.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(current.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["subject"], "refresh-owner");
    assert_eq!(body["capabilities"], json!(["launch_triggers"]));
    upstream.abort();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_browser_login_reservation_fences_logout_across_replicas() {
    let (client, name, first, second) = stores().await;
    for order in ["logout-first", "complete-first", "expired", "race"] {
        let id = SecretDigest::identify("browser-login", order);
        let expiry = if order == "expired" {
            SystemTime::UNIX_EPOCH
        } else {
            SystemTime::now() + Duration::from_secs(60)
        };
        first
            .reserve_browser_login(id.clone(), expiry)
            .await
            .unwrap();
        assert!(second.browser_login(&id).await.unwrap().is_none());
        let login = BrowserLogin {
            id: SecretDigest::identify("browser-login", &format!("session-{order}")),
            subject: SubjectId::new("owner-a"),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        };
        match order {
            "logout-first" => {
                second.delete_browser_login(&id).await.unwrap();
                assert!(!first
                    .complete_browser_login(&id, login.clone(), None)
                    .await
                    .unwrap());
            }
            "complete-first" => {
                assert!(first
                    .complete_browser_login(&id, login.clone(), None)
                    .await
                    .unwrap());
                assert_eq!(
                    second
                        .browser_login(&login.id)
                        .await
                        .unwrap()
                        .unwrap()
                        .subject,
                    login.subject
                );
                let wrong = BrowserLogin {
                    subject: SubjectId::new("owner-b"),
                    ..login.clone()
                };
                assert!(!second
                    .complete_browser_login(&id, wrong, None)
                    .await
                    .unwrap());
                second.delete_browser_login(&id).await.unwrap();
            }
            "expired" => {
                assert!(!second
                    .complete_browser_login(&id, login.clone(), None)
                    .await
                    .unwrap());
            }
            "race" => {
                let (issued, removed) = tokio::join!(
                    first.complete_browser_login(&id, login.clone(), None),
                    second.delete_browser_login(&id)
                );
                issued.unwrap();
                removed.unwrap();
            }
            _ => unreachable!(),
        }
        assert!(first.browser_login(&login.id).await.unwrap().is_none());
        assert!(!second
            .complete_browser_login(&id, login, None)
            .await
            .unwrap());
    }
    client.database(&name).drop().await.unwrap();
}
