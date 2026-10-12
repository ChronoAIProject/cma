use cma_image_manager::config::*;
use cma_image_manager::{
    auth::NyxIdAuth,
    cloud::{CloudBuild, Executor},
    http::{self, App},
    schema,
    store::Store,
    worker,
};
use std::sync::Arc;

fn required(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{name} is required").into())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Vec<_> = std::env::args().skip(1).collect();
    let db_name = required(CMA_IM_MONGODB_DATABASE)?;
    if args == ["--print-runtime-role"] {
        println!("{}", schema::runtime_role(&db_name));
        return Ok(());
    }
    if args == ["--print-migration-role"] {
        println!("{}", schema::migration_role(&db_name));
        return Ok(());
    }
    if !args.is_empty() && args != ["--migrate"] {
        return Err(
            "usage: cma-image-manager [--migrate|--print-runtime-role|--print-migration-role]"
                .into(),
        );
    }
    let mongo = mongodb::Client::with_uri_str(required(CMA_IM_MONGODB_URI)?).await?;
    let database = mongo.database(&db_name);
    if args == ["--migrate"] {
        schema::migrate(&database).await?;
        return Ok(());
    }
    let store = Store::open(database.clone()).await?;
    let issuer = required(CMA_IM_NYXID_ISSUER)?;
    let auth = NyxIdAuth::new(
        &issuer,
        &format!(
            "{}/api/v1/proxy/s/cma-image-manager",
            issuer.trim_end_matches('/')
        ),
        &required(CMA_IM_NYXID_CLIENT_ID)?,
        required(CMA_IM_NYXID_CLIENT_SECRET)?,
    )?;
    let browser = Arc::new(cma_service_auth::browser::BrowserAuth::new(
        auth.0.clone(),
        &database,
        schema::SESSIONS,
        &required(CMA_IM_WEB_ORIGIN)?,
        "/api/image-manager",
        "/image-manager",
        &std::fs::read(required(CMA_IM_COOKIE_KEY_FILE)?)?,
    )?);
    let executor: Executor = Arc::new(CloudBuild::new(
        "example-project-02".into(),
        "asia-northeast1".into(),
        required(CMA_IM_BUILD_SERVICE_ACCOUNT)?,
        required(CMA_IM_BUILDER_IMAGE)?,
        std::env::var(CMA_IM_GOOGLE_TOKEN_FILE)
            .ok()
            .filter(|s| !s.is_empty())
            .map(Into::into),
    )?);
    let worker_store = store.clone();
    let worker_executor = executor.clone();
    let worker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if worker::tick(&worker_store, &worker_executor).await.is_err() {
                tracing::warn!("image build worker dependency unavailable");
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(
        std::env::var(CMA_IM_LISTEN).unwrap_or_else(|_| "0.0.0.0:8080".into()),
    )
    .await?;
    axum::serve(
        listener,
        browser.protect(
            http::router(Arc::new(App {
                store,
                auth,
                executor,
            }))
            .merge(browser.routes()),
        ),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    worker.abort();
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = signal.recv()=>{}}
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
