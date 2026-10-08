//! Console settings: the feature switch, the NyxID OAuth client the console
//! signs people in with, the sign-in rate and the bounds of its in-memory
//! sessions.
//!
//! Each key is read by its own function, as the media keys are, so an image
//! that predates a key simply ignores it and a newer ConfigMap never stops an
//! older image from starting. A missing client id or secret is not an error:
//! the console then fails soft with a 503 page while ingress keeps serving.
use super::{media::bounded, ConfigError};
use cmaeg_core::channel::Secret;
use std::{env, time::Duration};

pub const CMAEG_CONSOLE_ENABLED: &str = "CMAEG_CONSOLE_ENABLED";
pub const CMAEG_CONSOLE_CLIENT_ID: &str = "CMAEG_CONSOLE_CLIENT_ID";
pub const CMAEG_CONSOLE_CLIENT_SECRET: &str = "CMAEG_CONSOLE_CLIENT_SECRET";
pub const CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE: &str = "CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE";
pub const CMAEG_CONSOLE_SESSION_TTL_MS: &str = "CMAEG_CONSOLE_SESSION_TTL_MS";
pub const CMAEG_CONSOLE_SESSION_IDLE_MS: &str = "CMAEG_CONSOLE_SESSION_IDLE_MS";
pub const CMAEG_CONSOLE_SESSIONS_MAX: &str = "CMAEG_CONSOLE_SESSIONS_MAX";
pub const CMAEG_CONSOLE_SESSIONS_PER_SUBJECT: &str = "CMAEG_CONSOLE_SESSIONS_PER_SUBJECT";

/// `(default, min, max)` of each bounded key.
const LOGIN_RATE: (u64, u64, u64) = (60, 1, 6_000);
const SESSION_TTL_MS: (u64, u64, u64) = (28_800_000, 600_000, 86_400_000);
/// The idle maximum is the configured absolute TTL, checked separately.
/// Thirty minutes by default (Amendment R6 §R6.4).
const SESSION_IDLE_MS: (u64, u64) = (1_800_000, 60_000);
const SESSIONS_MAX: (u64, u64, u64) = (1_000, 10, 100_000);
const SESSIONS_PER_SUBJECT: (u64, u64, u64) = (5, 1, 50);
/// A NyxID client id is a short opaque identifier; anything longer is a
/// pasted value, not an id.
const MAX_CLIENT_ID: usize = 256;
const MAX_CLIENT_SECRET: usize = 4_096;

/// The console's settings, validated at startup even while the console is
/// off, so switching it on is never the first moment a bad value is noticed.
#[derive(Debug, Clone)]
pub struct ConsoleConfig {
    /// Off, no `/console` route exists and nothing console-related is
    /// allocated.
    pub enabled: bool,
    /// Empty leaves the console disabled with a 503 page.
    pub client_id: String,
    /// Absent leaves the console disabled with a 503 page.
    pub client_secret: Option<Secret>,
    /// Gateway-wide sign-in starts and callbacks per minute.
    pub login_rate_per_minute: u32,
    /// Absolute session lifetime.
    pub session_ttl: Duration,
    /// Session lifetime without a request.
    pub session_idle: Duration,
    pub sessions_max: usize,
    pub sessions_per_subject: usize,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            client_secret: None,
            login_rate_per_minute: narrow(LOGIN_RATE.0),
            session_ttl: Duration::from_millis(SESSION_TTL_MS.0),
            session_idle: Duration::from_millis(SESSION_IDLE_MS.0),
            sessions_max: usize::try_from(SESSIONS_MAX.0).unwrap_or(usize::MAX),
            sessions_per_subject: usize::try_from(SESSIONS_PER_SUBJECT.0).unwrap_or(usize::MAX),
        }
    }
}

impl ConsoleConfig {
    pub(super) fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(read_env)
    }

    /// Every key through `lookup`, so tests need not mutate the process
    /// environment.
    fn from_lookup(
        lookup: impl Fn(&'static str) -> Result<Option<String>, ConfigError>,
    ) -> Result<Self, ConfigError> {
        let session_ttl = session_ttl(lookup(CMAEG_CONSOLE_SESSION_TTL_MS)?.as_deref())?;
        let config = Self {
            enabled: enabled(lookup(CMAEG_CONSOLE_ENABLED)?.as_deref())?,
            client_id: client_id(lookup(CMAEG_CONSOLE_CLIENT_ID)?.as_deref())?,
            client_secret: client_secret(lookup(CMAEG_CONSOLE_CLIENT_SECRET)?)?,
            login_rate_per_minute: login_rate(
                lookup(CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE)?.as_deref(),
            )?,
            session_ttl,
            session_idle: session_idle(
                lookup(CMAEG_CONSOLE_SESSION_IDLE_MS)?.as_deref(),
                session_ttl,
            )?,
            sessions_max: sessions_max(lookup(CMAEG_CONSOLE_SESSIONS_MAX)?.as_deref())?,
            sessions_per_subject: sessions_per_subject(
                lookup(CMAEG_CONSOLE_SESSIONS_PER_SUBJECT)?.as_deref(),
            )?,
        };
        tracing::info!(
            enabled = config.enabled,
            client_configured = !config.client_id.is_empty() && config.client_secret.is_some(),
            login_rate_per_minute = config.login_rate_per_minute,
            session_ttl_ms = config.session_ttl.as_millis() as u64,
            session_idle_ms = config.session_idle.as_millis() as u64,
            sessions_max = config.sessions_max,
            sessions_per_subject = config.sessions_per_subject,
            "console settings"
        );
        Ok(config)
    }

    /// The first missing client setting, named for the operator's error log;
    /// `None` once both are configured.
    pub fn missing_client_key(&self) -> Option<&'static str> {
        if self.client_id.is_empty() {
            Some(CMAEG_CONSOLE_CLIENT_ID)
        } else if self.client_secret.is_none() {
            Some(CMAEG_CONSOLE_CLIENT_SECRET)
        } else {
            None
        }
    }
}

fn read_env(key: &'static str) -> Result<Option<String>, ConfigError> {
    match env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(invalid(format!("{key} must be valid UTF-8"))),
    }
}

fn invalid(detail: String) -> ConfigError {
    ConfigError::InvalidConsole { detail }
}

/// The media parser's grammar and message, reported as a console error.
fn read_bounded(
    key: &'static str,
    value: Option<&str>,
    (default, min, max): (u64, u64, u64),
) -> Result<u64, ConfigError> {
    bounded(key, value, default, min, max).map_err(|error| match error {
        ConfigError::InvalidMedia { detail } => invalid(detail),
        other => other,
    })
}

/// Unset is off; a set value must be exactly `true` or `false`, so a typo
/// fails startup instead of silently leaving the console off.
fn enabled(value: Option<&str>) -> Result<bool, ConfigError> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(invalid(format!(
            "{CMAEG_CONSOLE_ENABLED} must be true or false"
        ))),
    }
}

fn client_id(value: Option<&str>) -> Result<String, ConfigError> {
    let value = value.unwrap_or_default();
    if value.len() > MAX_CLIENT_ID || !value.bytes().all(|byte| (33..=126).contains(&byte)) {
        return Err(invalid(format!(
            "{CMAEG_CONSOLE_CLIENT_ID} must be at most {MAX_CLIENT_ID} printable characters without spaces"
        )));
    }
    Ok(value.to_owned())
}

/// The value is never echoed: the error names only the key.
fn client_secret(value: Option<String>) -> Result<Option<Secret>, ConfigError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > MAX_CLIENT_SECRET || !value.bytes().all(|byte| (33..=126).contains(&byte)) {
        return Err(invalid(format!(
            "{CMAEG_CONSOLE_CLIENT_SECRET} must be at most {MAX_CLIENT_SECRET} printable characters without spaces"
        )));
    }
    Ok(Some(Secret::new(value)))
}

fn login_rate(value: Option<&str>) -> Result<u32, ConfigError> {
    read_bounded(CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE, value, LOGIN_RATE).map(narrow)
}

fn session_ttl(value: Option<&str>) -> Result<Duration, ConfigError> {
    read_bounded(CMAEG_CONSOLE_SESSION_TTL_MS, value, SESSION_TTL_MS).map(Duration::from_millis)
}

fn session_idle(value: Option<&str>, ttl: Duration) -> Result<Duration, ConfigError> {
    let (default, min) = SESSION_IDLE_MS;
    let max = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
    read_bounded(
        CMAEG_CONSOLE_SESSION_IDLE_MS,
        value,
        (default.min(max), min, max),
    )
    .map(Duration::from_millis)
}

fn sessions_max(value: Option<&str>) -> Result<usize, ConfigError> {
    read_bounded(CMAEG_CONSOLE_SESSIONS_MAX, value, SESSIONS_MAX).map(widen)
}

fn sessions_per_subject(value: Option<&str>) -> Result<usize, ConfigError> {
    read_bounded(
        CMAEG_CONSOLE_SESSIONS_PER_SUBJECT,
        value,
        SESSIONS_PER_SUBJECT,
    )
    .map(widen)
}

/// The u32 key's bound is far below `u32::MAX`.
fn narrow(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// The usize keys' bounds are far below `usize::MAX` on every target.
fn widen(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
