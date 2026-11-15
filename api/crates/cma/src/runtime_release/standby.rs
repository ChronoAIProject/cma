use super::{bootstrap::VerifiedBootstrap, selection, Session};
use crate::{config::Config, release_authority::AuthorityStore};
use std::{future::IntoFuture, sync::Arc, time::Duration};

pub(super) async fn wait_for_writer(
    config: &Config,
    store: Arc<AuthorityStore>,
    bootstrap: VerifiedBootstrap,
) -> Result<Arc<Session>, String> {
    if bootstrap.expires_at_unix() <= super::files::now()? {
        return Err("inert bootstrap expired before listener admission".into());
    }
    let router = super::readonly::compose(config).await?;
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|_| "cannot bind inert read listener")?;
    let (stop, signal) = tokio::sync::watch::channel(false);
    let server = axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(crate::shutdown::signalled(signal))
        .into_future();
    tokio::pin!(server);
    let selected = async {
        loop {
            if let Some(head) = store
                .load(&config.deployment_id)
                .await
                .map_err(|error| error.to_string())?
            {
                if let Ok(writer) = selection::selected_writer(&head, bootstrap.process()) {
                    super::pools::verify_selected(config, &head).await?;
                    let session = Arc::new(Session {
                        store: store.clone(),
                        writer,
                    });
                    // Composition, detached recovery and its process lifetime
                    // remain unresolved until a trusted collector proves finality.
                    use layer3_adapter::effect_authority::{EffectAuthority, EffectIntent};
                    let intent = EffectIntent::new(
                        "process_lifetime",
                        &config.deployment_id,
                        &(bootstrap.process(), bootstrap.sha256()),
                    )
                    .map_err(|error| error.to_string())?;
                    session
                        .admit(&intent)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Ok(session);
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    let outcome = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(900), selected) =>
            result.map_err(|_| "managed writer selection exceeded its bound".to_owned()).and_then(|value| value),
        result = &mut server => return Err(format!("inert read listener ended: {result:?}")),
        () = crate::shutdown::shutdown_signal() => Err("inert process was stopped".into()),
    };
    let _ = stop.send(true);
    tokio::time::timeout(Duration::from_secs(30), &mut server)
        .await
        .map_err(|_| "inert read listener did not drain")?
        .map_err(|_| "inert read listener failed")?;
    outcome
}
