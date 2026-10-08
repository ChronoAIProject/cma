//! The receipt time of a tool request, including time spent reading its body.
use axum::{extract::Request, middleware::Next, response::Response};
use tokio::time::Instant;

/// Public for direct callers of the shared tools service; never a wire field.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct RequestStart(pub Instant);

pub(crate) async fn stamp(mut request: Request, next: Next) -> Response {
    // Reply-target routes can also be mounted under the tools router. The
    // outermost receipt time must survive a second instance of this layer.
    request
        .extensions_mut()
        .get_or_insert_with(|| RequestStart(Instant::now()));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        middleware::from_fn,
        routing::post,
        Extension, Router,
    };
    use hyper::service::Service;
    use hyper_util::service::TowerToHyperService;
    use std::{convert::Infallible, time::Duration};

    #[tokio::test(start_paused = true)]
    async fn receipt_time_precedes_body_read_and_survives_nested_stamps() {
        async fn handler(Extension(start): Extension<RequestStart>, request: Request) -> String {
            to_bytes(request.into_body(), 1024).await.unwrap();
            start.0.elapsed().as_secs().to_string()
        }
        let app = Router::new()
            .route("/mcp", post(handler))
            .layer(from_fn(stamp))
            .layer(from_fn(stamp));
        let body = Body::from_stream(futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok::<_, Infallible>(bytes::Bytes::from_static(b"{}"))
        }));
        let response = TowerToHyperService::new(app)
            .call(Request::post("/mcp").body(body).unwrap())
            .await
            .unwrap();
        assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "3");
    }

    #[tokio::test(start_paused = true)]
    async fn nested_routes_preserve_the_first_receipt_time() {
        let first = Instant::now();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let app = Router::new()
            .route(
                "/target",
                post(|Extension(start): Extension<RequestStart>| async move {
                    start.0.elapsed().as_secs().to_string()
                }),
            )
            .layer(from_fn(stamp));
        let mut request = Request::post("/target").body(Body::empty()).unwrap();
        request.extensions_mut().insert(RequestStart(first));
        let response = TowerToHyperService::new(app).call(request).await.unwrap();
        assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "2");
    }
}
