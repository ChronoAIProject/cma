//! Keep small interactive frames immediate on the remote Kubernetes connection.
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client as HttpClient},
    rt::TokioExecutor,
};
use kube::client::{retry::RetryPolicy, Body, ClientBuilder, ConfigExt};
use tower::{retry::RetryLayer, ServiceBuilder};

use super::{unavailable, MetadataLayer};
use crate::Result;

pub(super) fn client(config: kube::Config, metadata: &MetadataLayer) -> Result<kube::Client> {
    // This configuration is constructed from the explicit remote target, never
    // inferred from kubeconfig. Keep its mounted CA, timeouts and request layers.
    // kube's default connector leaves Nagle enabled, adding an ACK round trip to
    // successive small desktop/conversation frames over a distant cluster API.
    let mut tcp = HttpConnector::new();
    tcp.enforce_http(false);
    tcp.set_nodelay(true);
    let tls = config
        .rustls_https_connector_with_connector(tcp)
        .map_err(|_| unavailable("native Kubernetes client or mounted CA is unavailable"))?;
    let mut connector = hyper_timeout::TimeoutConnector::new(tls);
    connector.set_connect_timeout(config.connect_timeout);
    connector.set_read_timeout(config.read_timeout);
    connector.set_write_timeout(config.write_timeout);
    let http: HttpClient<_, Body> = HttpClient::builder(TokioExecutor::new()).build(connector);
    let auth = config
        .auth_layer()
        .map_err(|_| unavailable("native Kubernetes mounted credentials are unavailable"))?;
    let headers = config
        .extra_headers_layer()
        .map_err(|_| unavailable("native Kubernetes request headers are invalid"))?;
    let service = ServiceBuilder::new()
        .layer(config.base_uri_layer())
        .option_layer(
            config
                .default_retry
                .then_some(RetryLayer::new(RetryPolicy::server_retry())),
        )
        .option_layer(auth)
        .layer(headers)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .map_err(tower::BoxError::from)
        .service(http);
    Ok(ClientBuilder::new(service, config.default_namespace)
        .with_layer(metadata)
        .build())
}
