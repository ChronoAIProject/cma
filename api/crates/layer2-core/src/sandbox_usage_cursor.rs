//! Service-issued continuations for live allocation-usage pages, not snapshots.
use crate::sandbox::store::{PageRequest, SandboxStoreError};
use crate::{Caller, SandboxId, SubjectId};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::hmac;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const TTL: u64 = 15 * 60;
const MAX_CURSOR: usize = 4096;

/// Reader support is unconditional. Issuance starts new signed chains; already
/// signed chains keep their original expiry even on reader-only replicas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsageCursorPolicy {
    #[default]
    ReaderOnly,
    IssueSigned,
    Strict,
}

pub struct UsageCursors {
    key: hmac::Key,
    policy: UsageCursorPolicy,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

pub(crate) struct Scope<'a> {
    pub caller: &'a Caller,
    pub admin: bool,
    pub owner: Option<&'a SubjectId>,
    pub sandbox: Option<&'a SandboxId>,
    pub filter_owner: Option<&'a SubjectId>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    version: u8,
    scope: String,
    last: String,
    issued_at: u64,
    expires_at: u64,
}

pub(crate) struct Continuation {
    pub page: PageRequest,
    lifetime: Option<(u64, u64)>,
}

#[derive(Debug)]
pub(crate) enum CursorError {
    Invalid,
    Store(SandboxStoreError),
}

impl From<SandboxStoreError> for CursorError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Store(error)
    }
}

impl UsageCursors {
    /// The caller supplies a stable, deployment-scoped, Usage-specific key.
    pub fn new(key: &[u8; 32], policy: UsageCursorPolicy) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, key),
            policy,
            clock: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            }),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    fn scope(&self, scope: &Scope<'_>) -> String {
        let (endpoint, order) = if scope.sandbox.is_some() {
            ("events", "revision:asc")
        } else {
            ("list", "sandbox_id:asc")
        };
        let mut mac = hmac::Context::with_key(&self.key);
        mac.update(b"usage-continuation-scope-v1\0");
        for part in [
            Some(scope.caller.subject.as_str()),
            Some(if scope.admin { "admin" } else { "owner" }),
            scope.owner.map(SubjectId::as_str),
            Some(endpoint),
            scope.sandbox.map(SandboxId::as_str),
            Some(order),
            scope.filter_owner.map(SubjectId::as_str),
        ] {
            mac.update(&[u8::from(part.is_some())]);
            let bytes = part.unwrap_or_default().as_bytes();
            mac.update(&(bytes.len() as u64).to_be_bytes());
            mac.update(bytes);
        }
        URL_SAFE_NO_PAD.encode(mac.sign().as_ref())
    }

    pub(crate) fn begin(
        &self,
        scope: &Scope<'_>,
        page: &PageRequest,
    ) -> Result<Continuation, CursorError> {
        PageRequest::new(page.limit, None)?;
        if page
            .cursor
            .as_ref()
            .is_some_and(|token| token.len() > MAX_CURSOR)
        {
            return Err(invalid());
        }
        let now = (self.clock)();
        let mut last = page.cursor.clone();
        let mut lifetime = None;
        if let Some(token) = &page.cursor {
            if token.starts_with("u1.") {
                let (body, tag) = token[3..].split_once('.').ok_or_else(invalid)?;
                let body = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
                let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| invalid())?;
                let mut authenticated = b"u1.".to_vec();
                authenticated.extend_from_slice(&body);
                hmac::verify(&self.key, &authenticated, &tag).map_err(|_| invalid())?;
                let payload: Payload = serde_json::from_slice(&body).map_err(|_| invalid())?;
                if payload.version != 1
                    || payload.scope != self.scope(scope)
                    || serde_json::to_vec(&payload).map_err(|_| invalid())? != body
                {
                    return Err(invalid());
                }
                validate_lifetime(payload.issued_at, payload.expires_at, now)?;
                lifetime = Some((payload.issued_at, payload.expires_at));
                validate_last(&payload.last, scope.sandbox.is_some()).map_err(|_| invalid())?;
                last = Some(payload.last);
            } else if self.policy == UsageCursorPolicy::Strict || reserved_version(token) {
                return Err(invalid());
            }
            validate_last(last.as_deref().unwrap(), scope.sandbox.is_some())?;
        }
        if lifetime.is_none() && self.policy != UsageCursorPolicy::ReaderOnly {
            let expires = now.checked_add(TTL).ok_or_else(invalid)?;
            validate_lifetime(now, expires, now)?;
            lifetime = Some((now, expires));
        }
        Ok(Continuation {
            page: PageRequest {
                limit: page.limit,
                cursor: last,
            },
            lifetime,
        })
    }

    pub(crate) fn finish(
        &self,
        scope: &Scope<'_>,
        chain: Continuation,
        next: Option<String>,
    ) -> Result<Option<String>, CursorError> {
        let Some(last) = next else { return Ok(None) };
        validate_last(&last, scope.sandbox.is_some()).map_err(|_| {
            SandboxStoreError::Corrupt("invalid usage continuation from store".into())
        })?;
        let Some((issued_at, expires_at)) = chain.lifetime else {
            return Ok(Some(last));
        };
        validate_lifetime(issued_at, expires_at, (self.clock)())?;
        let payload = Payload {
            version: 1,
            scope: self.scope(scope),
            last,
            issued_at,
            expires_at,
        };
        let body = serde_json::to_vec(&payload).map_err(|_| invalid())?;
        let mut authenticated = b"u1.".to_vec();
        authenticated.extend_from_slice(&body);
        let tag = hmac::sign(&self.key, &authenticated);
        Ok(Some(format!(
            "u1.{}.{}",
            URL_SAFE_NO_PAD.encode(body),
            URL_SAFE_NO_PAD.encode(tag.as_ref())
        )))
    }
}

fn reserved_version(token: &str) -> bool {
    token.starts_with("u1")
        || token.strip_prefix('u').is_some_and(|tail| {
            tail.split('.').next().is_some_and(|version| {
                !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit())
            })
        })
}

fn validate_last(last: &str, events: bool) -> Result<(), SandboxStoreError> {
    if last.is_empty()
        || last.len() > 256
        || (events
            && last
                .parse::<i64>()
                .ok()
                .is_none_or(|revision| revision <= 0))
    {
        return Err(SandboxStoreError::InvalidPage);
    }
    Ok(())
}

fn validate_lifetime(issued: u64, expires: u64, now: u64) -> Result<(), CursorError> {
    if issued == 0 || issued > now || issued.checked_add(TTL) != Some(expires) || now >= expires {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> CursorError {
    CursorError::Invalid
}

#[cfg(test)]
mod tests;
