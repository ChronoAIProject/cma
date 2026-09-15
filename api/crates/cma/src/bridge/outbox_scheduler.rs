use layer2_core::bridge::outbox::RelayDeliveryPort;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
};

use crate::store::MongoBridgeReplyToolStore;

pub(crate) struct BridgeOutboxScheduler {
    store: Arc<MongoBridgeReplyToolStore>,
    port: Arc<dyn RelayDeliveryPort>,
    holder: String,
}
impl BridgeOutboxScheduler {
    pub(crate) fn new(
        store: Arc<MongoBridgeReplyToolStore>,
        port: Arc<dyn RelayDeliveryPort>,
        holder: String,
    ) -> Self {
        Self {
            store,
            port,
            holder,
        }
    }
    /// Four independent attempts share Mongo leases; each holder retains its
    /// scan cursor. Shutdown permits in-flight sends to publish receipts for up
    /// to 25 seconds, then stops them; uncompleted Sending rows remain unknown.
    pub(crate) fn spawn(self, mut stop: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut lanes = JoinSet::new();
            for lane in 0..4 {
                let store = self.store.clone();
                let port = self.port.clone();
                let receiver = stop.clone();
                let holder = format!("{}:{lane}", self.holder);
                lanes.spawn(run_lane(store, port, holder, receiver));
            }
            while !*stop.borrow() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
            if tokio::time::timeout(Duration::from_secs(25), async {
                while lanes.join_next().await.is_some() {}
            })
            .await
            .is_err()
            {
                lanes.abort_all();
                while lanes.join_next().await.is_some() {}
            }
        })
    }
}
async fn run_lane(
    store: Arc<MongoBridgeReplyToolStore>,
    port: Arc<dyn RelayDeliveryPort>,
    holder: String,
    mut stop: watch::Receiver<bool>,
) {
    let mut last_warning = None;
    loop {
        if *stop.borrow() {
            return;
        }
        let delay = match store.deliver_one(&holder, port.as_ref()).await {
            Ok(true) => Duration::from_millis(1),
            Ok(false) => Duration::from_millis(100),
            Err(error) => {
                let now = tokio::time::Instant::now();
                if last_warning.is_none_or(|prior| now - prior >= Duration::from_secs(30)) {
                    tracing::warn!(failure=%error,"Bridge outbox attempt paused");
                    last_warning = Some(now);
                }
                Duration::from_secs(1)
            }
        };
        tokio::select! {
            result=stop.changed()=>{if result.is_err()||*stop.borrow(){return;}},
            _=store.notified()=>{},
            _=tokio::time::sleep(delay)=>{},
        }
    }
}
