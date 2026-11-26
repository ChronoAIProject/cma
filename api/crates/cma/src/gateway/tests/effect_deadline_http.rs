//! Hold a real read/response, expire the reservation, then resume the adapter.
use super::*;
use axum::{
    extract::State,
    routing::{delete, get, post},
    Json, Router,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

#[derive(Default)]
struct Hold {
    entered: Notify,
    release: Notify,
    mutations: AtomicUsize,
}
impl Hold {
    async fn wait(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
    async fn expire(&self) {
        self.entered.notified().await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(121)).await;
        tokio::time::resume();
        self.release.notify_one();
    }
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, task)
}
fn route(id: &str) -> Value {
    json!({"id":id,"channel_bot_id":"external-bot","platform":"telegram",
        "agent_api_key_id":"route-key","platform_conversation_id":"*",
        "default_agent":true,"is_active":true})
}
async fn mutation(State(hold): State<Arc<Hold>>) -> Json<Value> {
    hold.mutations.fetch_add(1, Ordering::SeqCst);
    Json(json!({}))
}
#[tokio::test]
async fn scope_plan_response_after_expiry_cannot_mint_a_key() {
    let hold = Arc::new(Hold::default());
    let router = Router::new()
        .route(
            "/api/v1/api-keys/scope-plan",
            post(
                |State(hold): State<Arc<Hold>>, Json(body): Json<Value>| async move {
                    hold.wait().await;
                    Json(crate::auth::keys::scope_plan_fixture("owner", &body, &[]))
                },
            ),
        )
        .route("/api/v1/api-keys", post(mutation))
        .with_state(hold.clone());
    let (base, task) = serve(router).await;
    let issuer = KeyIssuer::new(reqwest::Client::new(), base, "deadline");
    let bot = super::effect_deadline::reserved_bot().await;
    let admission = EffectAdmission::from_bot(&bot).unwrap().unwrap();
    let checked = AtomicBool::new(false);
    let owner = bot.resource_owner();
    let request = issuer.mint_bot_admitted(
        "route",
        "",
        &[],
        Some("https://example.invalid/pending"),
        &owner,
        || {
            checked.store(true, Ordering::SeqCst);
            admission.check(&bot).is_ok()
        },
    );
    let (result, ()) = tokio::join!(request, hold.expire());
    assert!(result.is_err());
    assert!(
        checked.load(Ordering::SeqCst),
        "scope-plan response must reach the send fence"
    );
    assert_eq!(hold.mutations.load(Ordering::SeqCst), 0);
    assert!(bot.work.unwrap().effect.is_some());
    task.abort();
    let _ = task.await;
}
#[tokio::test]
async fn route_inventory_after_expiry_cannot_create_a_route() {
    let hold = Arc::new(Hold::default());
    let router = Router::new()
        .route(
            "/api/v1/channel-conversations",
            get(|State(hold): State<Arc<Hold>>| async move {
                hold.wait().await;
                Json(json!({"conversations":[],"total":0}))
            })
            .post(mutation),
        )
        .with_state(hold.clone());
    let (base, task) = serve(router).await;
    let client = nyxid::NyxidClient {
        http: reqwest::Client::new(),
        base,
    };
    let bot = super::effect_deadline::reserved_bot().await;
    let admission = EffectAdmission::from_bot(&bot).unwrap().unwrap();
    let (result, ()) = tokio::join!(
        client.create_route("", &bot, "route-key", Some(&admission)),
        hold.expire()
    );
    assert_eq!(result, Err(BotError::Conflict));
    assert_eq!(hold.mutations.load(Ordering::SeqCst), 0);
    assert!(bot.work.unwrap().effect.is_some());
    task.abort();
    let _ = task.await;
}
#[tokio::test]
async fn expiry_between_deletes_retains_custody_and_refuses_the_next_send() {
    let hold = Arc::new(Hold::default());
    let router = Router::new()
        .route(
            "/api/v1/channel-conversations",
            get(|| async { Json(json!({"conversations":[route("one"),route("two")],"total":2})) }),
        )
        .route(
            "/api/v1/channel-conversations/{id}",
            delete(|State(hold): State<Arc<Hold>>| async move {
                hold.mutations.fetch_add(1, Ordering::SeqCst);
                hold.wait().await;
                axum::http::StatusCode::NO_CONTENT
            }),
        )
        .with_state(hold.clone());
    let (base, task) = serve(router).await;
    let client = nyxid::NyxidClient {
        http: reqwest::Client::new(),
        base,
    };
    let bot = super::effect_deadline::reserved_bot().await;
    let admission = EffectAdmission::from_bot(&bot).unwrap().unwrap();
    let keys = ["route-key".to_string()];
    let (result, ()) = tokio::join!(
        client.delete_routes("", &bot, &keys, Some(&admission)),
        hold.expire()
    );
    assert_eq!(result, Err(BotError::Conflict));
    assert_eq!(hold.mutations.load(Ordering::SeqCst), 1);
    assert!(bot.work.unwrap().effect.is_some());
    task.abort();
    let _ = task.await;
}
