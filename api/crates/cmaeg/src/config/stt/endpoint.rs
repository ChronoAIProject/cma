//! The speech-to-text endpoint: its URL and platform key, the model, the
//! accepted formats and the per-attempt timeout.
//!
//! Neither the URL nor the key ever appears in an error, a log line or a
//! Debug rendering: the URL may embed a tenant-specific path and the key is a
//! platform credential.
use super::{
    super::{media::bounded, ConfigError},
    CMAEG_STT_API_KEY, CMAEG_STT_FORMATS, CMAEG_STT_MODEL, CMAEG_STT_TIMEOUT_MS, CMAEG_STT_URL,
    KNOWN_FORMATS,
};
use cmaeg_core::channel::Secret;
use reqwest::Url;
use std::{collections::BTreeSet, fmt, time::Duration};

/// The containers the OpenAI speech-to-text API documents.
pub(super) const DEFAULT_FORMATS: [&str; 7] = ["mp3", "mp4", "mpeg", "mpga", "m4a", "wav", "webm"];
pub(super) const DEFAULT_MODEL: &str = "gpt-transcribe";
pub(super) const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const TIMEOUT_MS_RANGE: (u64, u64) = (1_000, 120_000);
const MAX_MODEL_BYTES: usize = 128;
const MAX_KEY_BYTES: usize = 4_096;

/// Where transcriptions are sent, and the platform key sent with them.
#[derive(Clone, PartialEq, Eq)]
pub struct SttEndpoint {
    pub url: Url,
    pub api_key: Secret,
}

/// Neither the URL nor the key may reach a log line.
impl fmt::Debug for SttEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SttEndpoint")
            .finish_non_exhaustive()
    }
}

/// The endpoint settings as read, before they join the spend limits.
pub(super) struct EndpointSettings {
    pub endpoint: Option<SttEndpoint>,
    pub model: String,
    pub formats: BTreeSet<&'static str>,
    pub timeout: Duration,
}

/// Read the endpoint keys through `lookup`. The key is read only when a URL
/// is set: with speech-to-text disabled it is ignored, whatever it holds.
pub(super) fn read(
    lookup: &impl Fn(&'static str) -> Result<Option<String>, ConfigError>,
) -> Result<EndpointSettings, ConfigError> {
    let model = model(lookup(CMAEG_STT_MODEL)?.as_deref().unwrap_or(DEFAULT_MODEL))?;
    let formats = match lookup(CMAEG_STT_FORMATS)? {
        Some(value) => formats(&value)?,
        None => DEFAULT_FORMATS.into_iter().collect(),
    };
    let timeout_ms = bounded(
        CMAEG_STT_TIMEOUT_MS,
        lookup(CMAEG_STT_TIMEOUT_MS)?.as_deref(),
        DEFAULT_TIMEOUT_MS,
        TIMEOUT_MS_RANGE.0,
        TIMEOUT_MS_RANGE.1,
    )?;
    let endpoint = match lookup(CMAEG_STT_URL)?.filter(|url| !url.is_empty()) {
        None => None,
        Some(url) => Some(SttEndpoint {
            url: endpoint_url(&url)?,
            api_key: api_key(lookup(CMAEG_STT_API_KEY)?)?,
        }),
    };
    Ok(EndpointSettings {
        endpoint,
        model,
        formats,
        timeout: Duration::from_millis(timeout_ms),
    })
}

/// An absolute `https` URL with a host and no userinfo, query or fragment.
/// `http` is accepted only with a loopback host, for local tests, as for
/// `CMAEG_PUBLIC_URL`.
fn endpoint_url(value: &str) -> Result<Url, ConfigError> {
    let invalid = || ConfigError::InvalidMedia {
        detail: format!(
            "{CMAEG_STT_URL} must be an absolute https URL with a host and no credentials, \
             query or fragment (http only with a loopback host)"
        ),
    };
    let url = Url::parse(value).map_err(|_| invalid())?;
    let loopback = match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    let scheme_allowed = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    if !scheme_allowed
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(url)
}

/// The platform key: required with a URL, printable ASCII without
/// whitespace, so it can only ever be a bearer token.
fn api_key(value: Option<String>) -> Result<Secret, ConfigError> {
    let Some(value) = value else {
        return Err(ConfigError::InvalidMedia {
            detail: format!("{CMAEG_STT_API_KEY} must be set when {CMAEG_STT_URL} is set"),
        });
    };
    if value.is_empty() || value.len() > MAX_KEY_BYTES || !printable(&value) {
        return Err(ConfigError::InvalidMedia {
            detail: format!(
                "{CMAEG_STT_API_KEY} must be 1 to {MAX_KEY_BYTES} printable ASCII characters \
                 without whitespace"
            ),
        });
    }
    Ok(Secret::new(value))
}

fn model(value: &str) -> Result<String, ConfigError> {
    if value.is_empty() || value.len() > MAX_MODEL_BYTES || !printable(value) {
        return Err(ConfigError::InvalidMedia {
            detail: format!(
                "{CMAEG_STT_MODEL} must be 1 to {MAX_MODEL_BYTES} printable ASCII characters \
                 without whitespace"
            ),
        });
    }
    Ok(value.to_owned())
}

/// A comma list of distinct known formats; an empty entry, an unknown
/// format or a repeat fails startup.
fn formats(value: &str) -> Result<BTreeSet<&'static str>, ConfigError> {
    let mut formats = BTreeSet::new();
    for entry in value.split(',') {
        let known = KNOWN_FORMATS
            .into_iter()
            .find(|known| *known == entry)
            .ok_or_else(|| ConfigError::InvalidMedia {
                detail: format!(
                    "{CMAEG_STT_FORMATS} must list formats from {} separated by commas, \
                     without empty entries",
                    KNOWN_FORMATS.join(", ")
                ),
            })?;
        if !formats.insert(known) {
            return Err(ConfigError::InvalidMedia {
                detail: format!("{CMAEG_STT_FORMATS} lists {known} more than once"),
            });
        }
    }
    Ok(formats)
}

/// Visible ASCII: no control character, space or other whitespace.
fn printable(value: &str) -> bool {
    value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}
