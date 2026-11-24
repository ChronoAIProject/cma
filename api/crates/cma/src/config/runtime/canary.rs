//! Deployment catalog of published artifacts for fresh owner runtime canaries.

use std::collections::BTreeSet;

use layer3_adapter::agent_sandbox::NativePoolSpec;
use layer3_adapter::{HomeVolumePolicy, RuntimeImage, SandboxCapacity};
use serde::{Deserialize, Serialize};

use super::{parse_pinned_image, parse_sandbox_namespace, SHA256_SEPARATOR};

const KEY: &str = "CHRONO_RUNTIME_CANARY_RELEASES";
const MAX_BYTES: usize = 32 * 1024;
const MAX_RELEASES: usize = 8;
const MAX_REPOSITORY_PATH_BYTES: usize = 255;
const DESKTOP_LABEL: &str = "dev.chrono.cma.runtime-desktop";
const DESKTOP_ENVIRONMENT_LABEL: &str = "dev.chrono.cma.desktop-environment";
const SOURCE_LABEL: &str = "org.opencontainers.image.revision";

#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Catalog(Vec<Release>);

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Release {
    pub(crate) release_key: String,
    pub(crate) lane: Lane,
    pub(crate) image: RuntimeImage,
    pub(crate) platform: String,
    pub(crate) capacity: SandboxCapacity,
    pub(crate) config_digest: String,
    pub(crate) source_revision: String,
    pub(crate) publication_run: u64,
    pub(crate) publication_evidence_sha256: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Lane {
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

    pub(crate) fn release(&self, key: &str) -> Option<&Release> {
        self.0.iter().find(|release| release.release_key == key)
    }

    pub(crate) fn fingerprint(&self) -> Result<String, String> {
        layer2_core::profiles::content_digest(&("runtime-canary-catalog-v1", self))
            .map_err(|_| invalid("catalog fingerprint is unavailable"))
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
            cold_canary: true,
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
        || repository.len() > MAX_REPOSITORY_PATH_BYTES
        || !repository.split('/').all(valid_repository_component)
    {
        return None;
    }
    Some(digest)
}

fn valid_repository_component(mut component: &str) -> bool {
    // Distribution path components alternate lowercase alphanumeric runs
    // with a period, one or two underscores, or one or more hyphens.
    loop {
        let suffix = component.trim_start_matches(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        });
        if suffix.len() == component.len() {
            return false;
        }
        component = match suffix.as_bytes() {
            [] => return true,
            [b'_', b'_', ..] => &suffix[2..],
            [b'.' | b'_', ..] => &suffix[1..],
            [b'-', ..] => suffix.trim_start_matches('-'),
            _ => return false,
        };
    }
}

fn valid_registry(registry: &str) -> bool {
    let (host, port) = match registry.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (registry, None),
    };
    if !host.contains('.') && port.is_none() {
        return false;
    }
    if host
        .split('.')
        .any(|part| parse_sandbox_namespace(KEY, part).is_err())
    {
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

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct WriteScope {
    enabled: bool,
    owners: BTreeSet<layer2_core::SubjectId>,
}

impl WriteScope {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        native: bool,
        catalog: &Catalog,
    ) -> Result<Self, String> {
        Self::from_lookup_keys(
            lookup,
            native,
            catalog,
            "CHRONO_RUNTIME_CANARY_WRITE_ENABLED",
            "CHRONO_RUNTIME_CANARY_OWNERS",
        )
    }

    pub(super) fn retained_from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        native: bool,
        catalog: &Catalog,
    ) -> Result<Self, String> {
        Self::from_lookup_keys(
            lookup,
            native,
            catalog,
            "CHRONO_RETAINED_RUNTIME_TRANSITIONS_ENABLED",
            "CHRONO_RETAINED_RUNTIME_TRANSITION_OWNERS",
        )
    }

    fn from_lookup_keys(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        native: bool,
        catalog: &Catalog,
        enabled_key: &str,
        owners_key: &str,
    ) -> Result<Self, String> {
        let enabled = match lookup(enabled_key)?.as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            _ => return Err(format!("{enabled_key} must be true or false")),
        };
        let owners = match lookup(owners_key)? {
            None => BTreeSet::new(),
            Some(value) if value.len() <= 4096 => {
                serde_json::from_str::<BTreeSet<layer2_core::SubjectId>>(&value)
                    .map_err(|_| format!("{owners_key} must be a JSON list"))?
            }
            Some(_) => return Err(format!("{owners_key} exceeds its byte limit")),
        };
        if owners.len() > MAX_RELEASES
            || owners.iter().any(|owner| {
                let text = owner.as_str();
                text.is_empty()
                    || text.len() > 256
                    || text.trim() != text
                    || text.chars().any(char::is_control)
            })
        {
            return Err(format!("{owners_key} is invalid"));
        }
        if enabled && (!native || catalog.len() == 0 || owners.is_empty()) {
            return Err(
                "runtime canary writes require native allocation, a catalog and an owner scope"
                    .into(),
            );
        }
        Ok(Self { enabled, owners })
    }

    pub(crate) fn allows(&self, owner: &layer2_core::SubjectId) -> bool {
        self.enabled && self.owners.contains(owner)
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn fingerprint(&self) -> Result<String, String> {
        layer2_core::profiles::content_digest(&("runtime-canary-owners-v1", &self.owners))
            .map_err(|_| "runtime canary owner scope fingerprint is unavailable".into())
    }
}
