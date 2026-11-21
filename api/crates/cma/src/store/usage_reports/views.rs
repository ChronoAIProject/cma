//! Latest pointers select immutable namespaces; they never authorize a caller.
use super::*;
use layer2_core::sandbox_metrics::{
    MetricsStore, RefreshState, ReportStatus, ReportView, Scope, ViewRefresh, ViewSelection,
};

pub(super) const VIEW_KIND: &str = "retained_view_v1";
pub(super) const READ_GRACE_SECONDS: u64 = 30;

#[derive(Clone)]
pub(crate) struct ViewLease {
    pub key: String,
    pub revision: i64,
}
impl ReportingStore {
    pub(crate) fn with_view_policy(
        &self,
        policy: crate::config::report_views::ViewPolicy,
    ) -> Arc<Self> {
        let mut store = self.clone();
        store.view_policy = Arc::new(policy);
        Arc::new(store)
    }

    async fn empty_latest_view(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
        key: &str,
    ) -> Result<ReportView> {
        let report = self.enqueue_view(scope, selection, at, key).await?;
        Ok(ReportView {
            schema_version: 1,
            window: selection.window,
            requested_range: selection.query.range(),
            generated_at: None,
            stale: false,
            refresh: ViewRefresh {
                state: RefreshState::Pending,
                report_id: Some(report.report_id.clone()),
                retry_after_seconds: 2,
            },
            report,
        })
    }
    pub(crate) async fn latest_view(
        &self,
        scope: &Scope,
        selection: &ViewSelection,
        at: u64,
    ) -> Result<ReportView> {
        if !self.view_policy.enabled {
            return Err(MetricsError::ViewDisabled);
        }
        if !self
            .view_policy
            .view_scopes
            .iter()
            .any(|allowed| allowed.allows(scope, selection.query.filters.sandbox_id.as_deref()))
        {
            return self.on_demand_view(scope, selection, at).await;
        }
        self.check_circuit(at)?;
        let key = selection.key(scope)?;
        let pointer = self
            .collection(CONTROL)
            .find_one(doc! {"_id":&key})
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        let mut ready = None;
        if let Some(pointer) = &pointer {
            if pointer.get_str("kind").ok() != Some(VIEW_KIND)
                || pointer.get_str("principal").ok() != Some(&scope.principal)
                || pointer.get_bool("admin").ok() != Some(scope.admin)
            {
                return Err(MetricsError::Unavailable);
            }
            if pointer.contains_key("latest_id") && pointer.get_str("latest_id").is_err() {
                return Err(MetricsError::Unavailable);
            }
            if pointer.contains_key("active_id") && pointer.get_str("active_id").is_err() {
                return Err(MetricsError::Unavailable);
            }
            if let Ok(id) = pointer.get_str("latest_id") {
                let retain_until = pointer
                    .get_i64("latest_retain_until")
                    .map_err(unavailable)?;
                if retain_until <= at as i64 {
                    return self.empty_latest_view(scope, selection, at, &key).await;
                }
                match self.report(scope, id, at).await {
                    Ok(report) => {
                        if report.status != ReportStatus::Ready
                            || !selection.matches(scope, &report)?
                        {
                            return Err(MetricsError::Unavailable);
                        }
                        let generation =
                            pointer.get_i64("latest_generation").map_err(unavailable)?;
                        let completed = self
                            .retained_namespace(&report)
                            .await?
                            .ok_or(MetricsError::Unavailable)?;
                        if completed.get_i64("generation").ok() != Some(generation) {
                            return Err(MetricsError::Unavailable);
                        }
                        let generated =
                            u64::try_from(pointer.get_i64("generated_at").map_err(unavailable)?)
                                .map_err(unavailable)?;
                        if completed.get_i64("generated_at").ok() != Some(generated as i64) {
                            return Err(MetricsError::Unavailable);
                        }
                        ready = Some((report, generated));
                    }
                    // Expiry has an explicit first-build response, never an empty success.
                    Err(MetricsError::Expired) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let stale = ready.as_ref().is_some_and(|(_, generated)| {
            at.saturating_sub(*generated) >= self.view_policy.freshness_seconds
        });
        let refresh = if ready.is_none() || stale || selection.refresh {
            match self.enqueue_view(scope, selection, at, &key).await {
                Ok(report) => ViewRefresh {
                    state: RefreshState::Pending,
                    report_id: Some(report.report_id),
                    retry_after_seconds: 2,
                },
                Err(MetricsError::Busy | MetricsError::Unavailable) if ready.is_some() => {
                    ViewRefresh {
                        state: RefreshState::Deferred,
                        report_id: None,
                        retry_after_seconds: 30,
                    }
                }
                Err(error) => return Err(error),
            }
        } else if let Some(active) = pointer.as_ref().and_then(|p| p.get_str("active_id").ok()) {
            // Explicit refresh can run while the retained snapshot is still
            // fresh. Subsequent observations must keep that job visible.
            match self.report(scope, active, at).await {
                Ok(report) if report.status == ReportStatus::Pending => {
                    if !selection.matches(scope, &report)? {
                        return Err(MetricsError::Unavailable);
                    }
                    ViewRefresh {
                        state: RefreshState::Pending,
                        report_id: Some(report.report_id),
                        retry_after_seconds: 2,
                    }
                }
                // A racing commit/failure is observed on the next view read;
                // the previously selected, complete snapshot remains safe.
                Ok(_) | Err(MetricsError::Expired | MetricsError::Unavailable) => ViewRefresh {
                    state: RefreshState::Deferred,
                    report_id: None,
                    retry_after_seconds: 2,
                },
                Err(error) => return Err(error),
            }
        } else {
            ViewRefresh {
                state: RefreshState::Idle,
                report_id: None,
                retry_after_seconds: 0,
            }
        };
        let (report, generated_at) = match ready {
            Some((report, generated)) => (report, Some(generated)),
            None => (
                self.report(
                    scope,
                    refresh
                        .report_id
                        .as_deref()
                        .ok_or(MetricsError::Unavailable)?,
                    at,
                )
                .await?,
                None,
            ),
        };
        // A worker may finish between admission and readback. Use the immutable
        // completion receipt instead of returning Ready with pending metadata.
        let (report, generated_at, stale, refresh) =
            if report.status == ReportStatus::Ready && generated_at.is_none() {
                let job = self
                    .retained_namespace(&report)
                    .await?
                    .ok_or(MetricsError::Unavailable)?;
                let generated = u64::try_from(job.get_i64("generated_at").map_err(unavailable)?)
                    .map_err(unavailable)?;
                (
                    report,
                    Some(generated),
                    false,
                    ViewRefresh {
                        state: RefreshState::Idle,
                        report_id: None,
                        retry_after_seconds: 0,
                    },
                )
            } else {
                (report, generated_at, stale, refresh)
            };
        metrics::counter!("cma_usage_view_responses_total", "state" => if generated_at.is_none() { "pending" } else if stale { "stale" } else { "fresh" }).increment(1);
        Ok(ReportView {
            schema_version: 1,
            window: selection.window,
            requested_range: selection.query.range(),
            report,
            generated_at,
            stale,
            refresh,
        })
    }
}
