//! Own the local listener before advertising its kernel-assigned port.
use crate::{
    http::ManagementState,
    replicas::{drain::Control, runtime::Runtime},
    Config,
};
use std::{
    io::{self, Write},
    net::SocketAddr,
};
use tokio::net::TcpListener;

pub(super) struct Http(Option<TcpListener>);

impl Http {
    pub(super) async fn prepare(config: &mut Config) -> io::Result<Self> {
        if config.listen.port() != 0 || !config.listen.ip().is_loopback() {
            return Ok(Self(None));
        }
        // Membership and generated links need the actual port before startup
        // composes them. Keep the same socket until the router takes ownership.
        let listener = bind_http(config.listen).await?;
        let configured = format!("http://{}", config.listen);
        config.listen = listener.local_addr()?;
        let resolved = format!("http://{}", config.listen);
        for origin in [
            &mut config.management.public_url,
            &mut config.management.public_api_url,
        ] {
            if *origin == configured {
                origin.clone_from(&resolved);
            }
        }
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
