use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use reqwest::header::HeaderValue;
use serde::Deserialize;
use url::Url;

use crate::model::{Check, Job, Mode, Provenance, Reason};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub environment: String,
    pub location: String,
    pub revision: String,
    pub candidate: String,
    pub runner_sha: String,
    pub state_path: PathBuf,
    pub witness_path: PathBuf,
    pub listen: SocketAddr,
    pub web: Url,
    pub issuer: Url,
    pub cma: Url,
    pub gateway: Url,
    pub owner_bearer_file: PathBuf,
    pub gateway_bearer_file: PathBuf,
    pub browser: Option<Browser>,
    pub reference: Reference,
    #[serde(default)]
    pub deterministic_enabled: bool,
    pub deterministic: Option<Targets>,
    #[serde(default)]
    pub funded_enabled: bool,
    pub funded: Option<Funded>,
    #[serde(default)]
    pub schedule_offset: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Browser {
    pub cookie_file: PathBuf,
    pub subject: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub agent: String,
    pub sandbox: String,
    pub run: String,
    pub after: u64,
    pub answer: String,
    pub channel: String,
    pub event: String,
    pub consumer: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Targets {
    pub agui_agent: String,
    pub responses_agent: String,
    pub gateway: Gateway,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gateway {
    pub channel: String,
    pub consumer: String,
    pub ingress: Url,
    pub source: Url,
    pub event_type: String,
    pub secret_file: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Funded {
    pub targets: Targets,
    pub prompt: String,
    pub budget: Budget,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub max_attempt_cost: u32,
    pub hourly_cost: u32,
    pub daily_cost: u32,
    pub hourly_attempts: u32,
    pub daily_attempts: u32,
}

pub fn bounded_read(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "expected file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file exceeds bound",
        ));
    }
    Ok(bytes)
}

pub fn secret(path: &Path) -> Result<HeaderValue, Reason> {
    let bytes = bounded_read(path, 8192).map_err(|_| Reason::CredentialMissing)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Reason::CredentialDenied)?
        .trim_end_matches(['\r', '\n']);
    if text.is_empty() || text.trim() != text {
        return Err(Reason::CredentialDenied);
    }
    let mut value = HeaderValue::from_str(text).map_err(|_| Reason::CredentialDenied)?;
    value.set_sensitive(true);
    Ok(value)
}

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b':'))
        && value != "."
        && value != ".."
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let bytes = bounded_read(path, 32 * 1024).map_err(|_| "configuration unavailable")?;
        let config: Self = serde_json::from_slice(&bytes).map_err(|_| "invalid configuration")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), &'static str> {
        if ![
            &self.environment,
            &self.location,
            &self.revision,
            &self.candidate,
        ]
        .into_iter()
        .all(|v| identifier(v))
            || !valid_sha(&self.runner_sha)
            || option_env!("CMA_JOURNEY_BUILD_SHA").is_some_and(|sha| sha != self.runner_sha)
            || self.schedule_offset > 14
            || self.state_path == self.witness_path
        {
            return Err("invalid location, revision, state paths or schedule offset");
        }
        for origin in [&self.web, &self.issuer, &self.cma, &self.gateway] {
            if !secure_url(origin) || origin.query().is_some() {
                return Err("invalid public endpoint");
            }
        }
        if [&self.issuer, &self.cma]
            .into_iter()
            .any(|u| u.path() != "/")
        {
            return Err("expected service origin");
        }
        let gateway_prefix = self
            .gateway
            .path()
            .strip_suffix('/')
            .unwrap_or(self.gateway.path());
        if !gateway_prefix.is_empty()
            && !gateway_prefix[1..].split('/').all(|segment| {
                !segment.is_empty()
                    && segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
        {
            return Err("invalid Gateway API base path");
        }
        let r = &self.reference;
        if ![&r.agent, &r.sandbox, &r.run, &r.event, &r.consumer]
            .into_iter()
            .all(|v| identifier(v))
            || !channel_id(&r.channel)
            || r.answer.trim().is_empty()
            || r.answer.len() > 512
        {
            return Err("invalid retained reference fixture");
        }
        if self
            .browser
            .as_ref()
            .is_some_and(|browser| !identifier(&browser.subject))
        {
            return Err("invalid browser subject");
        }
        if self.deterministic_enabled && self.deterministic.is_none()
            || self.funded_enabled && self.funded.is_none()
        {
            return Err("enabled schedule requires dedicated targets");
        }
        let mut agents = HashSet::from([r.agent.as_str()]);
        let mut channels = HashSet::from([r.channel.as_str()]);
        let mut ingress = HashSet::new();
        for target in self
            .deterministic
            .iter()
            .chain(self.funded.iter().map(|f| &f.targets))
        {
            let gw = &target.gateway;
            if ![
                &target.agui_agent,
                &target.responses_agent,
                &gw.consumer,
                &gw.event_type,
            ]
            .into_iter()
            .all(|v| identifier(v))
                || !channel_id(&gw.channel)
                || !agents.insert(&target.agui_agent)
                || !agents.insert(&target.responses_agent)
                || !channels.insert(&gw.channel)
                || !ingress.insert(&gw.ingress)
                || !secure_url(&gw.ingress)
                || gw.ingress.origin() != self.gateway.origin()
                || !gw
                    .ingress
                    .path()
                    .strip_prefix(gateway_prefix)
                    .is_some_and(|path| path.starts_with('/'))
                || gw.ingress.query().is_some()
                || !secure_url(&gw.source)
                || gw.source.query().is_some()
            {
                return Err("invalid or shared synthetic targets");
            }
        }
        if let Some(funded) = &self.funded {
            let b = funded.budget;
            if funded.prompt.trim().is_empty()
                || funded.prompt.len() > 512
                || b.max_attempt_cost == 0
                || b.hourly_cost < b.max_attempt_cost
                || b.daily_cost < b.hourly_cost
                || !(1..=12).contains(&b.hourly_attempts)
                || !(b.hourly_attempts..=288).contains(&b.daily_attempts)
            {
                return Err("invalid funded prompt or budget");
            }
        }
        Ok(())
    }

    pub fn provenance(&self) -> Provenance {
        Provenance {
            configuration: self.revision.clone(),
            candidate: self.candidate.clone(),
            runner_sha: self.runner_sha.clone(),
        }
    }

    pub fn domain(&self) -> String {
        format!("{}/{}", self.environment, self.location)
    }

    pub fn jobs(&self) -> Vec<(Job, bool)> {
        let base = i64::from(self.schedule_offset);
        let reads = [
            Check::Web,
            Check::Discovery,
            Check::Browser,
            Check::Agent,
            Check::AguiEvents,
            Check::GatewayEvents,
        ];
        let mut jobs: Vec<_> = reads
            .into_iter()
            .enumerate()
            .map(|(n, check)| {
                (
                    Job {
                        check,
                        mode: Mode::Read,
                        offset: base + n as i64,
                    },
                    check != Check::Browser || self.browser.is_some(),
                )
            })
            .collect();
        for (mode, enabled) in [
            (Mode::Deterministic, self.deterministic_enabled),
            (Mode::Funded, self.funded_enabled),
        ] {
            for (n, check) in [Check::Agui, Check::Responses, Check::Gateway]
                .into_iter()
                .enumerate()
            {
                jobs.push((
                    Job {
                        check,
                        mode,
                        offset: base + 35 * n as i64,
                    },
                    enabled,
                ));
            }
        }
        jobs
    }

    pub fn targets(&self, mode: Mode) -> Option<&Targets> {
        match mode {
            Mode::Read => None,
            Mode::Deterministic => self.deterministic.as_ref(),
            Mode::Funded => self.funded.as_ref().map(|v| &v.targets),
        }
    }
}

fn secure_url(url: &Url) -> bool {
    let scheme_ok = url.scheme() == "https"
        || cfg!(test) && url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    scheme_ok
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn valid_sha(sha: &str) -> bool {
    sha.len() == 40
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn channel_id(value: &str) -> bool {
    // CMAEG's sealed channel pointers have a separate public 512-byte limit.
    value.len() <= 512
        && value.strip_prefix("cmaeg1.ch.").is_some_and(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
}
