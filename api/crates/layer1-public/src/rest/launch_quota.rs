//! Reservation boundary for visitor Trigger Launch quotas.

use std::time::SystemTime;

use async_trait::async_trait;
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::{rfc3339_at, trigger_domain::LaunchCreateResult, Caller};

use super::{problem_document, problem_response};

const VISITOR_LAUNCH_RESOURCE: &str = "visitor_launches";

/// Injected quota boundary used by the public Trigger Launch create route.
#[async_trait]
pub trait LaunchQuota: Send + Sync {
    /// Reserve capacity after outer idempotency and body validation, but before admission.
    async fn reserve(&self, caller: &Caller) -> Result<LaunchQuotaPermit, LaunchQuotaError>;
}

/// One reserved quota unit that releases itself unless a new Launch is created.
pub struct LaunchQuotaPermit {
    settlement: Option<Box<dyn FnOnce(bool) + Send + 'static>>,
}

impl LaunchQuotaPermit {
    /// Build a permit whose callback receives whether capacity was committed.
    pub fn reserved(settlement: impl FnOnce(bool) + Send + 'static) -> Self {
        Self {
            settlement: Some(Box::new(settlement)),
        }
    }

    /// Build a no-op permit for callers outside the visitor quota.
    pub fn unmetered() -> Self {
        Self { settlement: None }
    }

    /// Commit only a newly-created Launch; an inner replay releases the reservation.
    pub fn settle(self, result: &LaunchCreateResult) {
        self.settle_created(result.created);
    }

    fn settle_created(mut self, created: bool) {
        if let Some(settlement) = self.settlement.take() {
            settlement(created);
        }
    }
}

impl std::fmt::Debug for LaunchQuotaPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LaunchQuotaPermit")
            .field("reserved", &self.settlement.is_some())
            .finish()
    }
}

impl Drop for LaunchQuotaPermit {
    fn drop(&mut self) {
        if let Some(settlement) = self.settlement.take() {
            settlement(false);
        }
    }
}

/// Stable reset evidence returned when visitor Launch capacity is exhausted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchQuotaRefusal {
    limit: u32,
    reset_at: SystemTime,
}

impl LaunchQuotaRefusal {
    /// Describe an exhausted daily visitor Launch allowance.
    pub fn visitor_launches(limit: u32, reset_at: SystemTime) -> Self {
        Self { limit, reset_at }
    }

    pub const fn code(&self) -> &'static str {
        "resource_quota"
    }

    pub const fn resource(&self) -> &'static str {
        VISITOR_LAUNCH_RESOURCE
    }

    pub const fn limit(&self) -> u32 {
        self.limit
    }

    pub fn reset_at(&self) -> SystemTime {
        self.reset_at
    }

    pub fn reset_at_rfc3339(&self) -> String {
        rfc3339_at(self.reset_at)
    }

    pub fn retry_after_seconds(&self, now: SystemTime) -> u64 {
        self.reset_at
            .duration_since(now)
            .map(ceil_seconds)
            .unwrap_or(1)
    }
}

/// Failure to reserve capacity before Launch admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchQuotaError {
    ResourceQuota(LaunchQuotaRefusal),
    Unavailable,
}

impl LaunchQuotaError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ResourceQuota(refusal) => refusal.code(),
            Self::Unavailable => "launch_quota_unavailable",
        }
    }

    fn into_response_at(self, now: SystemTime) -> Response {
        match self {
            Self::ResourceQuota(refusal) => quota_response(refusal, now),
            Self::Unavailable => {
                let status = StatusCode::SERVICE_UNAVAILABLE;
                problem_response(
                    status,
                    problem_document(
                        status,
                        "launch-quota-unavailable",
                        "Visitor Launch capacity could not be checked.",
                        "launch_quota_unavailable",
                    ),
                )
            }
        }
    }
}

impl std::fmt::Display for LaunchQuotaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for LaunchQuotaError {}

impl IntoResponse for LaunchQuotaError {
    fn into_response(self) -> Response {
        self.into_response_at(SystemTime::now())
    }
}

fn quota_response(refusal: LaunchQuotaRefusal, now: SystemTime) -> Response {
    let status = StatusCode::TOO_MANY_REQUESTS;
    let mut document = problem_document(
        status,
        "resource-quota",
        "This visitor has exhausted the daily Trigger Launch allowance.",
        refusal.code(),
    );
    let fields = document
        .as_object_mut()
        .expect("a problem document is a JSON object");
    fields.insert("resource".into(), refusal.resource().into());
    fields.insert("limit".into(), refusal.limit().into());
    fields.insert("reset_at".into(), refusal.reset_at_rfc3339().into());
    let mut response = problem_response(status, document);
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&refusal.retry_after_seconds(now).to_string())
            .expect("a positive integer is a valid Retry-After value"),
    );
    response
}

fn ceil_seconds(duration: std::time::Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() != 0))
        .max(1)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, UNIX_EPOCH},
    };

    use axum::body::to_bytes;

    use super::*;

    #[test]
    fn permit_is_send_and_settles_exactly_once() {
        fn assert_send<T: Send>() {}
        assert_send::<LaunchQuotaPermit>();

        let outcomes = Arc::new(Mutex::new(Vec::new()));
        for created in [false, true] {
            let recorded = Arc::clone(&outcomes);
            LaunchQuotaPermit::reserved(move |committed| recorded.lock().unwrap().push(committed))
                .settle_created(created);
        }
        assert_eq!(*outcomes.lock().unwrap(), [false, true]);
    }

    #[test]
    fn dropped_permit_releases_once() {
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&outcomes);
        let permit =
            LaunchQuotaPermit::reserved(move |committed| recorded.lock().unwrap().push(committed));
        drop(permit);
        assert_eq!(*outcomes.lock().unwrap(), [false]);
    }

    #[tokio::test]
    async fn cancelled_async_work_releases_the_send_permit() {
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&outcomes);
        let (ready, held) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _permit = LaunchQuotaPermit::reserved(move |committed| {
                recorded.lock().unwrap().push(committed)
            });
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        held.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(*outcomes.lock().unwrap(), [false]);
    }

    #[tokio::test]
    async fn resource_quota_response_has_reset_evidence_and_retry_after() {
        let now = UNIX_EPOCH + Duration::from_secs(20_000 * 86_400 + 60);
        let reset_at = now + Duration::from_secs(61);
        let refusal = LaunchQuotaRefusal::visitor_launches(5, reset_at);
        let response = LaunchQuotaError::ResourceQuota(refusal.clone()).into_response_at(now);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "61");
        assert_eq!(refusal.retry_after_seconds(now), 61);

        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "resource_quota");
        assert_eq!(body["resource"], VISITOR_LAUNCH_RESOURCE);
        assert_eq!(body["limit"], 5);
        assert_eq!(body["reset_at"], rfc3339_at(reset_at));
    }
}
