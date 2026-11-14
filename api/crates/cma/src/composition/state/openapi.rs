//! Startup work that must finish before the HTTP listener is bound.

pub(super) async fn warm(build: impl FnOnce() + Send + 'static) -> Result<(), String> {
    // JoinError includes panics. Propagate them to serve()'s fatal error log;
    // continuing would leave a poisoned LazyLock in the request path.
    tokio::task::spawn_blocking(build)
        .await
        .map_err(|error| format!("OpenAPI cache initialization failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc};

    #[tokio::test(flavor = "current_thread")]
    async fn warming_finishes_on_a_blocking_thread_before_startup_continues() {
        let completed = Arc::new(AtomicBool::new(false));
        let captured = completed.clone();
        let runtime_thread = std::thread::current().id();
        warm(move || {
            assert_ne!(std::thread::current().id(), runtime_thread);
            layer1_public::rest::openapi_cache::warm();
            captured.store(true, Ordering::SeqCst);
        })
        .await
        .unwrap();
        assert!(completed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn warming_panic_fails_startup() {
        let error = warm(|| panic!("document fixture failed"))
            .await
            .unwrap_err();
        assert!(error.contains("OpenAPI cache initialization failed"));
        assert!(error.contains("document fixture failed"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn warmed_document_burst_keeps_real_liveness_route_responsive() {
        use axum::{body::Body, http::Request, http::StatusCode};
        use std::time::{Duration, Instant};
        use tokio::sync::Barrier;
        use tower::ServiceExt;

        struct Healthy;
        #[async_trait::async_trait]
        impl crate::health::ReadinessProbe for Healthy {
            async fn ping(&self) -> Result<(), layer2_core::StoreError> {
                Ok(())
            }
        }

        warm(layer1_public::rest::openapi_cache::warm)
            .await
            .unwrap();
        let router = layer1_public::rest::router().merge(crate::health::router(Arc::new(Healthy)));
        let barrier = Arc::new(Barrier::new(66));
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..64 {
            let barrier = barrier.clone();
            let router = router.clone();
            tasks.spawn(async move {
                barrier.wait().await;
                let path = ["/openapi.json", "/agent-manager/openapi.json"][index % 2];
                let request = Request::get(path).body(Body::empty()).unwrap();
                assert_eq!(
                    router.oneshot(request).await.unwrap().status(),
                    StatusCode::OK
                );
            });
        }
        let probe_barrier = barrier.clone();
        let probe = tokio::spawn(async move {
            probe_barrier.wait().await;
            let request = Request::get("/livez").body(Body::empty()).unwrap();
            router.oneshot(request).await.unwrap()
        });
        let start = Instant::now();
        barrier.wait().await;
        assert_eq!(probe.await.unwrap().status(), StatusCode::OK);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(50),
            "livez took {elapsed:?}"
        );
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    }
}
