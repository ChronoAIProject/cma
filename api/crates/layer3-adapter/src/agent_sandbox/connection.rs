//! A native target is independent of the local legacy Kubernetes authority.
use std::path::Path;

use super::{rejected, unavailable};
use crate::Result;

mod metadata;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteKubernetesAuth {
    TokenFile { path: String },
    GoogleMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteKubernetesConfig {
    pub server: String,
    pub ca_file: String,
    pub auth: RemoteKubernetesAuth,
}

impl RemoteKubernetesConfig {
    pub fn validate(&self) -> Result<()> {
        let url = url::Url::parse(&self.server)
            .map_err(|_| rejected("native Kubernetes API must be a canonical HTTPS origin"))?;
        if self.server.len() > 2048
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || self.server != url.origin().ascii_serialization()
        {
            return Err(rejected(
                "native Kubernetes API must be a canonical HTTPS origin",
            ));
        }
        let token_file = match &self.auth {
            RemoteKubernetesAuth::TokenFile { path } => Some(path),
            RemoteKubernetesAuth::GoogleMetadata => None,
        };
        for path in std::iter::once(&self.ca_file).chain(token_file) {
            if path.len() > 4096
                || !Path::new(path).is_absolute()
                || path.chars().any(char::is_control)
            {
                return Err(rejected(
                    "native Kubernetes credential paths must be absolute",
                ));
            }
        }
        Ok(())
    }

    fn configuration(&self) -> Result<kube::Config> {
        self.validate()?;
        let mut config = kube::Config::new(
            self.server
                .parse()
                .map_err(|_| rejected("native Kubernetes API origin is invalid"))?,
        );
        config.root_cert_file = Some(self.ca_file.clone().into());
        if let RemoteKubernetesAuth::TokenFile { path } = &self.auth {
            config.auth_info.token_file = Some(path.clone());
        }
        // Deliberately do not infer kubeconfig, execute credential plugins, apply
        // debug environment overrides, or disable server certificate validation.
        Ok(config)
    }
}

#[derive(Clone)]
pub struct NativeKubernetesConnection {
    pub client: kube::Client,
    pub authority: String,
}

/// Share this exact client and authority between native CRDs and their home store.
/// Explicit remote configuration never changes another client's inferred target.
pub async fn resolve_kubernetes_connection(
    remote: Option<&RemoteKubernetesConfig>,
) -> Result<NativeKubernetesConnection> {
    let config = match remote {
        Some(remote) => remote.configuration()?,
        None => kube::Config::infer()
            .await
            .map_err(|_| unavailable("Kubernetes configuration is unavailable"))?,
    };
    let authority = config.cluster_url.to_string();
    let client = if remote.is_some_and(|remote| remote.auth == RemoteKubernetesAuth::GoogleMetadata)
    {
        metadata::client(config)?
    } else {
        kube::Client::try_from(config).map_err(|_| {
            unavailable("native Kubernetes client or mounted credentials are unavailable")
        })?
    };
    Ok(NativeKubernetesConnection { client, authority })
}

#[cfg(test)]
mod tests;
