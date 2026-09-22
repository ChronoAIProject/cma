use super::{
    io::{BoxIo, ObservedIo, Socket},
    observation::{ActiveWrite, Pending},
    Driver, ResponseTransport, H1, H2,
};
use crate::providers::ProviderError;
use rustls::{pki_types::ServerName, ClientConfig, RootCertStore};
use std::{sync::Arc, time::Instant};
use tokio::{io::AsyncWriteExt, net::TcpStream};

pub enum Connection {
    H1(H1),
    H2(H2),
}

pub fn tls_config() -> Arc<ClientConfig> {
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("supported TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Arc::new(config)
}
impl ResponseTransport {
    pub(super) async fn connect(&self) -> Result<Connection, ProviderError> {
        tokio::time::timeout(self.connect_timeout, self.connect_inner())
            .await
            .map_err(|_| ProviderError::Unavailable("provider connect timed out".into()))?
    }
    async fn connect_inner(&self) -> Result<Connection, ProviderError> {
        let host = self.origin.host_str().ok_or_else(super::unavailable)?;
        let port = self
            .origin
            .port_or_known_default()
            .ok_or_else(super::unavailable)?;
        #[cfg(test)]
        let tcp = if let Some(address) = self.address {
            TcpStream::connect(address).await
        } else {
            TcpStream::connect((host, port)).await
        };
        #[cfg(not(test))]
        let tcp = TcpStream::connect((host, port)).await;
        let tcp = tcp.map_err(|_| super::unavailable())?;
        tcp.set_nodelay(true).map_err(|_| super::unavailable())?;
        let active: ActiveWrite = Default::default();
        let socket = Socket {
            inner: tcp,
            active: active.clone(),
        };
        let (io, http2): (BoxIo, bool) = if self.origin.scheme() == "https" {
            let server = ServerName::try_from(host.to_owned()).map_err(|_| super::unavailable())?;
            let mut tls = tokio_rustls::TlsConnector::from(self.tls.clone())
                .connect(server, socket)
                .await
                .map_err(|_| super::unavailable())?;
            tls.flush().await.map_err(|_| super::unavailable())?;
            let http2 = tls.get_ref().1.alpn_protocol() == Some(b"h2".as_slice());
            (Box::new(tls), http2)
        } else {
            (
                Box::new(socket),
                host.to_ascii_lowercase().ends_with(".svc"),
            )
        };
        let pending: Pending = Default::default();
        let io = ObservedIo::new(io, active, http2, pending.clone());
        if http2 {
            let (send, connection) = h2::client::handshake(io)
                .await
                .map_err(|_| super::unavailable())?;
            let driver = Arc::new(Driver(tokio::spawn(async move {
                let _ = connection.await;
            })));
            Ok(Connection::H2(H2 {
                send,
                driver,
                pending,
                used: Instant::now(),
            }))
        } else {
            let (send, connection) =
                hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(io))
                    .await
                    .map_err(|_| super::unavailable())?;
            let driver = Arc::new(Driver(tokio::spawn(async move {
                let _ = connection.await;
            })));
            Ok(Connection::H1(H1 {
                send,
                driver,
                pending,
            }))
        }
    }
}
