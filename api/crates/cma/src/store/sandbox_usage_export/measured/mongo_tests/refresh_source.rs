use super::*;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Router,
};
use layer3_adapter::sandbox_metrics::MetricsClient;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub(super) enum Response {
    Values(f64),
    MemoryTimeout,
    TotalTimeout,
    Empty,
    CpuGap,
}

pub(super) struct Source {
    mode: Arc<Mutex<Response>>,
    calls: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
    client: MetricsClient,
    reporting: Arc<usage_reports::ReportingStore>,
}

impl Source {
    pub(super) async fn new(f: &Fixture) -> Self {
        let mode = Arc::new(Mutex::new(Response::Values(60.0)));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/select/0/prometheus/api/v1/export", get(export))
            .with_state((mode.clone(), calls.clone(), f.minute));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client =
            MetricsClient::internal(&format!("http://{address}/select/0/prometheus/")).unwrap();
        let reporting = usage_reports::ReportingStore::connect_with_budgets(
            &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
            f.db.name(),
            b"fixture-signing-key",
            crate::config::sandbox_metrics::MetricsBudgets {
                source_seconds: 1,
                ..Default::default()
            },
            crate::config::sandbox_metrics::DEFAULT_REPORTING_POOL_SIZE,
        )
        .await
        .unwrap();
        Self {
            mode,
            calls,
            task,
            client,
            reporting,
        }
    }

    pub(super) fn set(&self, mode: Response) {
        *self.mode.lock().unwrap() = mode;
        self.calls.lock().unwrap().clear();
    }

    pub(super) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub(super) async fn run(&self, f: &Fixture) -> bool {
        crate::sandbox_metrics::measured_fixture(
            self.reporting.clone(),
            &f.store,
            self.client.clone(),
            &keys::stream("fixture", &f.stream.id),
        )
        .await
        .unwrap()
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type SourceState = (Arc<Mutex<Response>>, Arc<Mutex<Vec<String>>>, u64);
async fn export(
    State((mode, calls, minute)): State<SourceState>,
    Query(query): Query<BTreeMap<String, String>>,
) -> (StatusCode, String) {
    let cpu = query["match[]"].contains("container_cpu_usage_seconds_total");
    calls
        .lock()
        .unwrap()
        .push(if cpu { "cpu" } else { "memory" }.into());
    let response = *mode.lock().unwrap();
    if matches!(response, Response::TotalTimeout)
        || (matches!(response, Response::MemoryTimeout) && !cpu)
    {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        return (StatusCode::GATEWAY_TIMEOUT, String::new());
    }
    if matches!(response, Response::Empty) {
        return (StatusCode::OK, String::new());
    }
    let delta = match response {
        Response::Values(delta) => delta,
        _ => 60.0,
    };
    let end = if matches!(response, Response::CpuGap) && cpu {
        minute + 30
    } else {
        minute + 60
    };
    (StatusCode::OK, serde_json::json!({
        "metric": {"__name__":if cpu {"container_cpu_usage_seconds_total"} else {"container_memory_working_set_bytes"},
            "cluster":"fixture","namespace":"fixture","pod":"pod-fixture","container":"runtime", "job":"kubelet","metrics_path":"/metrics/resource"},
        "values": if cpu {vec![10.0, 10.0+delta]} else {vec![1024.0,1024.0]},
        "timestamps":[minute*1000,end*1000]
    }).to_string()+"\n")
}
