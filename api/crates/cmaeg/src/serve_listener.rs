//! Own the local listener before advertising its kernel-assigned port.
use crate::{
    config::ManagementConfig,
    http::ManagementState,
    replicas::{drain::Control, runtime::Runtime},
    Config,
};
use std::{
    io::{self, Write},
    net::SocketAddr,
};
use tokio::net::TcpListener;
use url::{Origin, Url};

pub(super) struct Http(Option<TcpListener>);

impl Http {
    pub(super) async fn prepare(config: &mut Config) -> io::Result<Self> {
        if config.listen.port() != 0 || !config.listen.ip().is_loopback() {
            return Ok(Self(None));
        }
        // Membership and generated links need the actual port before startup
        // composes them. Keep the same socket until the router takes ownership.
        let listener = bind_http(config.listen).await?;
        let bound = listener.local_addr()?;
        resolve_public_endpoints(&mut config.management, config.listen, bound.port())?;
        config.listen = bound;
        Ok(Self(Some(listener)))
    }

    pub(super) async fn start_replicas(
        &self,
        enabled: bool,
        config: &Config,
        state: &ManagementState,
        control: &Control,
    ) -> Result<Option<Runtime>, Box<dyn std::error::Error + Send + Sync>> {
        if enabled {
            let local_address = self.0.as_ref().map(|_| config.listen);
            Runtime::start(config, state, control.clone(), local_address)
                .await
                .map(Some)
        } else {
            Runtime::verify_legacy(state, &config.management.public_url).await?;
            Ok(None)
        }
    }

    pub(super) async fn bind(self, address: SocketAddr) -> io::Result<(TcpListener, bool)> {
        match self.0 {
            Some(listener) => Ok((listener, true)),
            None => Ok((bind_http(address).await?, false)),
        }
    }
}

fn resolve_public_endpoints(
    management: &mut ManagementConfig,
    configured: SocketAddr,
    port: u16,
) -> io::Result<()> {
    let origin = parse_url(&format!("http://{configured}"))?.origin();
    for url in [&mut management.public_url, &mut management.public_api_url]
        .into_iter()
        .chain(management.console_public_url.iter_mut())
        .chain(management.public_api_aliases.iter_mut())
    {
        resolve_port(url, &origin, port)?;
    }
    Ok(())
}

fn parse_url(value: &str) -> io::Result<Url> {
    Url::parse(value).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid public URL"))
}

fn resolve_port(value: &mut String, origin: &Origin, port: u16) -> io::Result<()> {
    let mut url = parse_url(value)?;
    if &url.origin() == origin {
        url.set_port(Some(port))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid public URL port"))?;
        *value = url.as_str().trim_end_matches('/').to_owned();
    }
    Ok(())
}

#[test]
fn resolves_matching_loopback_origins_and_preserves_paths_and_independent_origins() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use cmaeg_core::{channel::Secret, seal::MasterKeyRing};
    use std::sync::Arc;

    let configured = "127.0.0.1:0".parse().unwrap();
    let mut management = ManagementConfig {
        public_url: "http://127.0.0.1:18999".into(),
        console_public_url: Some("http://127.0.0.1:0".into()),
        public_api_url: "http://127.0.0.1:0/event-gateway".into(),
        public_api_aliases: vec![
            "http://127.0.0.1:0/v1".into(),
            "https://gateway.example/v1".into(),
            "http://127.0.0.2:0/v1".into(),
        ],
        advertise_public_api_url: true,
        master_keys: Arc::new(
            MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1_u8; 32]))).unwrap(),
        ),
        cma_base_url: "https://cma.example".into(),
        cma_service_token: Secret::new("service-fixture"),
        nyxid_base_url: "https://nyxid.example".into(),
        tools_nyxid_slug: "cmaeg".into(),
    };
    resolve_public_endpoints(&mut management, configured, 12345).unwrap();
    assert_eq!(management.public_url, "http://127.0.0.1:18999");
    assert_eq!(
        management.console_public_url.as_deref(),
        Some("http://127.0.0.1:12345")
    );
    assert_eq!(
        management.public_api_url,
        "http://127.0.0.1:12345/event-gateway"
    );
    assert_eq!(
        management.public_api_aliases,
        [
            "http://127.0.0.1:12345/v1",
            "https://gateway.example/v1",
            "http://127.0.0.2:0/v1",
        ],
    );
    management.public_url = "http://127.0.0.1:0".into();
    resolve_public_endpoints(&mut management, configured, 12345).unwrap();
    assert_eq!(management.public_url, "http://127.0.0.1:12345");
    management.public_url = "invalid".into();
    assert!(resolve_public_endpoints(&mut management, configured, 12345).is_err());
}

pub(super) fn announce(
    listener: &TcpListener,
    metrics: SocketAddr,
    ephemeral: bool,
) -> io::Result<()> {
    if ephemeral {
        // Local callers need a reliable handoff even when tracing is filtered.
        let mut stdout = io::stdout().lock();
        writeln!(
            stdout,
            "CMAEG_BOUND_LISTENERS {} {metrics}",
            listener.local_addr()?
        )?;
        stdout.flush()?;
    }
    Ok(())
}

async fn bind_http(address: SocketAddr) -> io::Result<TcpListener> {
    TcpListener::bind(address)
        .await
        .map_err(|error| io::Error::new(error.kind(), format!("HTTP listener {address}: {error}")))
}
