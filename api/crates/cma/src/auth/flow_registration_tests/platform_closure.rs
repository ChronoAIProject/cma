use super::*;
use layer2_core::budget::{
    BudgetError, Closure, ClosureReason, PlatformAdmissionState, PlatformSpendLedger,
    PlatformSpendState,
};

const SUBJECT: &str = "unseen-cma-identity";
const CLOSED_DETAIL: &str =
    "New sign-ups are temporarily closed. Existing accounts are unaffected.";

struct FixedSpend(Result<PlatformAdmissionState, BudgetError>);

#[async_trait::async_trait]
impl PlatformSpendLedger for FixedSpend {
    async fn admission_state(&self, _: u64) -> Result<PlatformAdmissionState, BudgetError> {
        self.0.clone()
    }

    async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
        Err(BudgetError::Unavailable)
    }

    async fn record_cloud_observation(
        &self,
        _: layer2_core::budget::CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, layer2_core::budget::ObservationError> {
        Err(layer2_core::budget::ObservationError::Unavailable)
    }
}

fn closed(reason: ClosureReason) -> Arc<FixedSpend> {
    Arc::new(FixedSpend(Ok(PlatformAdmissionState {
        configured: true,
        operator_control: layer2_core::budget::OperatorAdmissionState::Open,
        closures: vec![Closure {
            reason,
            since: 1,
            settings_revision: 1,
            committed_at_entry: 0,
        }],
    })))
}

fn operator_stopped() -> Arc<FixedSpend> {
    Arc::new(FixedSpend(Ok(PlatformAdmissionState {
        operator_control: layer2_core::budget::OperatorAdmissionState::Stopped(
            layer2_core::admission::OperatorStop {
                since: 1,
                revision: 1,
                actor: SubjectId::new("operator"),
                reason: "contain".into(),
            },
        ),
        ..Default::default()
    })))
}

#[tokio::test]
async fn operator_stop_with_metering_off_refuses_new_subjects_and_existing_owners_sign_in() {
    let new = harness(operator_stopped(), vec![]).await;
    let (_, refused) = sign_in(&new.app).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(refused).await["code"], "registration_closed");
    assert!(new
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .is_none());
    let existing = harness(operator_stopped(), vec![SubjectId::new(SUBJECT)]).await;
    let (_, accepted) = sign_in(&existing.app).await;
    assert_eq!(accepted.status(), StatusCode::SEE_OTHER);
    assert!(existing
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn unreadable_operator_control_refuses_new_subjects_but_existing_owners_sign_in() {
    let unreadable = || {
        Arc::new(FixedSpend(Ok(PlatformAdmissionState {
            operator_control: layer2_core::budget::OperatorAdmissionState::Unreadable,
            ..Default::default()
        })))
    };
    let new = harness(unreadable(), vec![]).await;
    let (_, refused) = sign_in(&new.app).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(refused).await["code"], "registration_closed");
    assert!(new.directory.observed.lock().await.is_empty());
    let existing = harness(unreadable(), vec![SubjectId::new(SUBJECT)]).await;
    let (_, accepted) = sign_in(&existing.app).await;
    assert_eq!(accepted.status(), StatusCode::SEE_OTHER);
}

#[derive(Default)]
struct Directory {
    known: Vec<SubjectId>,
    observed: Mutex<Vec<PublicIdentity>>,
}

#[async_trait::async_trait]
impl UserDirectoryStore for Directory {
    async fn observe(&self, identity: &PublicIdentity) -> Result<(), StoreError> {
        self.observed.lock().await.push(identity.clone());
        Ok(())
    }

    async fn user(&self, subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        Ok(self.known.contains(subject).then(|| RegisteredUser {
            identity: PublicIdentity {
                subject: subject.clone(),
                display_name: None,
                avatar_url: None,
            },
            first_seen_at: "2026-09-05T00:00:00Z".into(),
            last_seen_at: "2026-09-05T00:00:00Z".into(),
        }))
    }

    async fn list(&self, _: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        panic!("sign-in does not list the user directory")
    }
}

#[async_trait::async_trait]
impl ActivityStore for Directory {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        panic!("sign-in does not list activity")
    }
}

struct Harness {
    app: Router,
    store: Arc<MemoryAuthStore>,
    directory: Arc<Directory>,
    _provider: SignInProvider,
}

async fn harness(spend: Arc<FixedSpend>, known: Vec<SubjectId>) -> Harness {
    let upstream = Router::new()
        .route(
            "/token",
            post(|| async { Json(json!({"access_token": "closure-access", "expires_in": 3600})) }),
        )
        .route(
            "/userinfo",
            get(|| async { Json(json!({"sub": SUBJECT, "permissions": ["cma:trigger:launch"]})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let provider = SignInProvider(tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    }));
    let store = Arc::new(MemoryAuthStore::default());
    let directory = Arc::new(Directory {
        known,
        ..Directory::default()
    });
    let auth = AuthState::with_store(
        AuthMode::Provider {
            base_url: base_url.clone(),
            client_id: "closure-client".into(),
            client_secret: "closure-value".into(),
            public_url: "https://cma.example.test".into(),
        },
        Some(SignInRecords {
            users: UserDirectory::new(directory.clone()),
            activity: ActivityService::new(directory.clone()),
        }),
        store.clone(),
    )
    .with_platform_spend(spend);
    *auth.metadata.write().await = Some(ProviderMetadata {
        authorization_endpoint: format!("{base_url}/authorize"),
        token_endpoint: format!("{base_url}/token"),
        userinfo_endpoint: Some(format!("{base_url}/userinfo")),
        introspection_endpoint: None,
    });
    let app = router(auth.clone()).layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::attach_subject,
    ));
    Harness {
        app,
        store,
        directory,
        _provider: provider,
    }
}

/// Plain sign-in: the entry is never checked, the callback decides.
async fn sign_in(app: &Router) -> (String, Response) {
    let login = app
        .clone()
        .oneshot(Request::get("/auth/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let authorize =
        reqwest::Url::parse(login.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let query: HashMap<_, _> = authorize.query_pairs().into_owned().collect();
    let binding = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let callback = Request::get(format!(
        "/auth/callback?code=closure-code&state={}",
        urlencode(&query["state"])
    ))
    .header(header::COOKIE, binding)
    .body(Body::empty())
    .unwrap();
    let state = query["state"].clone();
    (state, app.clone().oneshot(callback).await.unwrap())
}

async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}

#[tokio::test]
async fn callback_refuses_a_never_seen_subject_with_registration_closed_and_persists_nothing() {
    for reason in [ClosureReason::SpendWarning, ClosureReason::SpendStop] {
        let h = harness(closed(reason), vec![]).await;
        let (state, response) = sign_in(&h.app).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect();
        assert_eq!(cookies.len(), 1, "only the cleared browser binding");
        assert!(cookies[0].starts_with(&format!("{}=;", binding_cookie_name(&state))));
        assert!(cookies[0].contains("Max-Age=0"));
        let body = json_body(response).await;
        assert_eq!(body["code"], "registration_closed");
        assert_eq!(body["detail"], CLOSED_DETAIL);
        assert!(body.get("login_url").is_none());
        let subject = SubjectId::new(SUBJECT);
        assert!(h.store.subject(&subject).await.unwrap().is_none());
        assert!(h.directory.observed.lock().await.is_empty());
    }
}

#[tokio::test]
async fn callback_admits_an_existing_subject_under_spend_warning() {
    for reason in [ClosureReason::SpendWarning, ClosureReason::SpendStop] {
        let h = harness(closed(reason), vec![SubjectId::new(SUBJECT)]).await;
        let (_, response) = sign_in(&h.app).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(h
            .store
            .subject(&SubjectId::new(SUBJECT))
            .await
            .unwrap()
            .is_some());
    }
}

async fn registration_auth(spend: Arc<FixedSpend>) -> Arc<AuthState> {
    let auth = AuthState::with_store_and_nyxid_public_url(
        AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client-a".into(),
            client_secret: "secret".into(),
            public_url: "https://cma.example".into(),
        },
        None,
        Arc::new(MemoryAuthStore::default()),
        Some("https://nyx.example".into()),
    )
    .with_platform_spend(spend);
    *auth.metadata.write().await = Some(ProviderMetadata {
        authorization_endpoint: "https://nyx-api.example/oauth/authorize".into(),
        token_endpoint: "https://nyx-api.example/oauth/token".into(),
        userinfo_endpoint: None,
        introspection_endpoint: None,
    });
    auth
}

#[tokio::test]
async fn register_screen_is_refused_while_a_closure_exists_and_plain_login_is_not() {
    let auth = registration_auth(closed(ClosureReason::SpendWarning)).await;
    let refused = begin_login(auth.clone(), None, Some(LoginScreen::Register), false).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        refused.headers().get(header::SET_COOKIE).is_none(),
        "no login attempt or browser binding is created"
    );
    let body = json_body(refused).await;
    assert_eq!(body["code"], "registration_closed");
    assert!(body.get("login_url").is_none());
    let plain = begin_login(auth, None, None, false).await;
    assert_eq!(plain.status(), StatusCode::SEE_OTHER);
    assert!(plain.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .starts_with("https://nyx-api.example/oauth/authorize"));
}

#[tokio::test]
async fn register_screen_proceeds_when_the_admission_state_is_unreadable() {
    let auth = registration_auth(Arc::new(FixedSpend(Err(BudgetError::Unavailable)))).await;
    let response = begin_login(auth, None, Some(LoginScreen::Register), false).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .starts_with("https://nyx.example/register"));
}

#[tokio::test]
async fn an_unreadable_admission_state_admits_existing_owners_and_refuses_new_subjects() {
    let unreadable = || Arc::new(FixedSpend(Err(BudgetError::Unavailable)));
    let existing = harness(unreadable(), vec![SubjectId::new(SUBJECT)]).await;
    let (_, response) = sign_in(&existing.app).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(existing
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .is_some());
    let unseen = harness(unreadable(), vec![]).await;
    let (_, response) = sign_in(&unseen.app).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(unseen
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .is_none());
    assert!(unseen.directory.observed.lock().await.is_empty());
}
