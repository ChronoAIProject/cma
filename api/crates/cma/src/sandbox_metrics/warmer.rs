//! Operator-approved materialization scopes, not cached end-user authority.
//! Configured scopes determine background work only; every consumer still proves
//! current principal/capabilities and full live membership before reading bytes.
use super::*;
use layer2_core::sandbox_metrics::{MetricsStore, Scope, ViewInput};
use mongodb::bson::doc;

impl MetricsWorkers {
    pub(super) async fn warm_views(&self) {
        let policy = &self.store.view_policy;
        if !policy.enabled || !policy.warm_enabled {
            return;
        }
        let mut ticks = tokio::time::interval(Duration::from_secs(policy.warm_seconds));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let holder = format!("usage-warmer-{}", uuid::Uuid::new_v4().simple());
            let result =
                tokio::time::timeout(Duration::from_secs(45), self.schedule_warm_views(&holder))
                    .await;
            tracing::info!(
                success = matches!(result, Ok(Ok(()))),
                scopes = policy.warm_scopes.len(),
                "usage report warmer scheduled"
            );
            let _ = self.store.release("view-warmer", &holder).await;
        }
    }
    async fn schedule_warm_views(&self, holder: &str) -> layer2_core::sandbox_metrics::Result<()> {
        let policy = &self.store.view_policy;
        if !policy.enabled || !policy.warm_enabled {
            return Ok(());
        }
        self.store
            .check_circuit(crate::store::usage_reports::now())?;
        self.store.collection(crate::store::usage_reports::CONTROL).update_one(doc! {"_id":"view-warmer"},
            doc! {"$setOnInsert":{"document_schema_version":1_i64,"lease_until":0_i64,"holder":""}})
            .upsert(true).await.map_err(crate::store::usage_reports::unavailable)?;
        if !self.store.lease("view-warmer", holder, 70).await? {
            return Ok(());
        }
        for registered in &policy.warm_scopes {
            let at = crate::store::usage_reports::now();
            let mut input = ViewInput {
                window: registered.window,
                ..ViewInput::default()
            };
            input.filters.sandbox_id = registered.sandbox_id.clone();
            let selection = input.validate(at)?;
            let scope = registered.scope();
            // A test-Sandbox canary must still belong to the named principal.
            // Fleet-wide admin materialization needs a separately approved
            // unfiltered operator scope; registration never implies that grant.
            if let Some(id) = &registered.sandbox_id {
                self.store
                    .authorize_sandbox(
                        &Scope {
                            admin: false,
                            ..scope.clone()
                        },
                        id,
                    )
                    .await?;
            }
            self.store.latest_view(&scope, &selection, at).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) async fn warm_fixture(
    store: Arc<ReportingStore>,
) -> layer2_core::sandbox_metrics::Result<()> {
    let worker = MetricsWorkers {
        store,
        source: None,
        native: None,
        measured: None,
        cluster: "fixture".into(),
        enabled: false,
    };
    let holder = format!("warm-fixture-{}", uuid::Uuid::new_v4().simple());
    let result = worker.schedule_warm_views(&holder).await;
    if worker.store.view_policy.enabled && worker.store.view_policy.warm_enabled {
        worker.store.release("view-warmer", &holder).await?;
    }
    result
}
