use super::*;
use crate::store::sandbox_usage_export::CaptureStore;
use layer2_core::sandbox::store::SandboxStoreError;

impl MetricsWorkers {
    pub(super) async fn measured_evidence(&self) {
        let Some(capture) = &self.measured else {
            return;
        };
        let mut ticks = tokio::time::interval(Duration::from_secs(15));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let outcome = tokio::time::timeout(Duration::from_secs(90), async {
                capture.discover_measured().await?;
                for key in capture.next_measured_page().await? {
                    self.measured_minute(capture, &key).await?;
                }
                Ok::<_, SandboxStoreError>(())
            })
            .await;
            if !matches!(outcome, Ok(Ok(()))) {
                tracing::warn!("measured usage evidence cycle incomplete");
            }
        }
    }

    async fn measured_minute(
        &self,
        capture: &CaptureStore,
        key: &str,
    ) -> Result<bool, SandboxStoreError> {
        let Some(lease) = capture.claim_measured(key).await? else {
            return Ok(false);
        };
        let result = async {
            let Some(input) = capture.prepare_measured(&lease).await? else {
                return Ok(false);
            };
            let (mut series, complete) = self
                .raw_for_measured_evidence(&input.bindings(), input.range())
                .await
                .map_err(|_| {
                    SandboxStoreError::Unavailable("measured source unavailable".into())
                })?;
            for value in &mut series {
                value.labels.retain(|key, _| {
                    matches!(
                        key.as_str(),
                        "__name__"
                            | "cluster"
                            | "namespace"
                            | "pod"
                            | "persistentvolumeclaim"
                            | "container"
                            | "job"
                            | "metrics_path"
                    )
                });
            }
            let publication = input.freeze(series, complete)?;
            capture.publish_measured(&lease, &publication).await
        }
        .await;
        capture.release_measured(&lease).await?;
        match result {
            Ok(published) => Ok(published),
            Err(_) => {
                tracing::warn!("measured usage minute remains unresolved");
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
pub(crate) async fn measured_fixture(
    store: Arc<ReportingStore>,
    capture: &CaptureStore,
    source: MetricsClient,
    key: &str,
) -> Result<bool, SandboxStoreError> {
    MetricsWorkers {
        store,
        source: Some(source),
        native: None,
        measured: None,
        cluster: "fixture".into(),
        enabled: true,
    }
    .measured_minute(capture, key)
    .await
}
