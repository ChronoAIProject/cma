//! Attribution is set only after authentication. Headers alone never prove a consumer.
use axum::extract::Request;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Consumer {
    Unknown,
    PublicUser,
    Nyxid,
    DirectApi,
    Internal,
}
impl Consumer {
    fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::PublicUser => "public_user",
            Self::Nyxid => "nyxid",
            Self::DirectApi => "direct_api",
            Self::Internal => "internal",
        }
    }
}
#[derive(Clone, Default)]
pub struct Attribution(Arc<AtomicU8>);
impl Attribution {
    pub fn consumer(&self) -> &'static str {
        match self.0.load(Ordering::Relaxed) {
            1 => Consumer::PublicUser,
            2 => Consumer::Nyxid,
            3 => Consumer::DirectApi,
            4 => Consumer::Internal,
            _ => Consumer::Unknown,
        }
        .label()
    }
}

/// One authentication boundary, including cancellation and early refusals.
pub struct Resolution {
    attribution: Option<Attribution>,
    path: &'static str,
    started: std::time::Instant,
    outcome: &'static str,
}
impl Resolution {
    pub fn start(request: &Request, path: &'static str) -> Self {
        Self {
            attribution: request.extensions().get::<Attribution>().cloned(),
            path,
            started: std::time::Instant::now(),
            outcome: "cancelled",
        }
    }
    pub fn rejected<T>(mut self, response: T) -> T {
        self.finish("rejected", Consumer::Unknown);
        response
    }
    pub fn finish(&mut self, outcome: &'static str, consumer: Consumer) {
        self.outcome = outcome;
        if let Some(attribution) = &self.attribution {
            attribution.0.store(consumer as u8, Ordering::Relaxed);
        }
    }
}
impl Drop for Resolution {
    fn drop(&mut self) {
        metrics::counter!("cma_auth_resolutions_total", "path" => self.path, "outcome" => self.outcome).increment(1);
        metrics::histogram!("cma_auth_resolution_seconds", "path" => self.path, "outcome" => self.outcome).record(self.started.elapsed().as_secs_f64());
    }
}

pub fn protocol(route: &str) -> &'static str {
    if route.starts_with("/api/v1/agui/") {
        "agui"
    } else if route.starts_with("/api/v2/responses")
        || route.starts_with("/api/v1/responses")
        || route.starts_with("/api/v1/conversations")
        || route.starts_with("/api/v2/commands")
    {
        "responses"
    } else if route.starts_with("/api/") {
        "rest"
    } else if route.starts_with("/auth/") {
        "auth"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_verified_resolution_can_attribute_a_request() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let mut request = Request::builder()
                .header("x-consumer", "nyxid")
                .body(axum::body::Body::empty())
                .unwrap();
            let attribution = Attribution::default();
            request.extensions_mut().insert(attribution.clone());
            assert_eq!(attribution.consumer(), "unknown");
            let mut resolution = Resolution::start(&request, "ambient");
            resolution.finish("resolved", Consumer::Nyxid);
            assert_eq!(attribution.consumer(), "nyxid");
        });
        assert!(handle
            .render()
            .contains("path=\"ambient\",outcome=\"resolved\""));
        assert_eq!(protocol("/api/v1/agui/events"), "agui");
        assert_eq!(protocol("/api/v2/responses/{response_id}"), "responses");
    }
}
