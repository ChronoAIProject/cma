//! Install process signal handlers before the first asynchronous startup step.
use tokio::sync::watch;

#[derive(Clone)]
pub(crate) struct Termination(watch::Receiver<bool>);

impl Termination {
    pub(crate) fn install() -> Result<Self, String> {
        #[cfg(unix)]
        let source = {
            use tokio::signal::unix::{signal, SignalKind};
            // Stream construction arms handlers synchronously, before startup.
            let mut terminate = signal(SignalKind::terminate())
                .map_err(|_| "cannot install termination handler")?;
            let mut interrupt =
                signal(SignalKind::interrupt()).map_err(|_| "cannot install interrupt handler")?;
            async move {
                tokio::select! {
                    _ = terminate.recv() => (),
                    _ = interrupt.recv() => (),
                }
            }
        };
        #[not(unix)]
        let source = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        Ok(Self::observe(source))
    }

    pub(crate) fn observe(source: impl std::future::Future<Output = ()> + Send + 'static) -> Self {
        let (signal, state) = watch::channel(false);
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = signal.closed() => (),
                _ = source => { signal.send_replace(true); },
            }
        });
        Self(state)
    }

    pub(crate) fn requested(&self) -> bool {
        *self.0.borrow()
    }

    pub(crate) async fn cancelled(&self) {
        super::signalled(self.0.clone()).await;
    }

    pub(crate) async fn startup<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        tokio::select! {
            biased;
            _ = self.cancelled() => Err("termination requested during startup".into()),
            result = work => result,
        }
    }
}

#[cfg(test)]
mod tests;
