//! HTTP readiness response and compatibility exports.
pub use crate::readiness::{ReachabilityProbe, Readiness, ReadinessReport};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

#[derive(Serialize)]
struct ReadinessResponse<'a> {
    #[serde(flatten)]
    readiness: &'a ReadinessReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<ReleaseBuild>,
}

#[derive(Serialize)]
struct ReleaseBuild {
    source_sha: &'static str,
    version: &'static str,
}

impl ReleaseBuild {
    fn from_stamp(source_sha: Option<&'static str>, version: Option<&'static str>) -> Option<Self> {
        let source_sha = source_sha?;
        let version = version?;
        let full_revision = source_sha.len() == 40
            && source_sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && source_sha.bytes().any(|b| b != b'0');
        (full_revision && !version.is_empty()).then_some(Self {
            source_sha,
            version,
        })
    }
}

pub async fn readyz(State(readiness): State<Readiness>) -> Response {
    let report = readiness.report();
    let status = if report.ready {
        readiness.note_served_ready();
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = ReadinessResponse {
        readiness: &report,
        build: ReleaseBuild::from_stamp(
            option_env!("CHRONO_BUILD_SHA"),
            option_env!("CHRONO_BUILD"),
        ),
    };
    let mut response = (status, Json(body)).into_response();
    // Readiness has a typed diagnostic body even when local startup is incomplete.
    response.extensions_mut().insert(report);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn readiness_serializes_only_a_complete_release_stamp() {
        const SHA: &str = "1234567890abcdef1234567890abcdef12345678";
        let report = Readiness::initialized().report();
        let response = |source, version| {
            serde_json::to_value(ReadinessResponse {
                readiness: &report,
                build: ReleaseBuild::from_stamp(source, version),
            })
            .unwrap()
        };
        assert_eq!(
            response(Some(SHA), Some("1.2.3"))["build"],
            serde_json::json!({"source_sha": SHA, "version": "1.2.3"})
        );
        for (source, version) in [
            (None, Some("1.2.3")),
            (Some(SHA), None),
            (Some(SHA), Some("")),
            (Some("development"), Some("development")),
            (
                Some("0000000000000000000000000000000000000000"),
                Some("1.2.3"),
            ),
        ] {
            assert!(response(source, version).get("build").is_none());
        }
    }

    #[tokio::test]
    async fn served_waits_for_the_first_ready_answer() {
        let readiness = Readiness::initialized();
        let waiting = tokio::spawn({
            let readiness = readiness.clone();
            async move { readiness.served().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        assert_eq!(readyz(State(readiness.clone())).await.status(), 200);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        // Later waiters return at once.
        readiness.served().await;
    }

    #[tokio::test]
    async fn an_unready_answer_is_not_noted() {
        let readiness = Readiness::initialized();
        readiness.start_draining();
        assert_eq!(readyz(State(readiness.clone())).await.status(), 503);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), readiness.served())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn first_ready_answer_wakes_all_registered_clones_and_late_waiters() {
        let readiness = Readiness::initialized();
        let clones = vec![readiness.clone(); 16];
        let mut waiters: Vec<_> = clones.iter().map(|r| Box::pin(r.served())).collect();
        for waiter in &mut waiters {
            assert!(futures_util::poll!(waiter.as_mut()).is_pending());
        }
        assert_eq!(readyz(State(readiness.clone())).await.status(), 200);
        for waiter in &mut waiters {
            assert!(futures_util::poll!(waiter.as_mut()).is_ready());
        }
        readiness.start_draining();
        assert_eq!(readyz(State(readiness.clone())).await.status(), 503);
        assert!(futures_util::poll!(Box::pin(readiness.served())).is_ready());
    }
}
