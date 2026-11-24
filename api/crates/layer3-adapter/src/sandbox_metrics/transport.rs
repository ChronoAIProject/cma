use super::{
    decode_export, decode_raw_export, selector, PhysicalSeries, RawPhysicalSeries, Selection,
};
use crate::{AdapterError, Result};
use futures_util::StreamExt;
use std::{sync::Arc, time::Duration};

pub(super) const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const PROXY: &str = "/api/v1/namespaces/monitoring/services/vmselect-vm-stack-victoria-metrics-k8s-stack:8481/proxy/select/0/prometheus/api/v1/export";

#[derive(Clone)]
pub enum MetricsTransport {
    Internal {
        client: reqwest::Client,
        base: url::Url,
    },
    KubernetesProxy(kube::Client),
}
#[derive(Clone)]
pub struct MetricsClient {
    transport: MetricsTransport,
    budget: Arc<tokio::sync::Semaphore>,
}
impl MetricsClient {
    pub fn new(transport: MetricsTransport) -> Self {
        Self {
            transport,
            budget: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }
    pub fn internal(base: &str) -> Result<Self> {
        let base = url::Url::parse(base).map_err(|_| unavailable())?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.path() != "/select/0/prometheus/"
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(AdapterError::Rejected(
                "monitoring URL must identify the fixed tenant 0 Prometheus base".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self::new(MetricsTransport::Internal { client, base }))
    }
    pub async fn export(&self, selection: &Selection) -> Result<Vec<PhysicalSeries>> {
        self.export_decoded(selection, decode_export).await
    }
    pub async fn export_raw(&self, selection: &Selection) -> Result<Vec<RawPhysicalSeries>> {
        self.export_decoded(selection, decode_raw_export).await
    }
    async fn export_decoded<T>(
        &self,
        selection: &Selection,
        decode: impl FnOnce(&[u8], &Selection) -> Result<T>,
    ) -> Result<T> {
        let query = selector(selection)?;
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            let _permit = self.budget.acquire().await.map_err(|_| unavailable())?;
            let params = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("match[]", &query)
                .append_pair("start", &selection.from.to_string())
                .append_pair("end", &selection.to.to_string())
                .append_pair("reduce_mem_usage", "1")
                .append_pair("deny_partial_response", "1")
                .finish();
            let mut body = Vec::new();
            match &self.transport {
                MetricsTransport::Internal { client, base } => {
                    let mut url = base.join("api/v1/export").map_err(|_| unavailable())?;
                    url.set_query(Some(&params));
                    let response = client.get(url).send().await.map_err(|_| unavailable())?;
                    if !response.status().is_success()
                        || response
                            .content_length()
                            .is_some_and(|v| v > MAX_RESPONSE_BYTES as u64)
                    {
                        return Err(unavailable());
                    }
                    let mut stream = response.bytes_stream();
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.map_err(|_| unavailable())?;
                        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                            return Err(unavailable());
                        }
                        body.extend_from_slice(&chunk);
                    }
                }
                MetricsTransport::KubernetesProxy(client) => {
                    use http_body_util::BodyExt;
                    let request = http::Request::get(format!("{PROXY}?{params}"))
                        .body(kube::client::Body::empty())
                        .map_err(|_| unavailable())?;
                    let response = client.send(request).await.map_err(|_| unavailable())?;
                    if !response.status().is_success() {
                        return Err(unavailable());
                    }
                    let mut stream = response.into_body().into_data_stream();
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.map_err(|_| unavailable())?;
                        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                            return Err(unavailable());
                        }
                        body.extend_from_slice(&chunk);
                    }
                }
            }
            decode(&body, selection)
        })
        .await
        .map_err(|_| unavailable())?;
        tracing::info!(
            elapsed_ms = start.elapsed().as_millis() as u64,
            success = result.is_ok(),
            identities = selection.names.len(),
            "sandbox metrics upstream read"
        );
        result
    }
}
fn unavailable() -> AdapterError {
    AdapterError::Unavailable("bounded monitoring source unavailable".into())
}
