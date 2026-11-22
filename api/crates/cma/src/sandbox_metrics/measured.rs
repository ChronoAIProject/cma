use super::*;

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
                    let Some(lease) = capture.claim_measured(&key).await? else {
                        continue;
                    };
                    let result = async {
                        if let Some(input) = capture.prepare_measured(&lease).await? {
                            let (mut series, complete) = self
                                .raw(&input.bindings(), input.range())
                                .await
                                .map_err(|_| {
                                    layer2_core::sandbox::store::SandboxStoreError::Unavailable(
                                        "measured source unavailable".into(),
                                    )
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
                            capture.publish_measured(&lease, &publication).await?;
                        }
                        Ok::<_, layer2_core::sandbox::store::SandboxStoreError>(())
                    }
                    .await;
                    capture.release_measured(&lease).await?;
                    if result.is_err() {
                        tracing::warn!("measured usage minute remains unresolved");
                    }
                }
                Ok::<_, layer2_core::sandbox::store::SandboxStoreError>(())
            })
            .await;
            if !matches!(outcome, Ok(Ok(()))) {
                tracing::warn!("measured usage evidence cycle incomplete");
            }
        }
    }
}
