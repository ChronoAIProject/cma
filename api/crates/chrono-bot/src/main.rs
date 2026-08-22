//! chrono-bot control plane. Wires Layer 1 interfaces onto the Layer 2 core,
//! which drives kernels through Layer 3 adapters.
//!
//! This process never executes agent code. It serves public traffic, keeps one
//! rendered screen per live session, and relays terminals to and from sandboxed
//! runtimes that run elsewhere.

mod activity;
mod auth;
mod config;
mod kernels;
mod permissions;
mod renewal;
mod screen;
mod skills;
mod store;
mod sweep;

use std::sync::Arc;

use axum::{routing::get, Json, Router};
use layer1_public::{
    a2a::A2aInterface, agui::AguiInterface, mcp::McpInterface, pty::PtyInterface,
    rest::RestInterface, InterfaceProfile,
};
use layer2_core::{
    ActivityService, Baseline, EnvironmentProfileService, PlatformSettingsService, ScreenModel,
    SessionService, SkillId, SoftwarePackage, TerminalGeometry, UserDirectory,
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::activity::ActivityRecorder;
use crate::auth::{AuthState, SignInRecords};
use crate::config::Config;
use crate::screen::VtScreen;

fn interfaces() -> Vec<Box<dyn InterfaceProfile>> {
    vec![
        Box::new(AguiInterface),
        Box::new(RestInterface),
        Box::new(PtyInterface),
        Box::new(McpInterface),
        Box::new(A2aInterface),
    ]
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            // Configuration failures are fatal and must say exactly what is
            // missing; a control plane that starts half-configured is worse
            // than one that refuses to start.
            tracing::error!(error = %e, "configuration is not usable");
            std::process::exit(2);
        }
    };
    tracing::info!(
        bind = %config.bind,
        public_url = %config.public_url,
        sandbox = %config.sandbox_base_url,
        sandbox_image = %config.sandbox_image,
        auth = config.auth_mode_name(),
        "configuration loaded"
    );

    for iface in interfaces() {
        tracing::info!(
            interface = iface.interface_id(),
            operations = iface.operations().len(),
            "interface projected"
        );
    }

    let adapters = crate::kernels::registry(&config);

    let screen_factory: layer2_core::ScreenFactory = Arc::new(|geometry: TerminalGeometry| {
        Box::new(VtScreen::new(geometry)) as Box<dyn ScreenModel>
    });
    // Sessions outlive this process, so the stores are opened before anything
    // is served: starting without them would accept work we cannot keep, and a
    // control plane that opened one and not the other would serve half its
    // surface with the failure found by a user rather than at startup.
    let stores = match crate::store::connect(
        &config.mongodb_uri,
        &config.mongodb_database,
        config.activity_retention,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "cannot open the stores");
            std::process::exit(2);
        }
    };
    let store = Arc::new(stores.sessions) as Arc<dyn layer2_core::SessionStore>;
    let profile_store = Arc::new(stores.profiles) as Arc<dyn layer2_core::EnvironmentProfileStore>;
    let settings_store = Arc::new(stores.settings) as Arc<dyn layer2_core::PlatformSettingsStore>;
    // The account of what this service is asked to do, and of who asks. Both
    // are opened with everything else rather than lazily: a control plane that
    // served traffic it could not account for would be discovered by somebody
    // looking for an entry that was never written.
    let activity = ActivityService::new(Arc::new(stores.activity));
    let users = UserDirectory::new(Arc::new(stores.users));
    tracing::info!(
        retention_days = config.activity_retention.as_secs() / 86_400,
        "every request is recorded, and entries older than this are removed"
    );

    // Context storage is optional: without it a session still works, it simply
    // cannot carry its transcript into a new sandbox.
    //
    // It needs both an address and an identity, and switches off unless both
    // are present. Half a configuration would build a store that authenticates
    // as nobody and fails on first write, long after startup and precisely
    // when a transcript was being kept.
    let identity = crate::store::token::from_config(
        &config.platform_base_url,
        &config.storage_client_id,
        &config.storage_client_secret,
    );
    // The catalogue shares this identity rather than minting a second one:
    // both act as this service against the same platform, and two machine
    // identities for one process is two things to rotate.
    let identity_for_catalog = identity.clone();
    // The bucket is as load-bearing as the identity: this service writes only
    // into the one it is given and creates none, so an unnamed bucket is not a
    // default to fall back on.
    let context: Option<Arc<dyn layer2_core::ContextStore>> =
        match identity.filter(|_| !config.context_bucket.trim().is_empty()) {
            Some(tokens) => Some(Arc::new(crate::store::ChronoStorageContextStore::new(
                format!(
                    "{}/api/v1/proxy/s/chrono-storage-service",
                    config.platform_base_url.trim_end_matches('/')
                ),
                config.context_bucket.clone(),
                tokens,
            ))),
            None => {
                tracing::warn!(
                    "session context will not be stored. CHRONO_PLATFORM_BASE_URL, \
                 CHRONO_STORAGE_CLIENT_ID, CHRONO_STORAGE_CLIENT_SECRET and \
                 CHRONO_CONTEXT_BUCKET are all required. The bucket must already \
                 exist and be owned by this identity — nothing here creates one. \
                 No other service's credential is valid here."
                );
                None
            }
        };

    // Built before the service because the service asks it for an owner's
    // credential whenever it has to put a runtime back under a session that
    // nobody is currently watching.
    let auth_state = AuthState::new(
        config.auth_mode.clone(),
        Some(SignInRecords {
            users: Arc::clone(&users),
            activity: Arc::clone(&activity),
        }),
    );
    if !auth_state.requires_credential() {
        tracing::warn!(
            "running with a trusted fixed subject; every request is attributed \
             to the same principal. This must never be used outside local development."
        );
    }

    // What every session carries whether or not its owner asked for it. Read
    // from configuration so the list is a deployment fact, while "these cannot
    // be removed" stays a rule of the core.
    let baseline = Baseline::new(
        config.baseline_packages.iter().map(SoftwarePackage::new),
        config.baseline_skills.iter().map(SkillId::new),
        config
            .baseline_services
            .iter()
            .map(layer2_core::ServiceId::new),
    );
    tracing::info!(
        packages = ?config.baseline_packages,
        skills = ?config.baseline_skills,
        "every session carries these, and no profile can remove them"
    );

    // The registry a caller picks skills from. It shares this service's machine
    // identity, and prefers the caller's own credential per request so the
    // answer is what they can see rather than only what is public.
    // Built here rather than after the service, because both the picker that
    // shows what may be chosen and the mint that turns a choice into reach ask
    // the same platform the same question.
    let issuer = (!config.platform_base_url.trim().is_empty()).then(|| {
        Arc::new(auth::keys::KeyIssuer::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("http client builds"),
            config.platform_base_url.clone(),
            config.deployment_id.clone(),
        ))
    });
    let service_picker = issuer.clone().map(|i| {
        Arc::new(auth::keys::ServicePicker::new(i)) as Arc<dyn layer2_core::ServiceCatalog>
    });

    let catalog = crate::skills::from_config(&config.platform_base_url, identity_for_catalog);
    if catalog.is_none() {
        tracing::warn!(
            "no skill registry is configured, so the profile editor will offer \
             nothing to choose from. CHRONO_PLATFORM_BASE_URL, \
             CHRONO_STORAGE_CLIENT_ID and CHRONO_STORAGE_CLIENT_SECRET are all \
             required. Profiles still store and apply skills chosen by other means."
        );
    }

    // One persisted service is shared by every consumer: profile validation,
    // session provisioning and the administrator surface must never observe
    // different revisions of deployment policy by construction.
    let settings = Arc::new(PlatformSettingsService::new(settings_store));
    let environments = EnvironmentProfileService::new(
        profile_store,
        baseline,
        catalog
            .clone()
            .map(|c| c as Arc<dyn layer2_core::SkillCatalog>),
        service_picker,
        config.default_agent.clone(),
        Arc::clone(&settings),
    );
    let service = SessionService::new(
        adapters,
        screen_factory,
        store,
        layer2_core::SessionPorts {
            context,
            credentials: Some(Arc::clone(&auth_state) as Arc<dyn layer2_core::CredentialSource>),
            // The same registry the profile editor picks from, asked for
            // contents rather than for names. A deployment without one still
            // starts sessions; they carry whatever their image does.
            skills: catalog
                .clone()
                .map(|c| c as Arc<dyn layer2_core::SkillSource>),
            settings: Some(Arc::clone(&settings)),
        },
        Arc::clone(&environments),
        layer2_core::Deployment {
            // The runtime image reference is the runtime version. Deriving it
            // rather than configuring it separately means the two cannot drift:
            // any deployment that changes the image is one whose sessions are
            // carried onto it, and one that does not is not.
            runtime_version: Some(config.sandbox_image.clone()),
            identity: Some(config.deployment_id.clone()),
            adopts_untagged: config.adopts_untagged_runtimes,
        },
    );
    tracing::info!(
        runtime_version = %config.sandbox_image,
        deployment = %config.deployment_id,
        adopts_untagged = config.adopts_untagged_runtimes,
        "sessions on any other runtime will be moved onto this one"
    );

    // Keeps delegated credentials valid underneath running sessions, and pushes
    // each renewal into the runtimes already using the old value.
    // Per-session credentials are minted on the platform, so this only exists
    // where a platform root is configured. Without one every session falls
    // back to its owner's own token, which is what happened before scoping.
    if let Some(issuer) = issuer {
        auth_state.use_issuer(issuer).await;
        tracing::info!(
            deployment = %config.deployment_id,
            services = ?config.baseline_services,
            "each session gets its own credential, reaching these services and whatever its profile adds"
        );
    }

    renewal::spawn(Arc::clone(&auth_state), Arc::clone(&service));

    // Sessions end only when their owner says so, which leaves this as the one
    // thing that reclaims a runtime nothing refers to or finishes a session
    // whose start-up died with an earlier process.
    sweep::spawn(Arc::clone(&service));

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(config.cors_origins.clone()))
        .allow_credentials(true)
        .allow_headers([axum::http::header::CONTENT_TYPE])
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PATCH,
            axum::http::Method::DELETE,
        ]);

    // What somebody running the service can see and do. Its own router under
    // its own path segment so the whole surface can be restricted in front of
    // this process as well as inside it.
    // Minted where a credential can be carried and redeemed where one cannot,
    // so both surfaces are handed the same store.
    let tickets = layer1_public::tickets::TerminalTickets::new();

    let operator =
        layer1_public::rest::admin::router(Arc::new(layer1_public::rest::admin::AdminState {
            sessions: Arc::clone(&service),
            activity: Arc::clone(&activity),
            users: Arc::clone(&users),
            settings: Arc::clone(&settings),
        }));

    let app = Router::new()
        .route("/livez", get(|| async { "ok" }))
        .route("/ready", get(ready))
        .merge(layer1_public::rest::router(
            layer1_public::rest::SessionsState {
                sessions: Arc::clone(&service),
                users: Arc::clone(&users),
                tickets: Arc::clone(&tickets),
                environments: Arc::clone(&environments),
            },
        ))
        .merge(layer1_public::rest::environments::router(Arc::clone(
            &environments,
        )))
        // The priority-1 interface, sharing the facade's state because it
        // projects the same sessions: a thread here is a session there, and
        // two states would be two answers to the same question.
        .merge(layer1_public::agui::router(
            layer1_public::rest::SessionsState {
                sessions: Arc::clone(&service),
                users: Arc::clone(&users),
                tickets: Arc::clone(&tickets),
                environments: Arc::clone(&environments),
            },
        ))
        .merge(operator)
        .merge(layer1_public::pty::router(
            layer1_public::pty::TerminalState {
                sessions: Arc::clone(&service),
                tickets: Arc::clone(&tickets),
            },
        ))
        .merge(auth::router(Arc::clone(&auth_state)))
        // Ordered deliberately. A layer added later wraps the ones before it,
        // so identity is established first and recording sees both who called
        // and how their request came out. Reversing these two would record
        // every request as having no actor.
        .layer(axum::middleware::from_fn_with_state(
            ActivityRecorder::new(Arc::clone(&activity)),
            activity::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&auth_state),
            auth::attach_subject,
        ))
        .layer(cors);

    let listener = match tokio::net::TcpListener::bind(&config.bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(bind = %config.bind, error = %e, "cannot bind");
            std::process::exit(2);
        }
    };
    tracing::info!(bind = %config.bind, "control plane listening");

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!(error = %e, "server stopped with an error");
        std::process::exit(1);
    }
}

async fn ready() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ready" }))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("ctrl-c handler installs");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler installs")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received; draining");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_interface_has_a_unique_id() {
        let ids: Vec<_> = interfaces().iter().map(|i| i.interface_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "interface ids must be unique");
    }

    #[test]
    fn every_interface_projects_at_least_one_operation() {
        for iface in interfaces() {
            assert!(
                !iface.operations().is_empty(),
                "{} projects no operations",
                iface.interface_id()
            );
        }
    }
}
