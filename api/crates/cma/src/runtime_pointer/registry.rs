//! Verify OCI metadata supplied by the release registry before prewarming.

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;
use layer2_core::released_runtime::ReleasedLane;
use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{PointerObservation, PointerUnavailable, RuntimePointerSource};

const HOST: &str = "ghcr.io";
const AGENT: &str = "chronoaiproject/cma-runtime";
const MACHINE: &str = "chronoaiproject/cma-runtime-omarchy";
const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
const DOCKER_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";
const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
const DOCKER_INDEX: &str = "application/vnd.docker.distribution.manifest.list.v2+json";
const OCI_CONFIG: &str = "application/vnd.oci.image.config.v1+json";
const DOCKER_CONFIG: &str = "application/vnd.docker.container.image.v1+json";
const MANIFEST_ACCEPT: &str =
    "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json, application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json";
const BODY_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RuntimePointerConfig {
    token: Option<String>,
    tag: String,
    machine: bool,
}

impl RuntimePointerConfig {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        has_machine: bool,
    ) -> Result<Option<Self>, String> {
        let enabled = match lookup("CHRONO_RUNTIME_POINTER_ENABLED")?.as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            _ => return Err("CHRONO_RUNTIME_POINTER_ENABLED must be true or false".into()),
        };
        let agent = lookup("CHRONO_RUNTIME_POINTER")?.filter(|value| !value.is_empty());
        let machine = lookup("CHRONO_MACHINE_RUNTIME_POINTER")?.filter(|value| !value.is_empty());
        let token = if enabled {
            lookup("CHRONO_RUNTIME_REGISTRY_TOKEN")?.filter(|token| !token.is_empty())
        } else {
            None
        };
        if agent.is_none() && machine.is_none() && token.is_none() {
            return Ok(None);
        }
        if token.as_ref().is_some_and(|token| {
            token.len() > 4096 || !token.bytes().all(|byte| byte.is_ascii_graphic())
        }) {
            return Err("CHRONO_RUNTIME_REGISTRY_TOKEN is not a valid registry credential".into());
        }
        let tag = match lookup("CHRONO_ENVIRONMENT")?.as_deref() {
            Some("staging") => "staging-current",
            Some("production") => "production-current",
            _ => {
                return Err(
                    "runtime pointer following requires a staging or production environment".into(),
                )
            }
        };
        if agent.as_deref() != Some(format!("{HOST}/{AGENT}:{tag}").as_str()) {
            return Err(
                "CHRONO_RUNTIME_POINTER must match this environment's CMA runtime pointer".into(),
            );
        }
        if machine.is_some() != has_machine
            || machine
                .as_ref()
                .is_some_and(|value| value != &format!("{HOST}/{MACHINE}:{tag}"))
        {
            return Err(
                "CHRONO_MACHINE_RUNTIME_POINTER must match the configured machine lane".into(),
            );
        }
        Ok(Some(Self {
            token,
            tag: tag.into(),
            machine: has_machine,
        }))
    }

    pub(crate) fn can_follow(&self) -> bool {
        self.token.is_some()
    }
}

pub(crate) struct RegistryRuntimePointers {
    http: reqwest::Client,
    config: RuntimePointerConfig,
}

#[derive(Deserialize)]
struct RegistryToken {
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    media_type: String,
    #[serde(default)]
    manifests: Vec<Descriptor>,
    config: Option<Descriptor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    digest: String,
    media_type: String,
    size: usize,
    platform: Option<Platform>,
}

#[derive(Deserialize)]
struct Platform {
    os: String,
    architecture: String,
    variant: Option<String>,
}

impl Platform {
    fn supported(&self) -> bool {
        self.os == "linux" && self.architecture == "amd64" && self.variant.is_none()
    }
}

#[derive(Deserialize)]
struct ConfigBlob {
    #[serde(flatten)]
    platform: Platform,
    config: ConfigInner,
}

#[derive(Deserialize)]
struct ConfigInner {
    #[serde(rename = "Labels", default)]
    labels: BTreeMap<String, String>,
}

impl RegistryRuntimePointers {
    pub(crate) fn new(config: RuntimePointerConfig) -> Result<Self, PointerUnavailable> {
        let http = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| PointerUnavailable)?;
        Ok(Self { http, config })
    }

    async fn registry_token(&self, repository: &str) -> Result<String, PointerUnavailable> {
        let scope = format!("repository:{repository}:pull");
        let reply = self
            .http
            .get(format!("https://{HOST}/token"))
            .query(&[("service", HOST), ("scope", scope.as_str())])
            .basic_auth("x-access-token", self.config.token.as_ref())
            .send()
            .await
            .map_err(|_| PointerUnavailable)?;
        let token: RegistryToken = decode(&body(reply).await?)?;
        if token.token.is_empty() || !token.token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(PointerUnavailable);
        }
        Ok(token.token)
    }

    async fn manifest(
        &self,
        repo: &str,
        reference: &str,
        token: &str,
    ) -> Result<(String, Manifest, usize), PointerUnavailable> {
        let reply = self
            .http
            .get(format!("https://{HOST}/v2/{repo}/manifests/{reference}"))
            .bearer_auth(token)
            .header(ACCEPT, MANIFEST_ACCEPT)
            .send()
            .await
            .map_err(|_| PointerUnavailable)?;
        let expected = reply
            .headers()
            .get("Docker-Content-Digest")
            .and_then(|header| header.to_str().ok())
            .filter(|digest| valid_digest(digest))
            .ok_or(PointerUnavailable)?
            .to_owned();
        let media = reply
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|header| header.to_str().ok())
            .and_then(|header| header.split(';').next())
            .ok_or(PointerUnavailable)?
            .to_owned();
        let bytes = body(reply).await?;
        if fingerprint(&bytes) != expected || (valid_digest(reference) && reference != expected) {
            return Err(PointerUnavailable);
        }
        let manifest: Manifest = decode(&bytes)?;
        if manifest.schema_version != 2 || manifest.media_type != media {
            return Err(PointerUnavailable);
        }
        Ok((expected, manifest, bytes.len()))
    }

    async fn configuration(
        &self,
        repo: &str,
        descriptor: &Descriptor,
        token: &str,
    ) -> Result<ConfigBlob, PointerUnavailable> {
        if !matches!(descriptor.media_type.as_str(), OCI_CONFIG | DOCKER_CONFIG)
            || !descriptor.valid()
        {
            return Err(PointerUnavailable);
        }
        let mut reply = self
            .http
            .get(format!(
                "https://{HOST}/v2/{repo}/blobs/{}",
                descriptor.digest
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| PointerUnavailable)?;
        if reply.status().is_redirection() {
            // Registry blob redirects are signed, so registry credentials never
            // accompany the second request.
            let url = reply
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| url::Url::parse(value).ok())
                .ok_or(PointerUnavailable)?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url
                    .host_str()
                    .is_none_or(|host| host != HOST && !host.ends_with(".githubusercontent.com"))
            {
                return Err(PointerUnavailable);
            }
            reply = self
                .http
                .get(url)
                .send()
                .await
                .map_err(|_| PointerUnavailable)?;
        }
        let bytes = body(reply).await?;
        if bytes.len() != descriptor.size || fingerprint(&bytes) != descriptor.digest {
            return Err(PointerUnavailable);
        }
        decode(&bytes)
    }
}

impl Descriptor {
    fn valid(&self) -> bool {
        valid_digest(&self.digest) && (1..=BODY_LIMIT).contains(&self.size)
    }
}

#[async_trait]
impl RuntimePointerSource for RegistryRuntimePointers {
    async fn observe(
        &self,
        lane: ReleasedLane,
    ) -> Result<Option<PointerObservation>, PointerUnavailable> {
        let repo = match lane {
            ReleasedLane::Agent => AGENT,
            ReleasedLane::Machine if self.config.machine => MACHINE,
            ReleasedLane::Machine => return Ok(None),
        };
        let token = self.registry_token(repo).await?;
        let (digest, root, _) = self.manifest(repo, &self.config.tag, &token).await?;
        let image = format!("{HOST}/{repo}@{digest}");
        let refuse = |reason| {
            Ok(Some(PointerObservation::Refused {
                image: image.clone(),
                reason,
            }))
        };
        let manifest = match root.media_type.as_str() {
            OCI_INDEX | DOCKER_INDEX => {
                let mut platforms = root
                    .manifests
                    .iter()
                    .filter(|entry| entry.platform.as_ref().is_some_and(Platform::supported));
                let Some(platform) = platforms.next() else {
                    return refuse("platform_unavailable");
                };
                if platforms.next().is_some()
                    || !platform.valid()
                    || !matches!(platform.media_type.as_str(), OCI_MANIFEST | DOCKER_MANIFEST)
                {
                    return refuse("platform_ambiguous");
                }
                let (_, manifest, size) = self.manifest(repo, &platform.digest, &token).await?;
                if size != platform.size || manifest.media_type != platform.media_type {
                    return Err(PointerUnavailable);
                }
                manifest
            }
            OCI_MANIFEST | DOCKER_MANIFEST => root,
            _ => return refuse("unsupported_manifest"),
        };
        let config = manifest.config.as_ref().ok_or(PointerUnavailable)?;
        let config = self.configuration(repo, config, &token).await?;
        if !config.platform.supported() {
            return refuse("platform_unavailable");
        }
        if config
            .config
            .labels
            .get(layer3_adapter::RUNTIME_CONTRACT_LABEL)
            .map(String::as_str)
            != Some(
                layer3_adapter::RUNTIME_CONTRACT_VERSION
                    .to_string()
                    .as_str(),
            )
        {
            return refuse("runtime_contract");
        }
        if config
            .config
            .labels
            .get("org.opencontainers.image.version")
            .is_none_or(|value| semver::Version::parse(value).is_err())
        {
            return refuse("runtime_version");
        }
        Ok(Some(PointerObservation::Verified(image)))
    }
}

pub(super) fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn fingerprint(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, PointerUnavailable> {
    serde_json::from_slice(bytes).map_err(|_| PointerUnavailable)
}

async fn body(mut reply: reqwest::Response) -> Result<Vec<u8>, PointerUnavailable> {
    if !reply.status().is_success() {
        return Err(PointerUnavailable);
    }
    let mut result = Vec::new();
    while let Some(chunk) = reply.chunk().await.map_err(|_| PointerUnavailable)? {
        if result.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err(PointerUnavailable);
        }
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
