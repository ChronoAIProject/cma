//! A local proxy that attaches the owner's credential to the agent's calls.
//!
//! The agent is pointed at this proxy instead of at the real model endpoint,
//! and is given a placeholder key. Two problems disappear as a result.
//!
//! First, the credential is never visible to agent code. A terminal attachment
//! is equivalent to shell access, so anything reachable from the environment is
//! reachable by whoever is typing; a credential held here, in the supervisor's
//! memory, is not in that blast radius.
//!
//! Second, the credential can be replaced while the agent is mid-session.
//! Tokens expire, and an agent restarted to pick up a new one loses its
//! conversation, its working directory, and any in-flight work. Swapping the
//! value behind this proxy is invisible to the process using it.

use std::{pin::Pin, sync::Arc};

/// Placeholder an agent presents to this proxy.
///
/// The proxy replaces it outright, so it is inert and is not a secret. It
/// exists because kernels refuse to start without some credential present, and
/// it lives here rather than with any kernel because it is a fact about this
/// proxy: whatever a kernel is told to send, this is what arrives.
pub const PLACEHOLDER_KEY: &str = "local-proxy-no-credential-needed";

/// Local-only response classification consumed by the runtime probe helper.
/// Provider responses are never allowed to supply this header.
pub const GATEWAY_OUTCOME_HEADER: &str = "x-cma-runtime-gateway-outcome";
pub const GATEWAY_OUTCOME_POLICY_DENIED: &str = "policy_denied";
pub const GATEWAY_OUTCOME_BROKER_TRANSIENT: &str = "broker_transient";
pub const RUNTIME_GATEWAY_PROBE_HEADER: &str = "x-cma-runtime-gateway-probe";
pub const RUNTIME_GATEWAY_PROBE_HEADER_VALUE: &str = "v1";
const RUNTIME_GATEWAY_PROBE_UPSTREAM_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(4);
const CREDENTIAL_REFLECTION_FAULT: &str =
    "the upstream response was refused because it contained protected credential material";
const ENCODED_RESPONSE_FAULT: &str =
    "the upstream response was refused because its encoded body could not be inspected";
const MALFORMED_SPONSORED_RESPONSE_FAULT: &str =
    "the sponsored model response was malformed or exceeded its size bound";
const MAX_SPONSORED_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::{Stream, StreamExt};
use std::num::NonZeroU64;
use tokio::sync::RwLock;

use crate::github_capability::{
    CapabilityRefusal, GithubCapabilityGate, MAX_BODY_BYTES as MAX_GITHUB_BODY_BYTES,
};
use crate::sponsored_model::{
    SponsoredModelGate, SponsoredModelRefusal, MAX_REQUEST_BODY_BYTES as MAX_SPONSORED_BODY_BYTES,
};

/// The current credential, replaceable while requests are in flight.
#[derive(Clone, Default)]
pub struct CredentialHolder {
    inner: Arc<RwLock<Option<CredentialSnapshot>>>,
}

#[derive(Clone, PartialEq, Eq)]
struct CredentialSnapshot {
    value: String,
    custody_generation: Option<NonZeroU64>,
}

impl CredentialSnapshot {
    fn expose(&self) -> &str {
        &self.value
    }

    fn custody_generation(&self) -> Option<NonZeroU64> {
        self.custody_generation
    }

    fn as_bytes(&self) -> &[u8] {
        self.value.as_bytes()
    }

    fn into_bytes(self) -> Vec<u8> {
        self.value.into_bytes()
    }
}

impl std::ops::Deref for CredentialSnapshot {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.expose()
    }
}

impl std::fmt::Debug for CredentialSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialSnapshot")
            .field(
                "value",
                &format_args!("<redacted {} bytes>", self.value.len()),
            )
            .field("custody_generation", &self.custody_generation)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialInstallRefusal {
    MissingGeneration,
    ZeroGeneration,
    GenerationRollback,
    GenerationConflict,
}

impl CredentialHolder {
    /// An empty holder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the credential. Takes effect on the next request.
    #[cfg(test)]
    pub async fn set(&self, credential: String) {
        self.install(credential, None, false)
            .await
            .expect("generation-less credentials remain valid outside sponsored mode");
    }

    /// Install one exact bearer/generation snapshot from the control lane.
    pub(crate) async fn install(
        &self,
        value: String,
        custody_generation: Option<u64>,
        generation_required: bool,
    ) -> Result<(), CredentialInstallRefusal> {
        let custody_generation = match custody_generation {
            Some(generation) => {
                Some(NonZeroU64::new(generation).ok_or(CredentialInstallRefusal::ZeroGeneration)?)
            }
            None if generation_required => return Err(CredentialInstallRefusal::MissingGeneration),
            None => None,
        };
        let candidate = CredentialSnapshot {
            value,
            custody_generation,
        };
        let mut installed = self.inner.write().await;
        let replaced = installed.is_some();
        match installed.as_ref() {
            None => *installed = Some(candidate),
            Some(current)
                if current.custody_generation.is_none()
                    && candidate.custody_generation.is_none() =>
            {
                *installed = Some(candidate);
            }
            Some(current) if current == &candidate => {}
            Some(current)
                if current.custody_generation.is_none()
                    && candidate.custody_generation.is_some() =>
            {
                *installed = Some(candidate);
            }
            Some(current)
                if current.custody_generation.is_some()
                    && candidate.custody_generation.is_none() =>
            {
                return Err(CredentialInstallRefusal::GenerationRollback);
            }
            Some(current) if candidate.custody_generation < current.custody_generation => {
                return Err(CredentialInstallRefusal::GenerationRollback);
            }
            Some(current)
                if candidate.custody_generation == current.custody_generation
                    && candidate.value != current.value =>
            {
                return Err(CredentialInstallRefusal::GenerationConflict);
            }
            Some(_) => *installed = Some(candidate),
        }
        // Logged as a fact, never with the value.
        tracing::info!(replaced, "upstream credential updated");
        Ok(())
    }

    /// Whether a credential is currently held.
    pub async fn is_set(&self) -> bool {
        self.inner.read().await.is_some()
    }

    async fn get(&self) -> Option<CredentialSnapshot> {
        self.inner.read().await.clone()
    }
}

/// Where the agent's calls are forwarded, and with what credential.
#[derive(Clone)]
pub struct UpstreamState {
    /// Base URL of the real endpoint, without a trailing slash.
    pub base_url: String,
    /// The credential to present.
    pub credential: CredentialHolder,
    http: reqwest::Client,
    /// Launch-scoped filter for the identity-platform proxy, absent on the
    /// model endpoint and on runtimes not provisioned for this flow.
    github_capability: Option<GithubCapabilityGate>,
    /// Inert grant gate for the model proxy. When required, this is the only
    /// source of an outbound target and `base_url` is never interpreted.
    sponsored_model: Option<SponsoredModelGate>,
    /// The last refusal the model endpoint gave, if it has given one.
    ///
    /// Recorded because an agent's own account of a failed call is unreliable
    /// and sometimes absent: a CLI that retries a 503 quietly reports nothing
    /// at all, and the owner is left watching a turn produce silence. The
    /// proxy is the one place that always knows, so it is the place that
    /// remembers — and the control plane asks it when a turn goes wrong.
    fault: UpstreamFault,
}

/// The last thing the model endpoint refused, shared with whoever reports it.
#[derive(Clone, Default)]
pub struct UpstreamFault(std::sync::Arc<std::sync::Mutex<Option<String>>>);

impl UpstreamFault {
    /// Record a refusal, replacing any earlier one: the most recent is the
    /// one that explains the turn being asked about.
    pub fn set(&self, detail: String) {
        if let Ok(mut held) = self.0.lock() {
            *held = Some(detail);
        }
    }

    /// What the model endpoint last refused with, if anything.
    pub fn get(&self) -> Option<String> {
        self.0.lock().ok().and_then(|held| held.clone())
    }
}

impl std::fmt::Debug for UpstreamFault {
    /// Never renders what it holds: an upstream body can carry a credential
    /// the caller sent, and this type exists to be logged.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UpstreamFault(..)")
    }
}

impl UpstreamState {
    /// What the model endpoint last refused with, for whoever reports it.
    pub fn fault(&self) -> UpstreamFault {
        self.fault.clone()
    }

    /// Build proxy state for an upstream.
    pub fn new(base_url: impl Into<String>, credential: CredentialHolder) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            credential,
            github_capability: None,
            sponsored_model: None,
            fault: UpstreamFault::default(),
            http: reqwest::Client::builder()
                // A redirect is a new authority decision. Following one here
                // would carry the injected credential to a target that never
                // passed the configured-prefix or GitHub capability checks.
                .redirect(reqwest::redirect::Policy::none())
                // Model responses stream for a long time; a short timeout here
                // would sever a working turn mid-answer.
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("http client builds"),
        }
    }

    /// Add the runtime's GitHub policy gate to the platform proxy.
    pub fn with_github_capability(mut self, gate: GithubCapabilityGate) -> Self {
        self.github_capability = Some(gate);
        self
    }

    /// Add the sponsored grant gate to the model proxy only.
    pub fn with_sponsored_model(mut self, gate: SponsoredModelGate) -> Self {
        self.sponsored_model = Some(gate);
        self
    }

    /// Append an untrusted request path beneath the configured service base.
    ///
    /// URL resolution semantics treat dot segments as navigation. Concatenating
    /// attacker-controlled paths therefore lets `/../../service` escape a
    /// service-scoped NyxID base before the credential-bearing request leaves
    /// this process. Validate both literal and recursively percent-encoded path
    /// segments, then let the URL parser own the final structured target.
    fn target(&self, uri: &Uri) -> Result<reqwest::Url, ()> {
        if unsafe_proxy_path(uri.path()) {
            return Err(());
        }
        let mut target = reqwest::Url::parse(&self.base_url).map_err(|_| ())?;
        if !matches!(target.scheme(), "http" | "https")
            || target.host_str().is_none()
            || !target.username().is_empty()
            || target.password().is_some()
            || target.query().is_some()
            || target.fragment().is_some()
            || unsafe_proxy_path(target.path())
        {
            return Err(());
        }
        let base_path = target.path().trim_end_matches('/').to_string();
        let joined = format!("{base_path}/{}", uri.path().trim_start_matches('/'));
        target.set_path(&joined);
        target.set_query(uri.query());
        let expected_prefix = format!("{base_path}/");
        if target.path() != base_path && !target.path().starts_with(&expected_prefix) {
            return Err(());
        }
        Ok(target)
    }

    fn target_path(&self, path_and_query: &str) -> Result<reqwest::Url, ()> {
        let uri = path_and_query.parse::<Uri>().map_err(|_| ())?;
        self.target(&uri)
    }
}

fn unsafe_proxy_path(path: &str) -> bool {
    if !path.starts_with('/') || path.as_bytes().contains(&b'\\') {
        return true;
    }
    path.split('/').any(unsafe_proxy_segment)
}

fn unsafe_proxy_segment(segment: &str) -> bool {
    let mut decoded = segment.as_bytes().to_vec();
    // Each valid decoding reduces `%xx` from three bytes to one, so this bound
    // reaches a fixed point even for recursively encoded input.
    for _ in 0..=segment.len() {
        if decoded == b"." || decoded == b".." {
            return true;
        }
        if decoded
            .iter()
            .any(|byte| matches!(byte, b'/' | b'\\' | 0..=31 | 127))
        {
            return true;
        }
        if !decoded.contains(&b'%') {
            return false;
        }
        let Some(next) = percent_decode_once(&decoded) else {
            return true;
        };
        if next == decoded {
            return false;
        }
        decoded = next;
    }
    true
}

fn percent_decode_once(value: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] != b'%' {
            decoded.push(value[index]);
            index += 1;
            continue;
        }
        let high = hex_digit(*value.get(index + 1)?)?;
        let low = hex_digit(*value.get(index + 2)?)?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    Some(decoded)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn credential_prefix_suffix_len(haystack: &[u8], credential: &[u8]) -> usize {
    let longest = haystack.len().min(credential.len().saturating_sub(1));
    (1..=longest)
        .rev()
        .find(|length| haystack.ends_with(&credential[..*length]))
        .unwrap_or(0)
}

fn response_headers_contain_credential(headers: &HeaderMap, credential: &[u8]) -> bool {
    headers.iter().any(|(name, value)| {
        contains_bytes(name.as_str().as_bytes(), credential)
            || contains_bytes(value.as_bytes(), credential)
    })
}

fn response_has_unsupported_encoding(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .any(|value| {
            value.to_str().map_or(true, |value| {
                value
                    .split(',')
                    .any(|coding| !coding.trim().eq_ignore_ascii_case("identity"))
            })
        })
}

struct CredentialGuardState<S> {
    upstream: Pin<Box<S>>,
    pending: Vec<u8>,
    credential: Vec<u8>,
    fault: UpstreamFault,
    done: bool,
}

/// Hold back enough bytes to recognize a credential split across chunks.
///
/// A streaming response cannot be recalled after it reaches the sandbox. The
/// guard therefore retains the longest possible credential prefix and emits
/// only bytes that can no longer participate in a match. A match or upstream
/// read error terminates the body with a generic error and discards the held
/// suffix, so neither the credential nor a partial candidate is released.
fn credential_guarded_stream<S, E>(
    upstream: S,
    credential: Vec<u8>,
    fault: UpstreamFault,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let state = CredentialGuardState {
        upstream: Box::pin(upstream),
        pending: Vec::new(),
        credential,
        fault,
        done: false,
    };
    futures_util::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        loop {
            match state.upstream.next().await {
                Some(Ok(chunk)) => {
                    state.pending.extend_from_slice(&chunk);
                    if contains_bytes(&state.pending, &state.credential) {
                        tracing::error!(
                            "upstream response contained protected credential material"
                        );
                        state.fault.set(CREDENTIAL_REFLECTION_FAULT.into());
                        state.pending.clear();
                        state.done = true;
                        return Some((
                            Err(std::io::Error::other(CREDENTIAL_REFLECTION_FAULT)),
                            state,
                        ));
                    }

                    let retained = credential_prefix_suffix_len(&state.pending, &state.credential);
                    let safe_len = state.pending.len().saturating_sub(retained);
                    if safe_len == 0 {
                        continue;
                    }
                    let tail = state.pending.split_off(safe_len);
                    let safe = std::mem::replace(&mut state.pending, tail);
                    return Some((Ok(Bytes::from(safe)), state));
                }
                Some(Err(_)) => {
                    state.pending.clear();
                    state.done = true;
                    return Some((
                        Err(std::io::Error::other("upstream response stream failed")),
                        state,
                    ));
                }
                None if state.pending.is_empty() => return None,
                None => {
                    state.done = true;
                    let tail = std::mem::take(&mut state.pending);
                    return Some((Ok(Bytes::from(tail)), state));
                }
            }
        }
    })
}

/// Forward one request upstream with the credential attached.
pub async fn proxy(State(state): State<UpstreamState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let runtime_gateway_probe = parts
        .headers
        .get(RUNTIME_GATEWAY_PROBE_HEADER)
        .is_some_and(|value| value == RUNTIME_GATEWAY_PROBE_HEADER_VALUE);
    let sponsored_model_required = state
        .sponsored_model
        .as_ref()
        .is_some_and(SponsoredModelGate::required);
    if sponsored_model_required && !has_exact_sponsored_json_content_type(&parts.headers) {
        return sponsored_model_refusal(SponsoredModelRefusal::MalformedRequest);
    }
    let body_limit = if sponsored_model_required {
        MAX_SPONSORED_BODY_BYTES
    } else {
        state
            .github_capability
            .as_ref()
            .filter(|gate| gate.required())
            .map(|_| MAX_GITHUB_BODY_BYTES)
            .unwrap_or(usize::MAX)
    };
    let bytes = match axum::body::to_bytes(body, body_limit).await {
        Ok(b) => b,
        Err(e) => {
            if sponsored_model_required {
                return sponsored_model_refusal(SponsoredModelRefusal::MalformedRequest);
            }
            if let Some(gate) = state
                .github_capability
                .as_ref()
                .filter(|gate| gate.required())
            {
                if let Some(refusal) = gate.refuse_unreadable_body(&parts.uri).await {
                    return capability_refusal(refusal);
                }
            }
            return (StatusCode::BAD_REQUEST, format!("unreadable body: {e}")).into_response();
        }
    };

    // Sponsored authority is evaluated first and selects the complete CMA
    // target. No rejected request reaches credential lookup or ordinary URL
    // resolution, which keeps required mode fail-closed by construction.
    let sponsored = match state.sponsored_model.as_ref() {
        Some(gate) => match gate
            .validate(&parts.method, &parts.uri, &parts.headers, &bytes)
            .await
        {
            Ok(request) => request,
            Err(refusal) => return sponsored_model_refusal(refusal),
        },
        None => None,
    };

    // Policy runs before credential lookup. A missing credential must not mask
    // a missing/revoked policy, and no rejected request may acquire a bearer.
    let canonical = match (sponsored.as_ref(), state.github_capability.as_ref()) {
        (Some(_), _) => None,
        (None, Some(gate)) => match gate
            .evaluate(
                &parts.method,
                &parts.uri,
                &bytes,
                epoch_seconds(std::time::SystemTime::now()),
            )
            .await
        {
            Ok(request) => request,
            Err(refusal) => return capability_refusal(refusal),
        },
        (None, None) => None,
    };

    let canonical_github = canonical.is_some();
    let sponsored_request = sponsored.is_some();
    let Some(credential) = state.credential.get().await else {
        // Failing loudly beats forwarding an unauthenticated call and letting
        // the agent misread the upstream's 401 as a model error.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(GATEWAY_OUTCOME_HEADER, GATEWAY_OUTCOME_BROKER_TRANSIENT)],
            "no upstream credential available yet",
        )
            .into_response();
    };

    let sponsored = match sponsored {
        Some(request) => match request.prepare(credential.custody_generation()) {
            Ok(request) => Some(request),
            Err(refusal) => return sponsored_model_refusal(refusal),
        },
        None => None,
    };

    let (target, outbound_body, sponsored_stream_response) = match sponsored {
        Some(request) => (request.target, request.body, request.stream_response),
        None => {
            let target = match canonical.as_ref() {
                Some(request) => state.target_path(&request.path_and_query),
                None => state.target(&parts.uri),
            };
            let target = match target {
                Ok(target) => target,
                Err(()) => {
                    tracing::warn!("unsafe credential-proxy target refused");
                    return (
                        StatusCode::BAD_REQUEST,
                        "request path is outside the configured upstream service",
                    )
                        .into_response();
                }
            };
            let body = canonical
                .map(|request| request.body)
                .unwrap_or_else(|| bytes.to_vec());
            (target, body, false)
        }
    };

    let mut outbound = reqwest::Request::new(parts.method.clone(), target);
    // Copy the caller's headers except the ones we own or that must not travel.
    for (name, value) in parts.headers.iter() {
        let n = name.as_str().to_ascii_lowercase();
        if n == "authorization"
            || n == "host"
            || n == "content-length"
            || n == "accept-encoding"
            || n == RUNTIME_GATEWAY_PROBE_HEADER
        {
            continue;
        }
        if canonical_github
            && !matches!(
                n.as_str(),
                "accept" | "content-type" | "user-agent" | "x-github-api-version"
            )
        {
            continue;
        }
        if sponsored_request && !matches!(n.as_str(), "accept" | "user-agent") {
            continue;
        }
        outbound.headers_mut().insert(name.clone(), value.clone());
    }
    if sponsored_request {
        outbound.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    match HeaderValue::from_str(&format!("Bearer {}", credential.expose())) {
        Ok(v) => {
            outbound
                .headers_mut()
                .insert(axum::http::header::AUTHORIZATION, v);
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("credential is not a usable header: {e}"),
            )
                .into_response()
        }
    }
    *outbound.body_mut() = Some(outbound_body.into());

    let execution = if runtime_gateway_probe {
        match tokio::time::timeout(
            RUNTIME_GATEWAY_PROBE_UPSTREAM_DEADLINE,
            state.http.execute(outbound),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("runtime gateway probe timed out at the platform broker");
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    [(GATEWAY_OUTCOME_HEADER, GATEWAY_OUTCOME_BROKER_TRANSIENT)],
                    "upstream request timed out",
                )
                    .into_response();
            }
        }
    } else {
        state.http.execute(outbound).await
    };

    match execution {
        Ok(resp) => {
            let status = resp.status();
            if response_has_unsupported_encoding(resp.headers()) {
                tracing::error!("encoded upstream response refused at the credential boundary");
                state.fault.set(ENCODED_RESPONSE_FAULT.into());
                return (StatusCode::BAD_GATEWAY, ENCODED_RESPONSE_FAULT).into_response();
            }
            if response_headers_contain_credential(resp.headers(), credential.as_bytes()) {
                tracing::error!("upstream response header contained protected credential material");
                state.fault.set(CREDENTIAL_REFLECTION_FAULT.into());
                return (StatusCode::BAD_GATEWAY, CREDENTIAL_REFLECTION_FAULT).into_response();
            }
            if status.is_client_error() || status.is_server_error() {
                // Remembered rather than only logged. This is the moment
                // CMA learns the agent's model endpoint refused it, and
                // it is the only moment: what the agent does about it — retry,
                // give up quietly, report something vaguer — is the agent's
                // business, and none of it reaches the owner on its own.
                //
                // The status and the target, never the body: an upstream's
                // error body can echo the request, and the request carries the
                // credential this proxy just attached.
                tracing::warn!(%status, "the model endpoint refused a call");
                state.fault.set(format!(
                    "the model endpoint answered {status} for {}",
                    parts.uri.path()
                ));
            }
            if sponsored_stream_response && status.is_success() {
                return sponsored_event_stream_response(resp, credential, state.fault.clone())
                    .await;
            }
            let mut builder = Response::builder().status(status);
            for (name, value) in resp.headers().iter() {
                // Re-chunked downstream, so the upstream framing headers cannot
                // be copied verbatim.
                let n = name.as_str().to_ascii_lowercase();
                if n == "content-length" || n == "transfer-encoding" || n == GATEWAY_OUTCOME_HEADER
                {
                    continue;
                }
                builder = builder.header(name, value);
            }
            let stream = credential_guarded_stream(
                resp.bytes_stream(),
                credential.into_bytes(),
                state.fault.clone(),
            );
            builder.body(Body::from_stream(stream)).unwrap_or_else(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("bad upstream response: {e}"),
                )
                    .into_response()
            })
        }
        Err(e) => {
            if runtime_gateway_probe {
                tracing::warn!("runtime gateway probe failed at the platform broker");
            } else {
                tracing::warn!(error = %e, "upstream request failed");
            }
            (
                StatusCode::BAD_GATEWAY,
                [(GATEWAY_OUTCOME_HEADER, GATEWAY_OUTCOME_BROKER_TRANSIENT)],
                "upstream request failed",
            )
                .into_response()
        }
    }
}

async fn sponsored_event_stream_response(
    response: reqwest::Response,
    credential: CredentialSnapshot,
    fault: UpstreamFault,
) -> Response {
    let status = response.status();
    let headers = response.headers().clone();
    let body = match axum::body::to_bytes(
        Body::from_stream(response.bytes_stream()),
        MAX_SPONSORED_RESPONSE_BYTES,
    )
    .await
    {
        Ok(body) => body,
        Err(_) => {
            fault.set(MALFORMED_SPONSORED_RESPONSE_FAULT.into());
            return (StatusCode::BAD_GATEWAY, MALFORMED_SPONSORED_RESPONSE_FAULT).into_response();
        }
    };
    if contains_bytes(&body, credential.as_bytes()) {
        tracing::error!("upstream response contained protected credential material");
        fault.set(CREDENTIAL_REFLECTION_FAULT.into());
        return (StatusCode::BAD_GATEWAY, CREDENTIAL_REFLECTION_FAULT).into_response();
    }
    let response_value = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(value) if value.is_object() => value,
        _ => {
            fault.set(MALFORMED_SPONSORED_RESPONSE_FAULT.into());
            return (StatusCode::BAD_GATEWAY, MALFORMED_SPONSORED_RESPONSE_FAULT).into_response();
        }
    };
    let Some(output) = response_value
        .get("output")
        .and_then(serde_json::Value::as_array)
    else {
        fault.set(MALFORMED_SPONSORED_RESPONSE_FAULT.into());
        return (StatusCode::BAD_GATEWAY, MALFORMED_SPONSORED_RESPONSE_FAULT).into_response();
    };

    let mut event_stream = Vec::with_capacity(body.len().saturating_add(1024));
    let mut sequence_number = 1_u64;
    for (output_index, item) in output.iter().enumerate() {
        let event = serde_json::json!({
            "type": "response.output_item.done",
            "sequence_number": sequence_number,
            "output_index": output_index,
            "item": item,
        });
        if append_sse_event(&mut event_stream, "response.output_item.done", &event).is_err() {
            fault.set(MALFORMED_SPONSORED_RESPONSE_FAULT.into());
            return (StatusCode::BAD_GATEWAY, MALFORMED_SPONSORED_RESPONSE_FAULT).into_response();
        }
        sequence_number = sequence_number.saturating_add(1);
    }
    let completed = serde_json::json!({
        "type": "response.completed",
        "sequence_number": sequence_number,
        "response": response_value,
    });
    if append_sse_event(&mut event_stream, "response.completed", &completed).is_err() {
        fault.set(MALFORMED_SPONSORED_RESPONSE_FAULT.into());
        return (StatusCode::BAD_GATEWAY, MALFORMED_SPONSORED_RESPONSE_FAULT).into_response();
    }
    event_stream.extend_from_slice(b"data: [DONE]\n\n");

    let mut builder = Response::builder().status(status);
    for (name, value) in &headers {
        let name_string = name.as_str().to_ascii_lowercase();
        if matches!(
            name_string.as_str(),
            "content-length" | "transfer-encoding" | "content-type" | GATEWAY_OUTCOME_HEADER
        ) {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(event_stream))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, MALFORMED_SPONSORED_RESPONSE_FAULT).into_response()
        })
}

fn append_sse_event(
    target: &mut Vec<u8>,
    event_name: &str,
    event: &serde_json::Value,
) -> Result<(), serde_json::Error> {
    target.extend_from_slice(b"event: ");
    target.extend_from_slice(event_name.as_bytes());
    target.extend_from_slice(b"\ndata: ");
    serde_json::to_writer(&mut *target, event)?;
    target.extend_from_slice(b"\n\n");
    Ok(())
}

fn has_exact_sponsored_json_content_type(headers: &HeaderMap) -> bool {
    headers.get_all(header::CONTENT_TYPE).iter().count() == 1
        && headers
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value.as_bytes() == b"application/json")
        && !headers.contains_key(header::CONTENT_ENCODING)
}

fn capability_refusal(refusal: CapabilityRefusal) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(GATEWAY_OUTCOME_HEADER, GATEWAY_OUTCOME_POLICY_DENIED)],
        Json(serde_json::json!({
            "type": "https://cma.chrono.ai/problems/github-capability-refused",
            "title": "GitHub capability refused",
            "status": 403,
            "code": refusal.wire_name(),
        })),
    )
        .into_response()
}

fn sponsored_model_refusal(refusal: SponsoredModelRefusal) -> Response {
    let (status, outcome) = match refusal {
        SponsoredModelRefusal::MissingAuthority => (
            StatusCode::SERVICE_UNAVAILABLE,
            GATEWAY_OUTCOME_BROKER_TRANSIENT,
        ),
        _ => (StatusCode::FORBIDDEN, GATEWAY_OUTCOME_POLICY_DENIED),
    };
    (
        status,
        [(GATEWAY_OUTCOME_HEADER, outcome)],
        Json(serde_json::json!({
            "type": "https://cma.chrono.ai/problems/sponsored-model-refused",
            "title": "Sponsored model request refused",
            "status": status.as_u16(),
            "code": refusal.wire_name(),
        })),
    )
        .into_response()
}

fn epoch_seconds(now: std::time::SystemTime) -> u64 {
    now.duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sponsored_authority() -> crate::sponsored_model::SponsoredModelRuntimeAuthority {
        crate::sponsored_model::SponsoredModelRuntimeAuthority {
            version: crate::sponsored_model::AUTHORITY_VERSION,
            gateway_origin: "https://cma.internal.example".into(),
            runtime_audience: "runtime-a".into(),
            session_id: "sesn_trg_123456781234123412341234567890ab".into(),
            session_credential_generation: 7,
            launch_id: "11111111-1111-4111-8111-111111111111".into(),
            grant_id: "22222222-2222-4222-8222-222222222222".into(),
            grant_generation: 3,
            policy_generation: 4,
            maximum_output_tokens_per_request: 16,
            expires_at: "2099-01-02T03:04:05Z".into(),
            capability_fingerprint: "a".repeat(64),
        }
    }

    #[tokio::test]
    async fn a_holder_starts_empty_and_accepts_a_credential() {
        let h = CredentialHolder::new();
        assert!(!h.is_set().await);
        h.set("first".into()).await;
        assert!(h.is_set().await);
        assert_eq!(h.get().await.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn replacing_a_credential_takes_effect_immediately() {
        // This is the property the whole design rests on: a refreshed token
        // must reach the next request without restarting the agent.
        let h = CredentialHolder::new();
        h.set("old".into()).await;
        h.set("new".into()).await;
        assert_eq!(h.get().await.as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn a_holder_is_shared_by_clone() {
        let a = CredentialHolder::new();
        let b = a.clone();
        a.set("shared".into()).await;
        assert_eq!(b.get().await.as_deref(), Some("shared"));
    }

    #[tokio::test]
    async fn sponsored_credentials_require_exact_forward_only_generation_pairs() {
        let holder = CredentialHolder::new();
        assert_eq!(
            holder.install("missing".into(), None, true).await,
            Err(CredentialInstallRefusal::MissingGeneration)
        );
        assert_eq!(
            holder.install("zero".into(), Some(0), true).await,
            Err(CredentialInstallRefusal::ZeroGeneration)
        );
        holder
            .install("bearer-7".into(), Some(7), true)
            .await
            .unwrap();
        holder
            .install("bearer-7".into(), Some(7), true)
            .await
            .unwrap();
        assert_eq!(
            holder.install("conflict".into(), Some(7), true).await,
            Err(CredentialInstallRefusal::GenerationConflict)
        );
        assert_eq!(
            holder.install("rollback".into(), Some(6), true).await,
            Err(CredentialInstallRefusal::GenerationRollback)
        );
        holder
            .install("bearer-8".into(), Some(8), true)
            .await
            .unwrap();
        let installed = holder.get().await.unwrap();
        assert_eq!(installed.expose(), "bearer-8");
        assert_eq!(installed.custody_generation().unwrap().get(), 8);
    }

    #[tokio::test]
    async fn rotated_bearer_snapshot_binds_the_same_generation_into_the_next_envelope() {
        let gate =
            SponsoredModelGate::new(true, "sesn_trg_123456781234123412341234567890ab".into())
                .unwrap();
        gate.install("runtime-a", sponsored_authority())
            .await
            .unwrap();
        let holder = CredentialHolder::new();
        holder
            .install("bearer-7".into(), Some(7), true)
            .await
            .unwrap();
        holder
            .install("bearer-8".into(), Some(8), true)
            .await
            .unwrap();
        let validated = gate
            .validate(
                &axum::http::Method::POST,
                &"/responses".parse().unwrap(),
                &HeaderMap::from_iter([(
                    "idempotency-key".parse().unwrap(),
                    "rotated-request".parse().unwrap(),
                )]),
                br#"{"model":"model-a","max_output_tokens":10}"#,
            )
            .await
            .unwrap()
            .unwrap();

        let credential = holder.get().await.unwrap();
        let prepared = validated.prepare(credential.custody_generation()).unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&prepared.body).unwrap();

        assert_eq!(credential.expose(), "bearer-8");
        assert_eq!(envelope["authority"]["session_credential_generation"], 8);
    }

    #[test]
    fn reflected_credentials_and_encoded_bodies_are_refused_before_streaming() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-reflected-value",
            "prefix-agent-key-suffix".parse().unwrap(),
        );
        assert!(response_headers_contain_credential(&headers, b"agent-key"));
        headers.insert("agent-key-header", "safe".parse().unwrap());
        assert!(response_headers_contain_credential(&headers, b"agent-key"));
        assert!(!response_headers_contain_credential(
            &headers,
            b"different-key"
        ));

        headers.insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
        assert!(response_has_unsupported_encoding(&headers));
        headers.insert(header::CONTENT_ENCODING, "identity".parse().unwrap());
        assert!(!response_has_unsupported_encoding(&headers));
    }

    #[tokio::test]
    async fn a_reflected_credential_split_across_chunks_never_reaches_the_sandbox() {
        let upstream = futures_util::stream::iter([
            Ok::<_, std::convert::Infallible>(Bytes::from_static(b"safe-prefix:")),
            Ok(Bytes::from_static(b"session-agent-")),
            Ok(Bytes::from_static(b"key-secret:suffix")),
        ]);
        let guarded = credential_guarded_stream(
            upstream,
            b"session-agent-key-secret".to_vec(),
            UpstreamFault::default(),
        );
        futures_util::pin_mut!(guarded);

        let mut released = Vec::new();
        let mut refused = false;
        while let Some(chunk) = guarded.next().await {
            match chunk {
                Ok(chunk) => released.extend_from_slice(&chunk),
                Err(error) => {
                    refused = true;
                    assert_eq!(error.to_string(), CREDENTIAL_REFLECTION_FAULT);
                }
            }
        }

        assert!(refused);
        assert!(!contains_bytes(&released, b"session-agent-key-secret"));
        assert_eq!(released, b"safe-prefix:");
    }

    #[tokio::test]
    async fn clean_streams_are_forwarded_byte_for_byte() {
        let upstream = futures_util::stream::iter([
            Ok::<_, std::convert::Infallible>(Bytes::from_static(b"first")),
            Ok(Bytes::from_static(b"-second")),
            Ok(Bytes::from_static(b"-third")),
        ]);
        let guarded = credential_guarded_stream(
            upstream,
            b"session-agent-key-secret".to_vec(),
            UpstreamFault::default(),
        );
        futures_util::pin_mut!(guarded);

        let mut released = Vec::new();
        while let Some(chunk) = guarded.next().await {
            released.extend_from_slice(&chunk.expect("a clean response remains readable"));
        }
        assert_eq!(released, b"first-second-third");
    }

    #[tokio::test]
    async fn buffered_sponsored_responses_become_pinned_client_compatible_sse() {
        async fn complete() -> impl IntoResponse {
            axum::Json(serde_json::json!({
                "id": "resp-1",
                "object": "response",
                "status": "completed",
                "output": [{
                    "id": "msg-1",
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": "captured answer",
                        "annotations": [],
                        "logprobs": [],
                    }],
                }],
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2,
                    "total_tokens": 6,
                },
            }))
        }

        let app = axum::Router::new().route("/response", axum::routing::get(complete));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let upstream = reqwest::get(format!("http://{address}/response"))
            .await
            .unwrap();

        let response = sponsored_event_stream_response(
            upstream,
            CredentialSnapshot {
                value: "independent-session-credential".into(),
                custody_generation: NonZeroU64::new(7),
            },
            UpstreamFault::default(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/event-stream"))
        );
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: response.output_item.done\n"));
        assert!(body.contains("\"text\":\"captured answer\""));
        assert!(body.contains("event: response.completed\n"));
        assert!(body.ends_with("data: [DONE]\n\n"));
        server.abort();
    }

    #[tokio::test]
    async fn buffered_sponsored_sse_never_reflects_the_session_credential() {
        let secret = "independent-session-credential-must-not-return";
        let app = axum::Router::new().route(
            "/response",
            axum::routing::get(move || async move {
                axum::Json(serde_json::json!({
                    "id": "resp-reflected",
                    "object": "response",
                    "status": "completed",
                    "output": [{"type": "message", "content": secret}],
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let upstream = reqwest::get(format!("http://{address}/response"))
            .await
            .unwrap();
        let fault = UpstreamFault::default();

        let response = sponsored_event_stream_response(
            upstream,
            CredentialSnapshot {
                value: secret.into(),
                custody_generation: NonZeroU64::new(7),
            },
            fault.clone(),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        assert!(!contains_bytes(&body, secret.as_bytes()));
        assert_eq!(fault.get().as_deref(), Some(CREDENTIAL_REFLECTION_FAULT));
        server.abort();
    }

    #[tokio::test]
    async fn github_policy_denial_precedes_credential_lookup_in_the_proxy() {
        use base64::Engine as _;

        let keyring = serde_json::json!({
            "key-1": base64::engine::general_purpose::STANDARD.encode([0_u8; 32])
        })
        .to_string();
        let gate = GithubCapabilityGate::from_configuration(
            true,
            "sesn_trg_123456781234123412341234567890ab",
            vec![crate::github_capability::GITHUB_SERVICE_SLUG.into()],
            &keyring,
        )
        .expect("the missing-policy gate is configured");
        let state = UpstreamState::new("http://127.0.0.1:1", CredentialHolder::new())
            .with_github_capability(gate);
        let request = Request::builder()
            .uri("/api/v1/proxy/catalog-service-github/repositories/30?_nyxid_via=service-1")
            .body(Body::empty())
            .unwrap();

        let response = proxy(State(state), request).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers().get(GATEWAY_OUTCOME_HEADER),
            Some(&HeaderValue::from_static(GATEWAY_OUTCOME_POLICY_DENIED))
        );
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("missing_policy"));
    }

    #[tokio::test]
    async fn sponsored_request_before_authority_install_makes_zero_outbound_calls() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        async fn observed(State(calls): State<Arc<AtomicUsize>>) -> Response {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::OK.into_response()
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new()
            .fallback(axum::routing::any(observed))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let credential = CredentialHolder::new();
        credential
            .set("independent-session-credential".into())
            .await;
        let gate =
            SponsoredModelGate::new(true, "sesn_trg_123456781234123412341234567890ab".into())
                .unwrap();
        let state =
            UpstreamState::new(format!("http://{address}"), credential).with_sponsored_model(gate);
        let request = Request::builder()
            .method(axum::http::Method::POST)
            .uri("/responses")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"model":"model-a","max_output_tokens":10}"#))
            .unwrap();

        let response = proxy(State(state), request).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(GATEWAY_OUTCOME_HEADER),
            Some(&HeaderValue::from_static(GATEWAY_OUTCOME_BROKER_TRANSIENT))
        );
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("missing_authority"));
        tokio::task::yield_now().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "neither the configured ordinary upstream nor any provider target is dispatched"
        );
        server.abort();
    }

    #[tokio::test]
    async fn required_sponsored_mode_never_falls_back_to_the_ordinary_upstream() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        async fn observed(State(calls): State<Arc<AtomicUsize>>) -> Response {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::OK.into_response()
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new()
            .fallback(axum::routing::any(observed))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let credential = CredentialHolder::new();
        credential
            .set("independent-session-credential".into())
            .await;
        let gate =
            SponsoredModelGate::new(true, "sesn_trg_123456781234123412341234567890ab".into())
                .unwrap();
        gate.install("runtime-a", sponsored_authority())
            .await
            .unwrap();
        let state =
            UpstreamState::new(format!("http://{address}"), credential).with_sponsored_model(gate);
        let request = Request::builder()
            .method(axum::http::Method::POST)
            .uri("/v1/chat/completions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"model":"model-a","max_output_tokens":10}"#))
            .unwrap();

        let response = proxy(State(state), request).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("unsupported_request"));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn sponsored_media_type_encoding_and_size_are_refused_before_credential_lookup() {
        let gate =
            SponsoredModelGate::new(true, "sesn_trg_123456781234123412341234567890ab".into())
                .unwrap();
        let state = UpstreamState::new("http://127.0.0.1:1", CredentialHolder::new())
            .with_sponsored_model(gate);
        let bodies = [
            Request::builder()
                .method(axum::http::Method::POST)
                .uri("/responses")
                .body(Body::from("{}"))
                .unwrap(),
            Request::builder()
                .method(axum::http::Method::POST)
                .uri("/responses")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .body(Body::from("{}"))
                .unwrap(),
            Request::builder()
                .method(axum::http::Method::POST)
                .uri("/responses")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_ENCODING, "gzip")
                .body(Body::from("{}"))
                .unwrap(),
            Request::builder()
                .method(axum::http::Method::POST)
                .uri("/responses")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b'x'; MAX_SPONSORED_BODY_BYTES + 1]))
                .unwrap(),
        ];

        for request in bodies {
            let response = proxy(State(state.clone()), request).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
                .await
                .unwrap();
            assert!(String::from_utf8_lossy(&body).contains("malformed_request"));
        }
    }

    #[tokio::test]
    async fn oversized_github_body_is_typed_before_credential_lookup() {
        use base64::Engine as _;

        let keyring = serde_json::json!({
            "key-1": base64::engine::general_purpose::STANDARD.encode([0_u8; 32])
        })
        .to_string();
        let gate = GithubCapabilityGate::from_configuration(
            true,
            "sesn_trg_123456781234123412341234567890ab",
            vec![crate::github_capability::GITHUB_SERVICE_SLUG.into()],
            &keyring,
        )
        .expect("the capability gate is configured");
        let observed = gate.clone();
        let state = UpstreamState::new("http://127.0.0.1:1", CredentialHolder::new())
            .with_github_capability(gate);
        let request = Request::builder()
            .method(axum::http::Method::POST)
            .uri("/api/v1/proxy/catalog-service-github/repos/contributor/project/git/blobs?_nyxid_via=service-1")
            .body(Body::from(vec![0_u8; MAX_GITHUB_BODY_BYTES + 1]))
            .unwrap();

        let response = proxy(State(state), request).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers().get(GATEWAY_OUTCOME_HEADER),
            Some(&HeaderValue::from_static(GATEWAY_OUTCOME_POLICY_DENIED))
        );
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("malformed_request"));
        assert_eq!(
            observed.snapshot().await.last_refusal,
            Some("malformed_request")
        );
    }

    #[tokio::test]
    async fn an_upstream_header_cannot_reflect_the_injected_credential() {
        async fn reflect(headers: HeaderMap) -> Response {
            let reflected = headers
                .get(header::AUTHORIZATION)
                .expect("the proxy injected its credential")
                .clone();
            Response::builder()
                .status(StatusCode::OK)
                .header("x-reflected-authorization", reflected)
                .body(Body::from("ordinary upstream body"))
                .unwrap()
        }

        let app = axum::Router::new().fallback(axum::routing::any(reflect));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let secret = "session-agent-key-reflected-in-a-header";
        let credential = CredentialHolder::new();
        credential.set(secret.into()).await;
        let state = UpstreamState::new(format!("http://{address}"), credential);
        let fault = state.fault();
        let request = Request::builder()
            .uri("/reflect-header")
            .body(Body::empty())
            .unwrap();

        let response = proxy(State(state), request).await;

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(response
            .headers()
            .get("x-reflected-authorization")
            .is_none());
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        assert!(!contains_bytes(&body, secret.as_bytes()));
        assert_eq!(fault.get().as_deref(), Some(CREDENTIAL_REFLECTION_FAULT));
        server.abort();
    }

    #[tokio::test]
    async fn a_streamed_upstream_body_cannot_reflect_the_injected_credential() {
        async fn reflect(headers: HeaderMap) -> Response {
            let authorization = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .expect("the proxy injected its bearer")
                .to_string();
            let split = authorization.len() / 2;
            let chunks = futures_util::stream::iter([
                Ok::<_, std::convert::Infallible>(Bytes::from(format!(
                    "safe-prefix:{}",
                    &authorization[..split]
                ))),
                Ok(Bytes::from(authorization[split..].to_string())),
            ]);
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from_stream(chunks))
                .unwrap()
        }

        let app = axum::Router::new().fallback(axum::routing::any(reflect));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let secret = "session-agent-key-reflected-across-body-chunks";
        let credential = CredentialHolder::new();
        credential.set(secret.into()).await;
        let state = UpstreamState::new(format!("http://{address}"), credential);
        let fault = state.fault();
        let request = Request::builder()
            .uri("/reflect-body")
            .body(Body::empty())
            .unwrap();

        let response = proxy(State(state), request).await;
        let mut body = response.into_body().into_data_stream();
        let mut released = Vec::new();
        let mut refused = false;
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(chunk) => released.extend_from_slice(&chunk),
                Err(_) => refused = true,
            }
        }

        assert!(refused);
        assert!(!contains_bytes(&released, secret.as_bytes()));
        assert_eq!(fault.get().as_deref(), Some(CREDENTIAL_REFLECTION_FAULT));
        server.abort();
    }

    #[tokio::test]
    async fn an_upstream_redirect_is_not_followed_with_the_injected_credential() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        async fn redirect() -> Response {
            Response::builder()
                .status(StatusCode::TEMPORARY_REDIRECT)
                .header(header::LOCATION, "/outside-service-prefix")
                .body(Body::empty())
                .unwrap()
        }

        async fn outside(State(hits): State<Arc<AtomicUsize>>, headers: HeaderMap) -> Response {
            hits.fetch_add(1, Ordering::SeqCst);
            let reflected = headers
                .get(header::AUTHORIZATION)
                .cloned()
                .unwrap_or_else(|| HeaderValue::from_static("missing"));
            Response::builder()
                .status(StatusCode::OK)
                .header("x-reflected-authorization", reflected)
                .body(Body::from("redirect followed"))
                .unwrap()
        }

        let outside_hits = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new()
            .route("/scoped/start", axum::routing::any(redirect))
            .route("/outside-service-prefix", axum::routing::any(outside))
            .with_state(Arc::clone(&outside_hits));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let credential = CredentialHolder::new();
        credential
            .set("session-agent-key-must-stay-in-scope".into())
            .await;
        let state = UpstreamState::new(format!("http://{address}/scoped"), credential);
        let request = Request::builder()
            .uri("/start")
            .body(Body::empty())
            .unwrap();

        let response = proxy(State(state), request).await;

        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            response.headers().get(header::LOCATION),
            Some(&HeaderValue::from_static("/outside-service-prefix"))
        );
        assert_eq!(outside_hits.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[test]
    fn the_target_keeps_path_and_query() {
        let s = UpstreamState::new("http://upstream/base/", CredentialHolder::new());
        let uri: Uri = "/v1/chat/completions?stream=true".parse().expect("uri");
        assert_eq!(
            s.target(&uri).unwrap().as_str(),
            "http://upstream/base/v1/chat/completions?stream=true"
        );
    }

    #[test]
    fn a_refusal_from_the_model_endpoint_is_remembered() {
        // The agent's own account of a failed model call is unreliable and
        // sometimes absent — a CLI that retries a 503 quietly reports nothing
        // at all, and the owner watches a turn produce silence. The proxy is
        // the one thing that always learns of the refusal.
        let fault = UpstreamFault::default();
        assert_eq!(fault.get(), None, "nothing has been refused yet");

        fault.set("the model endpoint answered 503 for /responses".into());
        assert_eq!(
            fault.get().as_deref(),
            Some("the model endpoint answered 503 for /responses")
        );

        // The most recent refusal is the one that explains the turn being
        // asked about.
        fault.set("the model endpoint answered 401 for /responses".into());
        assert_eq!(
            fault.get().as_deref(),
            Some("the model endpoint answered 401 for /responses")
        );
    }

    #[test]
    fn a_refusal_is_never_rendered_by_debug() {
        // It is logged, and an upstream error body can echo the request that
        // produced it — which carries the credential this proxy attached.
        let fault = UpstreamFault::default();
        fault.set("swordfish".into());
        assert!(!format!("{fault:?}").contains("swordfish"));
    }

    #[test]
    fn the_target_handles_a_bare_path() {
        let s = UpstreamState::new("http://upstream", CredentialHolder::new());
        let uri: Uri = "/v1/models".parse().expect("uri");
        assert_eq!(
            s.target(&uri).unwrap().as_str(),
            "http://upstream/v1/models"
        );
    }

    #[test]
    fn model_proxy_targets_cannot_escape_the_configured_service_prefix() {
        let state = UpstreamState::new(
            "https://nyx.example/api/v1/proxy/s/chrono-llm",
            CredentialHolder::new(),
        );
        let ordinary: Uri = "/v1/responses?stream=true".parse().unwrap();
        let target = state.target(&ordinary).unwrap();
        assert_eq!(target.host_str(), Some("nyx.example"));
        assert_eq!(target.path(), "/api/v1/proxy/s/chrono-llm/v1/responses");
        assert_eq!(target.query(), Some("stream=true"));

        for path in [
            "/../../../../../api/v1/proxy/s/api-github/repositories/30",
            "/%2e%2e/%2E%2E/api/v1/proxy/s/api-github/repositories/30",
            "/%252e%252e/%252e%252e/api-github/repositories/30",
            "/.%2e/%2e./api-github/repositories/30",
            "/safe/%2f..%2fapi-github/repositories/30",
            "/safe\\..\\api-github/repositories/30",
            "/safe/%zz/api-github/repositories/30",
        ] {
            let uri: Uri = path
                .parse()
                .expect("the HTTP layer accepts the hostile URI");
            assert!(state.target(&uri).is_err(), "accepted hostile path {path}");
        }
    }

    #[tokio::test]
    async fn traversal_is_refused_before_the_model_proxy_uses_its_agent_key() {
        let credential = CredentialHolder::new();
        credential
            .set("session-agent-key-must-not-be-forwarded".into())
            .await;
        let state = UpstreamState::new("http://127.0.0.1:1/api/v1/proxy/s/chrono-llm", credential);
        for path in [
            "/../../../../../api/v1/proxy/s/api-github/repositories/30",
            "/%2e%2e/%2e%2e/api/v1/proxy/s/api-github/repositories/30",
        ] {
            let request = Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("the exploit request is representable");
            let response = proxy(State(state.clone()), request).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn authority_form_connect_cannot_turn_the_credential_proxy_into_a_tunnel() {
        let credential = CredentialHolder::new();
        credential
            .set("session-agent-key-must-not-be-forwarded".into())
            .await;
        let state = UpstreamState::new("http://127.0.0.1:1/model", credential);
        let request = Request::builder()
            .method(axum::http::Method::CONNECT)
            .uri("127.0.0.1:9091")
            .body(Body::empty())
            .expect("an authority-form CONNECT request is representable");

        let response = proxy(State(state), request).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_request_with_no_credential_is_refused_rather_than_forwarded() {
        let state = UpstreamState::new("http://upstream", CredentialHolder::new());
        let req = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("request");
        let resp = proxy(State(state), req).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn the_credential_is_never_rendered_by_debug() {
        // A holder carries a live credential; no Debug impl may print it.
        let h = CredentialHolder::new();
        let rendered = format!("{:?}", std::mem::size_of_val(&h));
        assert!(!rendered.contains("secret"));
    }
}
