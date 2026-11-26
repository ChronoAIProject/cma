//! Count provider dispatches, not callers sharing one verification flight.
use std::sync::Arc;

use super::verification_diagnostics::Endpoint;

pub fn route() -> Arc<str> {
    let route = layer2_core::diagnostic_context::DiagnosticContext::current().route;
    // Only fixed route templates enter these labels. Other routes remain counted
    // in the aggregate without accepting a caller-provided path as a metric key.
    match route.as_ref() {
        "GET /auth/session"
        | "GET /api/v1/agents"
        | "POST /api/v1/agents"
        | "GET /api/v1/agents/{id}"
        | "GET /api/v1/sandboxes"
        | "GET /api/v1/sandboxes/{id}"
        | "POST /api/v1/sandboxes"
        | "GET /api/v1/workspaces"
        | "GET /api/v1/workspaces/{id}"
        | "POST /api/v1/workspaces" => route,
        _ => "other".into(),
    }
}

pub fn resolution(source: &'static str) {
    metrics::counter!("cma_identity_verifications_total", "route" => route().to_string(), "source" => source).increment(1);
}

pub struct Call {
    endpoint: Endpoint,
    route: Arc<str>,
    finished: bool,
}

impl Call {
    pub fn start(endpoint: Endpoint) -> Self {
        let route = route();
        metrics::counter!("cma_identity_provider_calls_total", "endpoint" => endpoint.label(), "route" => route.to_string()).increment(1);
        Self {
            endpoint,
            route,
            finished: false,
        }
    }

    pub fn finish(&mut self, status: Option<u16>) {
        self.finished = true;
        let outcome = match status {
            Some(200) => "200",
            Some(401) => "401",
            Some(403) => "403",
            Some(429) => "429",
            Some(502) => "502",
            Some(503) => "503",
            Some(_) => "other_status",
            None => "transport",
        };
        self.record(outcome);
    }

    fn record(&self, outcome: &'static str) {
        metrics::counter!("cma_identity_provider_results_total", "endpoint" => self.endpoint.label(), "route" => self.route.to_string(), "outcome" => outcome).increment(1);
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        if !self.finished {
            self.record("cancelled");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn calls_results_and_callers_are_distinct_and_labels_never_contain_private_context() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recording = metrics::set_default_local_recorder(&recorder);
        let mut context = layer2_core::diagnostic_context::DiagnosticContext::current();
        context.route = "GET /api/v1/agents/{id}".into();
        context.request_hash = "private-request".into();
        context
            .scope(async {
                let mut accepted = Call::start(Endpoint::Userinfo);
                accepted.finish(Some(200));
                let mut throttled = Call::start(Endpoint::Introspection);
                throttled.finish(Some(429));
                let mut failed = Call::start(Endpoint::Discovery);
                failed.finish(None);
                drop(Call::start(Endpoint::AgentKey));
                resolution("led");
                resolution("shared");
            })
            .await;
        let mut context = layer2_core::diagnostic_context::DiagnosticContext::current();
        context.route = "GET /private/owner/token".into();
        context
            .scope(async {
                let mut call = Call::start(Endpoint::Userinfo);
                call.finish(Some(500));
            })
            .await;
        let output = handle.render();
        assert!(output.contains("cma_identity_provider_calls_total{endpoint=\"userinfo\",route=\"GET /api/v1/agents/{id}\"} 1"));
        for outcome in ["200", "429", "transport", "cancelled", "other_status"] {
            assert!(
                output.contains(&format!("outcome=\"{outcome}\"}} 1")),
                "{output}"
            );
        }
        for source in ["led", "shared"] {
            assert!(
                output.contains(&format!("source=\"{source}\"}} 1")),
                "{output}"
            );
        }
        for private in ["private", "owner", "token", "request_hash"] {
            assert!(!output.contains(private));
        }
    }
}
