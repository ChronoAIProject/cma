//! Prepared owner policy for generic successful operational reads, disabled by default.
use axum::http::{Method, StatusCode};
use layer2_core::{ActivityAction, ActivityRecord};

#[derive(Default)]
pub(super) struct RequestAuditPolicy {
    sample_operational_reads: bool,
}

impl RequestAuditPolicy {
    pub(super) fn new(sample_operational_reads: bool) -> Self {
        Self {
            sample_operational_reads,
        }
    }

    pub(super) fn retain(
        &self,
        method: &Method,
        matched_route: Option<&str>,
        status: StatusCode,
        record: &ActivityRecord,
    ) -> bool {
        if !self.sample_operational_reads
            || !matches!(*method, Method::GET | Method::HEAD)
            || status != StatusCode::OK
            || record.actor.is_none()
            || record.action != ActivityAction::Read
            || !matches!(
                matched_route,
                Some("/api/v1/sandboxes/{id}" | "/api/v1/workspaces/{id}")
            )
        {
            return true;
        }
        // UUID v4's low bits are uniform. Stable record identity makes this a
        // deterministic 1/16 sample across retries without an actor cache.
        let Some(id) = record
            .id
            .0
            .strip_prefix("act_")
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
        else {
            return true;
        };
        let retained = id.as_u128() & 15 == 0;
        metrics::counter!("cma_activity_operational_read_policy_total", "decision" => if retained {"retained"} else {"sampled_out"}).increment(1);
        retained
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{ActivityId, ActivityOutcome, SubjectId};
    #[test]
    fn default_retains_every_read_and_opt_in_samples_only_the_exact_success_allowlist() {
        let off = RequestAuditPolicy::default();
        let on = RequestAuditPolicy::new(true);
        let mut row = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
            .by(SubjectId::new("fixture"));
        let route = Some("/api/v1/sandboxes/{id}");
        let mut retained = 0;
        for n in 0..256_u128 {
            row.id = ActivityId(format!("act_{:032x}", n));
            assert!(off.retain(&Method::GET, route, StatusCode::OK, &row));
            retained += usize::from(on.retain(&Method::GET, route, StatusCode::OK, &row));
        }
        assert_eq!(retained, 16);
        row.id = ActivityId(format!("act_{:032x}", 1));
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(on.retain(&method, route, StatusCode::OK, &row));
        }
        for status in [
            StatusCode::CREATED,
            StatusCode::FOUND,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(on.retain(&Method::GET, route, status, &row));
        }
        for route in [
            None,
            Some("/auth/session"),
            Some("/api/v1/admin/activity"),
            Some("/api/v1/agents/{id}"),
            Some("/api/v1/sandboxes/actual"),
            Some("/api/v1/sandboxes/{id}/credentials"),
        ] {
            assert!(on.retain(&Method::GET, route, StatusCode::OK, &row));
        }
        assert!(!on.retain(
            &Method::HEAD,
            Some("/api/v1/workspaces/{id}"),
            StatusCode::OK,
            &row
        ));
        row.action = ActivityAction::Configure;
        assert!(on.retain(&Method::GET, route, StatusCode::OK, &row));
        row.action = ActivityAction::Read;
        row.actor = None;
        assert!(on.retain(&Method::GET, route, StatusCode::OK, &row));
        row.actor = Some(SubjectId::new("fixture"));
        row.id = ActivityId("unknown".into());
        assert!(on.retain(&Method::GET, route, StatusCode::OK, &row));
    }
}
