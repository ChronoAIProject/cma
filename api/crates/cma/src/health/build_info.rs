use axum::{
    extract::State,
    http::{header::CACHE_CONTROL, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;

const PATH: &str = "/api/v1/build-info";

#[derive(Clone, Copy, Serialize)]
struct BuildInfo {
    version: &'static str,
    source_sha: &'static str,
}

impl BuildInfo {
    fn from_stamp(version: Option<&'static str>, source_sha: Option<&'static str>) -> Option<Self> {
        let version = version?;
        let source_sha = source_sha?;
        let valid_revision = source_sha.len() == 40
            && source_sha
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && source_sha.bytes().any(|byte| byte != b'0');
        if !valid_revision {
            return None;
        }
        let valid_version =
            version.strip_prefix("git-") == Some(&source_sha[..12]) || is_release_version(version);
        valid_version.then_some(Self {
            version,
            source_sha,
        })
    }
}

// Match the release-version grammar used by the promotable Web build.
fn is_release_version(version: &str) -> bool {
    let valid_suffix = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
    };
    let mut metadata = version.splitn(2, '+');
    let mut prerelease = metadata.next().unwrap_or_default().splitn(2, '-');
    let core = prerelease.next().unwrap_or_default();
    let mut numbers = core.strip_prefix('v').unwrap_or(core).split('.');
    for _ in 0..3 {
        if !numbers
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return false;
        }
    }
    numbers.next().is_none()
        && prerelease.next().is_none_or(valid_suffix)
        && metadata.next().is_none_or(valid_suffix)
}

fn compiled_stamp() -> Option<BuildInfo> {
    BuildInfo::from_stamp(option_env!("CHRONO_BUILD"), option_env!("CHRONO_BUILD_SHA"))
}

pub(crate) fn print_build_info() -> Result<(), &'static str> {
    let stamp = compiled_stamp().ok_or("Agent Manager has no valid compiled build stamp")?;
    let json = serde_json::to_string(&stamp).map_err(|_| "Could not encode the build stamp")?;
    println!("{json}");
    Ok(())
}

pub(super) fn router() -> Router {
    routes(compiled_stamp())
}

fn routes(stamp: Option<BuildInfo>) -> Router {
    Router::new().route(PATH, get(build_info)).with_state(stamp)
}

async fn build_info(State(stamp): State<Option<BuildInfo>>) -> impl IntoResponse {
    let status = if stamp.is_some() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, [(CACHE_CONTROL, "no-store")], Json(stamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Method, Request},
    };
    use serde_json::{json, Value};
    use tower::ServiceExt;

    const REVISION: &str = "1234567890abcdef1234567890abcdef12345678";

    async fn get(router: Router) -> (StatusCode, Value) {
        let response = router
            .oneshot(Request::get(PATH).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn agent_manager_build_info_exposes_only_the_stamped_release_without_auth() {
        let app = routes(BuildInfo::from_stamp(Some("0.22.34"), Some(REVISION)));
        assert_eq!(
            get(app.clone()).await,
            (
                StatusCode::OK,
                json!({ "version": "0.22.34", "source_sha": REVISION })
            )
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn agent_manager_build_info_accepts_release_suffixes_and_its_own_git_prefix() {
        for version in ["v0.22.34-rc.1+preview.2", "git-1234567890ab"] {
            assert_eq!(
                get(routes(BuildInfo::from_stamp(Some(version), Some(REVISION)))).await,
                (
                    StatusCode::OK,
                    json!({ "version": version, "source_sha": REVISION })
                )
            );
        }
    }

    #[tokio::test]
    async fn agent_manager_build_info_is_unavailable_for_unusable_stamps() {
        for (version, revision) in [
            (None, Some(REVISION)),
            (Some("0.22.34"), None),
            (Some("bad version"), Some(REVISION)),
            (Some("node.example.test"), Some(REVISION)),
            (Some("git-234567890abc"), Some(REVISION)),
            (Some("0.22.34-"), Some(REVISION)),
            (Some("0.22.34+"), Some(REVISION)),
            (Some("0.22.34"), Some("development")),
            (
                Some("0.22.34"),
                Some("0000000000000000000000000000000000000000"),
            ),
        ] {
            assert_eq!(
                get(routes(BuildInfo::from_stamp(version, revision))).await,
                (StatusCode::SERVICE_UNAVAILABLE, Value::Null)
            );
        }
    }

    #[tokio::test]
    async fn agent_manager_health_router_serves_the_compiled_build_stamp_without_store_readiness() {
        let (_sender, store) = tokio::sync::watch::channel(None);
        let response = get(super::super::routes(store, None)).await;
        let expected = match BuildInfo::from_stamp(
            option_env!("CHRONO_BUILD"),
            option_env!("CHRONO_BUILD_SHA"),
        ) {
            Some(stamp) => (
                StatusCode::OK,
                json!({ "version": stamp.version, "source_sha": stamp.source_sha }),
            ),
            None => (StatusCode::SERVICE_UNAVAILABLE, Value::Null),
        };
        assert_eq!(response, expected);
    }
}
