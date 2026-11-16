//! Header-boundary HTTP metrics. Response bodies and external delivery are separate.
use axum::http::{Method, StatusCode};
use std::time::Instant;

const ATTEMPTS: &str = "cmaeg_http_attempts_total";
const RESPONSES: &str = "cmaeg_http_responses_total";
const IN_FLIGHT: &str = "cmaeg_http_in_flight";
const CANCELLED: &str = "cmaeg_http_cancelled_before_headers_total";
const LATENCY: &str = "cmaeg_http_response_header_seconds";

pub(super) fn describe() {
    metrics::describe_counter!(ATTEMPTS, "HTTP requests entering the EG router");
    metrics::describe_counter!(RESPONSES, "HTTP responses whose headers were produced");
    metrics::describe_counter!(CANCELLED, "HTTP attempts dropped before response headers");
    metrics::describe_gauge!(IN_FLIGHT, "HTTP requests awaiting response headers");
    metrics::describe_histogram!(
        LATENCY,
        metrics::Unit::Seconds,
        "HTTP time to response headers"
    );
}

pub(super) struct RequestMetrics {
    route: String,
    method: &'static str,
    started: Instant,
    finished: bool,
}

impl RequestMetrics {
    /// Only a framework route template (or `unmatched`), never a raw URI.
    pub(super) fn new(route: String, method: &Method) -> Self {
        let method = match method.as_str() {
            "GET" => "GET",
            "HEAD" => "HEAD",
            "POST" => "POST",
            "PUT" => "PUT",
            "PATCH" => "PATCH",
            "DELETE" => "DELETE",
            "OPTIONS" => "OPTIONS",
            "CONNECT" => "CONNECT",
            "TRACE" => "TRACE",
            _ => "OTHER",
        };
        metrics::counter!(ATTEMPTS, "route" => route.clone(), "method" => method).increment(1);
        metrics::gauge!(IN_FLIGHT, "route" => route.clone(), "method" => method).increment(1.);
        Self {
            route,
            method,
            started: Instant::now(),
            finished: false,
        }
    }

    pub(super) fn finish(&mut self, status: StatusCode) {
        let status = status.as_u16().to_string();
        metrics::counter!(RESPONSES, "route" => self.route.clone(), "method" => self.method,
            "status" => status.clone())
        .increment(1);
        metrics::histogram!(LATENCY, "route" => self.route.clone(), "method" => self.method,
            "status" => status)
        .record(self.started.elapsed().as_secs_f64());
        self.finished = true;
    }
}

impl Drop for RequestMetrics {
    fn drop(&mut self) {
        metrics::gauge!(IN_FLIGHT, "route" => self.route.clone(), "method" => self.method)
            .decrement(1.);
        if !self.finished {
            metrics::counter!(CANCELLED, "route" => self.route.clone(), "method" => self.method)
                .increment(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(text: &str, name: &str) -> f64 {
        text.lines()
            .find(|line| {
                line.starts_with(&format!("{name}{{"))
                    && line.contains("method=\"OTHER\"")
                    && line.contains("route=\"/waiting/{id}\"")
            })
            .unwrap_or_else(|| panic!("missing {name}: {text}"))
            .rsplit_once(' ')
            .unwrap()
            .1
            .parse()
            .unwrap()
    }

    #[test]
    fn dropped_attempt_releases_gauge_and_bounds_extension_method() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let attempt = RequestMetrics::new(
                "/waiting/{id}".into(),
                &Method::from_bytes(b"CUSTOM").unwrap(),
            );
            let waiting = handle.render();
            assert_eq!(sample(&waiting, IN_FLIGHT), 1.);
            drop(attempt);
        });
        let rendered = handle.render();
        assert_eq!(sample(&rendered, IN_FLIGHT), 0.);
        assert_eq!(sample(&rendered, CANCELLED), 1.);
        assert!(!rendered.contains("CUSTOM"));
        assert!(!rendered.contains(RESPONSES));
    }
}
