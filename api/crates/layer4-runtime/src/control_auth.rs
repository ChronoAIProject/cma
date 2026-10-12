//! Optional per-runtime authentication for direct in-cluster transport.
//! Legacy OpenSandbox deployments retain their provider proxy authentication.
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use subtle::ConstantTimeEq;

pub(crate) struct ControlAuth(String);

impl ControlAuth {
    pub(crate) fn from_environment() -> Result<Option<Arc<Self>>, String> {
        let Ok(value) = std::env::var("CHRONO_RUNTIME_CONTROL_TOKEN") else {
            return Ok(None);
        };
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("CHRONO_RUNTIME_CONTROL_TOKEN must be a 256-bit hexadecimal token".into());
        }
        // Runtime children are spawned with explicit environment maps. Also
        // remove the bootstrap value from the supervisor's process environment.
        std::env::remove_var("CHRONO_RUNTIME_CONTROL_TOKEN");
        Ok(Some(Arc::new(Self(value))))
    }
}

pub(crate) async fn authenticate(
    State(auth): State<Arc<ControlAuth>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if matches!(request.uri().path(), "/livez" | "/ready" | "/readyz") {
        return Ok(next.run(request).await);
    }
    let mut values = request.headers().get_all("x-cma-runtime-token").iter();
    let value = values.next().ok_or(StatusCode::UNAUTHORIZED)?;
    if values.next().is_some() || !bool::from(auth.0.as_bytes().ct_eq(value.as_bytes())) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get, Router};
    use tower::ServiceExt;
    #[tokio::test]
    async fn control_auth_requires_one_exact_token_but_allows_liveness() {
        let token = uuid::Uuid::new_v4().simple().to_string().repeat(2);
        let app = Router::new()
            .route("/control", get(|| async { "private" }))
            .route("/livez", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                Arc::new(ControlAuth(token.clone())),
                authenticate,
            ));
        for (path, values, expected) in [
            ("/control", vec![], 401),
            ("/control", vec!["wrong".to_owned()], 401),
            ("/control", vec![token.clone(), token.clone()], 401),
            ("/control", vec![token], 200),
            ("/livez", vec![], 200),
        ] {
            let mut request = Request::builder().uri(path);
            for value in values {
                request = request.header("x-cma-runtime-token", value)
            }
            assert_eq!(
                app.clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status()
                    .as_u16(),
                expected
            );
        }
    }
}
