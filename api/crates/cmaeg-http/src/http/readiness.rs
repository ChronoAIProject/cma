//! Project the shared readiness report into the Gateway status response.
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_engine::ReadinessReport;
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

pub fn readiness_response(
    report: ReadinessReport,
    source_sha: Option<&'static str>,
    version: Option<&'static str>,
) -> Response {
    let status = if report.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = ReadinessResponse {
        readiness: &report,
        build: ReleaseBuild::from_stamp(source_sha, version),
    };
    let mut response = (status, Json(body)).into_response();
    // Readiness has a typed diagnostic body even when local startup is incomplete.
    response.extensions_mut().insert(report);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_serializes_only_a_complete_release_stamp() {
        const SHA: &str = "1234567890abcdef1234567890abcdef12345678";
        let report = ReadinessReport {
            ready: true,
            reasons: Vec::new(),
            cma_reachable: false,
        };
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
}
