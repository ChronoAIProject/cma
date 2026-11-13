pub(super) async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        match terminate {
            Ok(mut signal) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result { tracing::warn!(%error, "ctrl-c signal listener stopped"); }
                    }
                    _ = signal.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "SIGTERM listener unavailable; waiting for ctrl-c");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

pub(super) fn log_drain_results(
    turns_drained: bool,
    routed_drained: bool,
    native_drained: bool,
    lark_controls_drained: bool,
    whatsapp_forms_drained: bool,
) {
    if !whatsapp_forms_drained {
        tracing::warn!("native WhatsApp Form drain incomplete; durable write fences retained");
    }
    if !lark_controls_drained {
        tracing::warn!("native Lark callback drain incomplete; consumed cards retain recovery");
    }
    if !native_drained {
        tracing::warn!("native Discord drain incomplete; durable inbox and lease fences retained");
    }
    if !turns_drained {
        tracing::warn!("turn drain deadline reached; remaining consumers cancelled");
    }
    if !routed_drained {
        tracing::warn!("routed execution drain incomplete; durable recovery retained");
    }
}

pub(super) async fn drain_github(
    worker: &mut Option<crate::github::Initialization>,
    deadline: std::time::Instant,
) {
    if let Some(worker) = worker {
        if !worker.drain(deadline).await {
            tracing::warn!("GitHub storage initialization did not stop before shutdown");
        }
    }
}
