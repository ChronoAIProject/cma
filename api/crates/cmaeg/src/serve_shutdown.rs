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

/// Own shutdown resources together so every entry point uses the same order.
pub(super) struct Drain {
    pub tracker: std::sync::Arc<crate::turns::TurnTracker>,
    pub drain_timeout: std::time::Duration,
    pub native_lark_controls: crate::native_lark::control_recovery::Worker,
    pub whatsapp_form_worker: Option<crate::whatsapp::forms::worker::Worker>,
    pub native_discord: Option<crate::discord::lifecycle::Worker>,
    pub routed_workers: Vec<crate::serve_routed::Worker>,
    pub github_initialization: Option<crate::github::Initialization>,
    pub delivery_engine: crate::delivery::engine::DeliveryEngine,
}
impl Drain {
    pub async fn run(mut self) -> std::time::Instant {
        self.tracker.start_draining();
        let deadline = std::time::Instant::now() + self.drain_timeout;
        self.native_lark_controls.stop();
        if let Some(worker) = &self.whatsapp_form_worker {
            worker.stop();
        }
        if let Some(worker) = &self.native_discord {
            worker.stop();
        }
        for worker in &self.routed_workers {
            worker.stop_scheduling();
        }
        if let Some(worker) = &self.github_initialization {
            worker.stop();
        }
        let (
            turns_drained,
            routed_drained,
            native_drained,
            lark_controls_drained,
            whatsapp_forms_drained,
        ) = tokio::join!(
            self.tracker.drain(deadline),
            async {
                let mut drained = true;
                for worker in &mut self.routed_workers {
                    drained &= worker.drain(deadline).await;
                }
                drained
            },
            async {
                match &mut self.native_discord {
                    Some(worker) => worker.drain(deadline).await,
                    None => true,
                }
            },
            self.native_lark_controls.drain(deadline),
            async {
                match &mut self.whatsapp_form_worker {
                    Some(worker) => worker.drain(deadline).await,
                    None => true,
                }
            }
        );
        log_drain_results(
            turns_drained,
            routed_drained,
            native_drained,
            lark_controls_drained,
            whatsapp_forms_drained,
        );
        drain_github(&mut self.github_initialization, deadline).await;
        if !self.delivery_engine.drain(deadline).await {
            tracing::warn!(
                "delivery drain deadline reached; remaining sends cancelled with unknown outcome"
            );
        }
        deadline
    }
}
