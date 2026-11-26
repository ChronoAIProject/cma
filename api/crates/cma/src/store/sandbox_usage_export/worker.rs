use super::*;
use futures_util::TryStreamExt;
use std::sync::Arc;

impl MongoSandboxStore {
    pub(crate) fn spawn_usage_projection(
        self: &Arc<Self>,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let capture = self.usage_capture.clone()?;
        let store = self.clone();
        Some(tokio::spawn(async move {
            let mut ticks = tokio::time::interval(std::time::Duration::from_secs(15));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut after = String::new();
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = ticks.tick() => {
                        tokio::select! {
                            _ = stop.changed() => break,
                            result = async {
                                if capture.baseline(&store, &layer2_core::now_rfc3339(), 32).await.is_err() {
                                    tracing::warn!("allocated usage inventory incomplete");
                                }
                                capture.sweep(&after).await
                            } => match result {
                                Ok(next) => after = next,
                                Err(_) => tracing::error!("allocated usage projection unavailable"),
                            }
                        }
                    }
                }
            }
        }))
    }
}

impl CaptureStore {
    async fn sweep(&self, after: &str) -> Result<String, SandboxStoreError> {
        let mut cursor = self
            .collection(CAPTURES)
            .find(doc! {"complete":false,"_id":{"$gt":after}})
            .projection(doc! {"_id":1})
            .sort(doc! {"_id":1})
            .limit(32)
            .await
            .map_err(unavailable)?;
        let mut next = String::new();
        let now =
            layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).ok_or_else(corrupt)?;
        while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
            next = row.get_str("_id").map_err(|_| corrupt())?.into();
            if self.project_one(&next, now).await.is_err() {
                tracing::warn!(source_hash = %records::hash(next.as_bytes()), "allocated usage projection held");
            }
        }
        Ok(next)
    }
}
