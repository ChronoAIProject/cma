//! Independent local asset and callback recovery preserves every durable fence.
use crate::{http::readiness::Readiness, ingress::IngressState};
use futures_util::FutureExt;
use std::{
    panic::AssertUnwindSafe,
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinHandle};
pub(crate) struct Worker {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}
impl Worker {
    pub(crate) fn start(
        state: IngressState,
        provisioner: super::provision::Provisioner,
        readiness: Readiness,
    ) -> Self {
        let (stop, stopped) = watch::channel(false);
        let signal = stopped.clone();
        let task = tokio::spawn(async move {
            let result = AssertUnwindSafe(async {
                tokio::join!(
                    recover(state, stopped.clone()),
                    provision(provisioner, stopped)
                );
            })
            .catch_unwind()
            .await;
            if result.is_err() || !*signal.borrow() {
                readiness.routed_recovery_failed();
                tracing::error!("native WhatsApp Form recovery stopped unexpectedly");
            }
        });
        Self {
            stop,
            task: Some(task),
        }
    }
    pub(crate) fn stop(&self) {
        let _ = self.stop.send(true);
    }
    pub(crate) async fn drain(&mut self, deadline: Instant) -> bool {
        self.stop();
        let Some(mut task) = self.task.take() else {
            return true;
        };
        match tokio::time::timeout_at(deadline.into(), &mut task).await {
            Ok(Ok(())) => true,
            Ok(Err(_)) => false,
            Err(_) => {
                task.abort();
                let _ = task.await;
                false
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
async fn recover(state: IngressState, mut stopped: watch::Receiver<bool>) {
    let Some(cards) = &state.interactions else {
        return;
    };
    let mut after = None;
    loop {
        if *stopped.borrow() {
            return;
        }
        let page = tokio::time::timeout(
            Duration::from_secs(10),
            cards.whatsapp_flow_recovery_page(after.as_deref()),
        )
        .await;
        let delay = match page {
            Ok(Ok(page)) => {
                for (id, channel) in page.cards {
                    if *stopped.borrow() {
                        return;
                    }
                    let Ok(_permit) = state.inflight.clone().try_acquire_owned() else {
                        break;
                    };
                    // Failed admission remains in the encrypted card for the next scan.
                    let _ = tokio::time::timeout(Duration::from_millis(2400), async {
                        cards.recover_whatsapp_flow_binding(&id).await?;
                        let _ = state.resume_whatsapp_flow(&id, &channel).await;
                        Ok::<(), crate::interactions::Error>(())
                    })
                    .await;
                }
                after = page.next;
                if after.is_some() {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(2)
                }
            }
            _ => {
                after = None;
                Duration::from_secs(2)
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(delay) => {},
            _ = stopped.changed() => if *stopped.borrow() { return; },
        }
    }
}

async fn provision(provisioner: super::provision::Provisioner, mut stopped: watch::Receiver<bool>) {
    let mut after = None;
    loop {
        if *stopped.borrow() {
            return;
        }
        let page = tokio::time::timeout(
            Duration::from_secs(10),
            provisioner.store.page(after.as_deref()),
        )
        .await;
        let delay = match page {
            Ok(Ok((assets, next))) => {
                for asset in assets {
                    if *stopped.borrow() {
                        return;
                    }
                    let _ = tokio::time::timeout(
                        Duration::from_secs(15),
                        provisioner.advance(&asset.id),
                    )
                    .await;
                }
                after = next;
                if after.is_some() {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(2)
                }
            }
            _ => {
                after = None;
                Duration::from_secs(2)
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(delay) => {},
            _ = stopped.changed() => if *stopped.borrow() { return; },
        }
    }
}
