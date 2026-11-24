//! Deployment catalog of published artifacts for fresh owner runtime canaries.

use std::collections::BTreeSet;

use layer3_adapter::agent_sandbox::NativePoolSpec;
use layer3_adapter::{HomeVolumePolicy, RuntimeImage, SandboxCapacity};
use serde::Deserialize;

use super::{parse_pinned_image, parse_sandbox_namespace, SHA256_SEPARATOR};

const KEY: &str = "CHRONO_RUNTIME_CANARY_RELEASES";
const MAX_BYTES: usize = 32 * 1024;
const MAX_RELEASES: usize = 8;
const DESKTOP_LABEL: &str = "dev.chrono.cma.runtime-desktop";
const DESKTOP_ENVIRONMENT_LABEL: &str = "dev.chrono.cma.desktop-environment";
const SOURCE_LABEL: &str = "org.opencontainers.image.revision";

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Catalog(Vec<Release>);

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    release_key: String,
    lane: Lane,
    image: RuntimeImage,
    platform: String,
    capacity: SandboxCapacity,
    config_digest: String,
    source_revision: String,
    publication_run: u64,
    publication_evidence_sha256: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Lane {
    Agent,
    DebianDesktop,
}

impl Catalog {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        home: Option<&HomeVolumePolicy>,
    ) -> Result<Self, String> {
        let Some(raw) = lookup(KEY)? else {
            return Ok(Self::default());
        };
        if raw.len() > MAX_BYTES {
            return Err(invalid("catalog exceeds its byte limit"));
        }
        let releases: Vec<Release> =
            serde_json::from_str(&raw).map_err(|_| invalid("expected a catalog array"))?;
        if releases.len() > MAX_RELEASES {
            return Err(invalid("catalog has too many releases"));
        }
        if releases.is_empty() {
            return Ok(Self::default());
        }
        let home = home
            .filter(|policy| policy.is_native_vct() && policy.mounts_full_home())
            .ok_or_else(|| invalid("catalog requires native full-home allocation"))?;
        let mut keys = BTreeSet::new();
        let mut digests = BTreeSet::new();
        for release in &releases {
            parse_sandbox_namespace(KEY, &release.release_key)
                .map_err(|_| invalid("release key must be a lowercase DNS label"))?;
            let digest = release.validate(home)?;
            if !keys.insert(release.release_key.as_str()) || !digests.insert(digest) {
                return Err(invalid("release keys and image digests must be unique"));
            }
        }
        Ok(Self(releases))
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
}

impl Release {
    fn validate(&self, home: &HomeVolumePolicy) -> Result<&str, String> {
        let digest = exact_image_digest(&self.image.uri).ok_or_else(|| {
            invalid("image must name a registry and repository with only a digest")
        })?;
        if self.platform != "linux/amd64"
            || self.publication_run == 0
            || !self
                .config_digest
                .strip_prefix("sha256:")
                .is_some_and(|digest| lower_hex(digest, 64))
            || !lower_hex(&self.source_revision, 40)
            || !lower_hex(&self.publication_evidence_sha256, 64)
            || self.image.labels.get(SOURCE_LABEL) != Some(&self.source_revision)
        {
            return Err(invalid("invalid platform or publication evidence"));
        }
        let desktop = match self.lane {
            Lane::Agent => "0",
            Lane::DebianDesktop => "1",
        };
        if self.image.labels.get(DESKTOP_LABEL).map(String::as_str) != Some(desktop)
            || self.image.labels.contains_key(DESKTOP_ENVIRONMENT_LABEL)
        {
            return Err(invalid("image labels do not match the canary lane"));
        }
        NativePoolSpec {
            image: self.image.clone(),
            cpu_millis: self.capacity.cpu_millis,
            memory_mib: self.capacity.memory_mib,
            home: home.clone(),
        }
        .validate()
        .map_err(|_| invalid("image or capacity is incompatible with the native home policy"))?;
        Ok(digest)
    }
}

fn exact_image_digest(image: &str) -> Option<&str> {
    parse_pinned_image(KEY, image).ok()?;
    let (name, digest) = image.rsplit_once(SHA256_SEPARATOR)?;
    let (registry, repository) = name.split_once('/')?;
    if !valid_registry(registry)
        || repository.contains(':')
        || name.bytes().any(|byte| byte.is_ascii_uppercase())
        || repository.split('/').any(|part| {
            !part
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
                || !part
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
    {
        return None;
    }
    Some(digest)
}

fn valid_registry(registry: &str) -> bool {
    let (host, port) = match registry.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (registry, None),
    };
    if !host.contains('.') && port.is_none() {
        return false;
    }
    if port.is_some_and(|port| {
        port.parse::<u16>()
            .ok()
            .is_none_or(|number| number == 0 || number.to_string() != port)
    }) {
        return false;
    }
    url::Host::parse(host)
        .ok()
        .is_some_and(|parsed| parsed.to_string() == host)
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn invalid(reason: &str) -> String {
    format!("{KEY} {reason}")
}
