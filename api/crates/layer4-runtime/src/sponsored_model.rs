//! Sponsored-model authority held only by the runtime supervisor.

use std::num::NonZeroU64;
use std::sync::Arc;

use axum::http::{HeaderMap, Method, Uri};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

pub(crate) const GATEWAY_PATH: &str = "/internal/v1/sponsored-model/responses";
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const MAX_GATEWAY_ENVELOPE_BYTES: usize = MAX_REQUEST_BODY_BYTES + 64 * 1024;
pub(crate) const AUTHORITY_VERSION: u32 = 2;
const GATEWAY_REQUEST_VERSION: u32 = 2;
const SPONSORED_MODEL_RESOURCE: &str = "responses";
const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SponsoredModelRuntimeAuthority {
    pub version: u32,
    pub gateway_origin: String,
    pub runtime_audience: String,
    pub session_id: String,
    pub session_credential_generation: u64,
    pub launch_id: String,
    pub grant_id: String,
    pub grant_generation: u64,
    pub policy_generation: u64,
    pub maximum_output_tokens_per_request: u64,
    pub expires_at: String,
    pub capability_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SponsoredModelAuthorityReceipt {
    pub runtime_audience: String,
    pub session_id: String,
    pub session_credential_generation: u64,
    pub launch_id: String,
    pub grant_id: String,
    pub grant_generation: u64,
    pub policy_generation: u64,
    pub maximum_output_tokens_per_request: u64,
    pub capability_fingerprint: String,
}

impl From<&SponsoredModelRuntimeAuthority> for SponsoredModelAuthorityReceipt {
    fn from(authority: &SponsoredModelRuntimeAuthority) -> Self {
        Self {
            runtime_audience: authority.runtime_audience.clone(),
            session_id: authority.session_id.clone(),
            session_credential_generation: authority.session_credential_generation,
            launch_id: authority.launch_id.clone(),
            grant_id: authority.grant_id.clone(),
            grant_generation: authority.grant_generation,
            policy_generation: authority.policy_generation,
            maximum_output_tokens_per_request: authority.maximum_output_tokens_per_request,
            capability_fingerprint: authority.capability_fingerprint.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SponsoredModelRefusal {
    MissingAuthority,
    NotRequired,
    AudienceMismatch,
    AuthorityChanged,
    ExpiredAuthority,
    MalformedAuthority,
    CredentialGenerationMismatch,
    UnsupportedRequest,
    MalformedRequest,
}

impl SponsoredModelRefusal {
    pub(crate) fn wire_name(self) -> &'static str {
        match self {
            Self::MissingAuthority => "missing_authority",
            Self::NotRequired => "not_required",
            Self::AudienceMismatch => "audience_mismatch",
            Self::AuthorityChanged => "authority_changed",
            Self::ExpiredAuthority => "expired_authority",
            Self::MalformedAuthority => "malformed_authority",
            Self::CredentialGenerationMismatch => "credential_generation_mismatch",
            Self::UnsupportedRequest => "unsupported_request",
            Self::MalformedRequest => "malformed_request",
        }
    }
}

#[derive(Clone)]
pub(crate) struct SponsoredModelGate {
    required: bool,
    session_id: String,
    authority: Arc<RwLock<Option<SponsoredModelRuntimeAuthority>>>,
}

impl SponsoredModelGate {
    pub(crate) fn new(required: bool, session_id: String) -> Result<Self, &'static str> {
        if required && !valid_identifier(&session_id, 255) {
            return Err("a sponsored model runtime requires its exact session id");
        }
        Ok(Self {
            required,
            session_id,
            authority: Arc::new(RwLock::new(None)),
        })
    }

    pub(crate) fn required(&self) -> bool {
        self.required
    }

    pub(crate) async fn install(
        &self,
        runtime: &str,
        authority: SponsoredModelRuntimeAuthority,
    ) -> Result<SponsoredModelAuthorityReceipt, SponsoredModelRefusal> {
        if !self.required {
            return Err(SponsoredModelRefusal::NotRequired);
        }
        validate_authority(&authority)?;
        require_live_authority(&authority)?;
        if authority.runtime_audience != runtime || authority.session_id != self.session_id {
            return Err(SponsoredModelRefusal::AudienceMismatch);
        }
        let mut installed = self.authority.write().await;
        match installed.as_ref() {
            None => *installed = Some(authority.clone()),
            Some(current) if current == &authority => {}
            Some(current)
                if same_grant_authority(current, &authority)
                    && authority.session_credential_generation
                        > current.session_credential_generation =>
            {
                *installed = Some(authority.clone());
            }
            Some(_) => return Err(SponsoredModelRefusal::AuthorityChanged),
        }
        Ok((&authority).into())
    }

    pub(crate) async fn validate(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<Option<ValidatedSponsoredModelRequest>, SponsoredModelRefusal> {
        if !self.required {
            return Ok(None);
        }
        let authority = self
            .authority
            .read()
            .await
            .clone()
            .ok_or(SponsoredModelRefusal::MissingAuthority)?;
        require_live_authority(&authority)?;
        if method != Method::POST
            || uri.query().is_some()
            || !matches!(uri.path(), "/responses" | "/v1/responses")
        {
            return Err(SponsoredModelRefusal::UnsupportedRequest);
        }
        let mut request: serde_json::Value =
            serde_json::from_slice(body).map_err(|_| SponsoredModelRefusal::MalformedRequest)?;
        let object = request
            .as_object_mut()
            .ok_or(SponsoredModelRefusal::MalformedRequest)?;
        let stream_response = match object.get("stream") {
            Some(value) if value == true => true,
            Some(value) if value == false => false,
            Some(_) => return Err(SponsoredModelRefusal::MalformedRequest),
            None => false,
        };
        object.insert("stream".into(), serde_json::Value::Bool(false));
        let model = object
            .get("model")
            .and_then(serde_json::Value::as_str)
            .filter(|model| valid_policy_token(model))
            .ok_or(SponsoredModelRefusal::MalformedRequest)?
            .to_string();
        let maximum_output_tokens = match object.get("max_output_tokens") {
            Some(value) => value
                .as_u64()
                .filter(|value| *value > 0)
                .map(|value| value.min(authority.maximum_output_tokens_per_request))
                .ok_or(SponsoredModelRefusal::MalformedRequest)?,
            None => authority.maximum_output_tokens_per_request,
        };
        object.insert(
            "max_output_tokens".into(),
            serde_json::Value::from(maximum_output_tokens),
        );
        let canonical_request = serde_json::to_vec(&canonical_json(&request))
            .map_err(|_| SponsoredModelRefusal::MalformedRequest)?;
        let request_digest = lower_hex(Sha256::digest(&canonical_request));
        let stable_request_id = stable_request_id(headers, &authority, &request, &request_digest)?;
        Ok(Some(ValidatedSponsoredModelRequest {
            authority,
            stable_request_id,
            request_digest,
            maximum_quota: SponsoredModelQuota {
                requests: 1,
                input_tokens: u64::try_from(canonical_request.len())
                    .map_err(|_| SponsoredModelRefusal::MalformedRequest)?
                    .max(1),
                output_tokens: maximum_output_tokens,
            },
            model,
            request,
            stream_response,
        }))
    }

    #[cfg(test)]
    async fn prepare(
        &self,
        method: &Method,
        uri: &Uri,
        body: &[u8],
    ) -> Result<Option<PreparedSponsoredModelRequest>, SponsoredModelRefusal> {
        let mut headers = HeaderMap::new();
        headers.insert(
            IDEMPOTENCY_KEY_HEADER,
            "sponsored-model-test-request".parse().unwrap(),
        );
        match self.validate(method, uri, &headers, body).await? {
            Some(request) => {
                let generation = NonZeroU64::new(request.authority.session_credential_generation);
                request.prepare(generation).map(Some)
            }
            None => Ok(None),
        }
    }
}

fn same_grant_authority(
    current: &SponsoredModelRuntimeAuthority,
    candidate: &SponsoredModelRuntimeAuthority,
) -> bool {
    let mut candidate = candidate.clone();
    candidate.session_credential_generation = current.session_credential_generation;
    &candidate == current
}

fn validate_authority(
    authority: &SponsoredModelRuntimeAuthority,
) -> Result<(), SponsoredModelRefusal> {
    if authority.version != AUTHORITY_VERSION
        || authority.session_credential_generation == 0
        || authority.grant_generation == 0
        || authority.policy_generation == 0
        || authority.maximum_output_tokens_per_request == 0
        || authority.maximum_output_tokens_per_request > u64::from(u32::MAX)
        || !valid_identifier(&authority.runtime_audience, 512)
        || !valid_identifier(&authority.session_id, 255)
        || !valid_identifier(&authority.launch_id, 128)
        || !valid_identifier(&authority.grant_id, 128)
        || expiration_epoch_seconds(&authority.expires_at).is_none()
        || !valid_digest(&authority.capability_fingerprint)
    {
        return Err(SponsoredModelRefusal::MalformedAuthority);
    }
    gateway_target(&authority.gateway_origin)?;
    Ok(())
}

fn require_live_authority(
    authority: &SponsoredModelRuntimeAuthority,
) -> Result<(), SponsoredModelRefusal> {
    let expires_at = expiration_epoch_seconds(&authority.expires_at)
        .ok_or(SponsoredModelRefusal::MalformedAuthority)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if expires_at <= now {
        return Err(SponsoredModelRefusal::ExpiredAuthority);
    }
    Ok(())
}

fn expiration_epoch_seconds(stamp: &str) -> Option<u64> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    fn number(bytes: &[u8]) -> Option<u32> {
        bytes.iter().try_fold(0_u32, |value, digit| {
            digit
                .is_ascii_digit()
                .then(|| value * 10 + u32::from(*digit - b'0'))
        })
    }
    let year = i64::from(number(&bytes[0..4])?);
    let month = number(&bytes[5..7])?;
    let day = number(&bytes[8..10])?;
    let hour = number(&bytes[11..13])?;
    let minute = number(&bytes[14..16])?;
    let second = number(&bytes[17..19])?;
    if !(1..=12).contains(&month) || day == 0 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 || civil_from_days(days) != (year, month, day) {
        return None;
    }
    u64::try_from(days)
        .ok()?
        .checked_mul(86_400)?
        .checked_add(u64::from(hour) * 3_600)?
        .checked_add(u64::from(minute) * 60)?
        .checked_add(u64::from(second))
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_piece = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_piece + 2) / 5 + 1) as u32;
    let month = if month_piece < 10 {
        month_piece + 3
    } else {
        month_piece - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_piece = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_piece + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn gateway_target(origin: &str) -> Result<reqwest::Url, SponsoredModelRefusal> {
    let parsed =
        reqwest::Url::parse(origin).map_err(|_| SponsoredModelRefusal::MalformedAuthority)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.origin().ascii_serialization() != origin
    {
        return Err(SponsoredModelRefusal::MalformedAuthority);
    }
    let mut target = parsed;
    target.set_path(GATEWAY_PATH);
    Ok(target)
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && !value.chars().any(char::is_control)
        && value.trim() == value
}

fn valid_policy_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
}

fn stable_request_id(
    headers: &HeaderMap,
    authority: &SponsoredModelRuntimeAuthority,
    request: &serde_json::Value,
    request_digest: &str,
) -> Result<String, SponsoredModelRefusal> {
    let mut keys = headers.get_all(IDEMPOTENCY_KEY_HEADER).iter();
    match (keys.next(), keys.next()) {
        (Some(key), None) => {
            let key = visible_ascii(key.as_bytes(), 255)?;
            let identity =
                scoped_request_identity("cma-sponsored-model-explicit-request-v1", authority, key)?;
            Ok(format!("smr-e-{identity}"))
        }
        (None, None) => {
            let logical_request_id = request
                .get("client_metadata")
                .and_then(serde_json::Value::as_object)
                .and_then(|metadata| metadata.get("turn_id"))
                .and_then(serde_json::Value::as_str)
                .ok_or(SponsoredModelRefusal::MalformedRequest)?;
            let logical_request_id = visible_ascii(logical_request_id.as_bytes(), 255)?;
            let identity = scoped_request_identity(
                "cma-sponsored-model-body-bound-request-v1",
                authority,
                logical_request_id,
            )?;
            Ok(format!("smr-b-{identity}-{request_digest}"))
        }
        _ => Err(SponsoredModelRefusal::MalformedRequest),
    }
}

fn visible_ascii(value: &[u8], maximum: usize) -> Result<&str, SponsoredModelRefusal> {
    if value.is_empty()
        || value.len() > maximum
        || !value.iter().all(|byte| (0x21..=0x7e).contains(byte))
    {
        return Err(SponsoredModelRefusal::MalformedRequest);
    }
    std::str::from_utf8(value).map_err(|_| SponsoredModelRefusal::MalformedRequest)
}

fn scoped_request_identity(
    domain: &str,
    authority: &SponsoredModelRuntimeAuthority,
    logical_request_id: &str,
) -> Result<String, SponsoredModelRefusal> {
    let canonical = serde_json::to_vec(&(
        domain,
        &authority.session_id,
        &authority.launch_id,
        &authority.grant_id,
        logical_request_id,
    ))
    .map_err(|_| SponsoredModelRefusal::MalformedRequest)?;
    Ok(lower_hex(Sha256::digest(canonical)))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&object[key]));
            }
            serde_json::Value::Object(canonical)
        }
        scalar => scalar.clone(),
    }
}

fn lower_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug)]
pub(crate) struct PreparedSponsoredModelRequest {
    pub target: reqwest::Url,
    pub body: Vec<u8>,
    pub stream_response: bool,
}

#[derive(Debug)]
pub(crate) struct ValidatedSponsoredModelRequest {
    authority: SponsoredModelRuntimeAuthority,
    stable_request_id: String,
    request_digest: String,
    maximum_quota: SponsoredModelQuota,
    model: String,
    request: serde_json::Value,
    stream_response: bool,
}

impl ValidatedSponsoredModelRequest {
    pub(crate) fn prepare(
        self,
        custody_generation: Option<NonZeroU64>,
    ) -> Result<PreparedSponsoredModelRequest, SponsoredModelRefusal> {
        let custody_generation = custody_generation
            .ok_or(SponsoredModelRefusal::CredentialGenerationMismatch)?
            .get();
        if custody_generation < self.authority.session_credential_generation {
            return Err(SponsoredModelRefusal::CredentialGenerationMismatch);
        }
        let mut authority = self.authority;
        authority.session_credential_generation = custody_generation;
        let envelope = SponsoredModelGatewayRequest {
            version: GATEWAY_REQUEST_VERSION,
            authority,
            stable_request_id: self.stable_request_id,
            request_digest: self.request_digest,
            maximum_quota: self.maximum_quota,
            model: self.model,
            resource: SPONSORED_MODEL_RESOURCE,
            request: self.request,
        };
        let target = gateway_target(envelope.authority.gateway_origin.as_str())?;
        let body =
            serde_json::to_vec(&envelope).map_err(|_| SponsoredModelRefusal::MalformedRequest)?;
        if body.len() > MAX_GATEWAY_ENVELOPE_BYTES {
            return Err(SponsoredModelRefusal::MalformedRequest);
        }
        Ok(PreparedSponsoredModelRequest {
            target,
            body,
            stream_response: self.stream_response,
        })
    }
}

#[derive(Serialize)]
struct SponsoredModelGatewayRequest {
    version: u32,
    authority: SponsoredModelRuntimeAuthority,
    stable_request_id: String,
    request_digest: String,
    maximum_quota: SponsoredModelQuota,
    model: String,
    resource: &'static str,
    request: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct SponsoredModelQuota {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(runtime: &str) -> SponsoredModelRuntimeAuthority {
        SponsoredModelRuntimeAuthority {
            version: AUTHORITY_VERSION,
            gateway_origin: "https://cma.internal.example".into(),
            runtime_audience: runtime.into(),
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
    async fn required_gate_refuses_model_calls_until_exact_authority_is_installed() {
        let gate = SponsoredModelGate::new(true, authority("runtime-a").session_id).unwrap();
        let result = gate
            .prepare(
                &Method::POST,
                &"/responses".parse().unwrap(),
                br#"{"model":"model-a","max_output_tokens":10}"#,
            )
            .await;
        assert!(matches!(
            result,
            Err(SponsoredModelRefusal::MissingAuthority)
        ));
        assert!(matches!(
            gate.install("runtime-b", authority("runtime-a")).await,
            Err(SponsoredModelRefusal::AudienceMismatch)
        ));
    }

    #[tokio::test]
    async fn installation_is_exact_idempotent_and_only_rotates_session_credential_forward() {
        let original = authority("runtime-a");
        let gate = SponsoredModelGate::new(true, original.session_id.clone()).unwrap();
        let first = gate.install("runtime-a", original.clone()).await.unwrap();
        assert_eq!(first, (&original).into());
        assert_eq!(
            gate.install("runtime-a", original.clone()).await.unwrap(),
            first
        );

        let mut rotated = original.clone();
        rotated.session_credential_generation += 1;
        assert_eq!(
            gate.install("runtime-a", rotated.clone()).await.unwrap(),
            (&rotated).into()
        );
        assert!(matches!(
            gate.install("runtime-a", original).await,
            Err(SponsoredModelRefusal::AuthorityChanged)
        ));
    }

    #[tokio::test]
    async fn expired_authority_is_refused_at_install_and_rechecked_before_every_request() {
        let mut expired = authority("runtime-a");
        expired.expires_at = "1970-01-01T00:00:01Z".into();
        let gate = SponsoredModelGate::new(true, expired.session_id.clone()).unwrap();
        assert!(matches!(
            gate.install("runtime-a", expired).await,
            Err(SponsoredModelRefusal::ExpiredAuthority)
        ));

        let current = authority("runtime-a");
        gate.install("runtime-a", current).await.unwrap();
        gate.authority.write().await.as_mut().unwrap().expires_at = "1970-01-01T00:00:01Z".into();
        assert!(matches!(
            gate.prepare(
                &Method::POST,
                &"/responses".parse().unwrap(),
                br#"{"model":"model-a","max_output_tokens":10}"#,
            )
            .await,
            Err(SponsoredModelRefusal::ExpiredAuthority)
        ));
    }

    #[tokio::test]
    async fn sponsored_contract_normalizes_streaming_and_enforces_the_authority_ceiling() {
        let installed = authority("runtime-a");
        let gate = SponsoredModelGate::new(true, installed.session_id.clone()).unwrap();
        gate.install("runtime-a", installed).await.unwrap();

        let streaming = gate
            .prepare(
                &Method::POST,
                &"/responses".parse().unwrap(),
                br#"{"model":"model-a","max_output_tokens":10,"stream":true}"#,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(streaming.stream_response);
        let streaming_envelope: serde_json::Value =
            serde_json::from_slice(&streaming.body).unwrap();
        assert_eq!(streaming_envelope["request"]["stream"], false);

        let prepared = gate
            .prepare(
                &Method::POST,
                &"/responses".parse().unwrap(),
                br#"{"model":"model-a","max_output_tokens":10,"stream":false}"#,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prepared.target.path(), GATEWAY_PATH);
        let envelope: serde_json::Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(envelope["maximum_quota"]["requests"], 1);
        assert_eq!(envelope["maximum_quota"]["output_tokens"], 10);
        assert!(envelope["stable_request_id"]
            .as_str()
            .unwrap()
            .starts_with("smr-e-"));

        let injected = gate
            .prepare(
                &Method::POST,
                &"/responses".parse().unwrap(),
                br#"{"model":"model-a","stream":true}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let injected: serde_json::Value = serde_json::from_slice(&injected.body).unwrap();
        assert_eq!(injected["request"]["max_output_tokens"], 16);
        assert_eq!(injected["maximum_quota"]["output_tokens"], 16);

        let tightened = gate
            .prepare(
                &Method::POST,
                &"/responses".parse().unwrap(),
                br#"{"model":"model-a","max_output_tokens":100}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let tightened: serde_json::Value = serde_json::from_slice(&tightened.body).unwrap();
        assert_eq!(tightened["request"]["max_output_tokens"], 16);
    }

    #[test]
    fn gateway_origin_is_https_and_canonical() {
        for invalid in [
            "http://cma.internal.example",
            "https://user@cma.internal.example",
            "https://cma.internal.example/path",
            "https://cma.internal.example/",
        ] {
            let mut value = authority("runtime-a");
            value.gateway_origin = invalid.into();
            assert!(validate_authority(&value).is_err(), "accepted {invalid}");
        }
    }

    #[tokio::test]
    async fn request_digest_is_independent_of_json_object_key_order() {
        let installed = authority("runtime-a");
        let gate = SponsoredModelGate::new(true, installed.session_id.clone()).unwrap();
        gate.install("runtime-a", installed).await.unwrap();

        let first = gate
            .prepare(
                &Method::POST,
                &"/responses".parse().unwrap(),
                br#"{"model":"model-a","max_output_tokens":10,"input":{"b":2,"a":1}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let second = gate
            .prepare(
                &Method::POST,
                &"/responses".parse().unwrap(),
                br#"{"input":{"a":1,"b":2},"max_output_tokens":10,"model":"model-a"}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let first: serde_json::Value = serde_json::from_slice(&first.body).unwrap();
        let second: serde_json::Value = serde_json::from_slice(&second.body).unwrap();
        assert_eq!(first["request_digest"], second["request_digest"]);
        assert_eq!(first["stable_request_id"], second["stable_request_id"]);
    }

    #[tokio::test]
    async fn body_bound_logical_retries_are_stable_but_distinct_bodies_remain_distinct() {
        let installed = authority("runtime-a");
        let gate = SponsoredModelGate::new(true, installed.session_id.clone()).unwrap();
        gate.install("runtime-a", installed).await.unwrap();
        let headers = HeaderMap::new();
        let first_body =
            br#"{"client_metadata":{"turn_id":"turn-1"},"model":"model-a","stream":true}"#;
        let changed_body = br#"{"client_metadata":{"turn_id":"turn-1"},"input":"changed","model":"model-a","stream":true}"#;

        let first = gate
            .validate(
                &Method::POST,
                &"/responses".parse().unwrap(),
                &headers,
                first_body,
            )
            .await
            .unwrap()
            .unwrap()
            .prepare(NonZeroU64::new(7))
            .unwrap();
        let retry = gate
            .validate(
                &Method::POST,
                &"/responses".parse().unwrap(),
                &headers,
                first_body,
            )
            .await
            .unwrap()
            .unwrap()
            .prepare(NonZeroU64::new(7))
            .unwrap();
        let changed = gate
            .validate(
                &Method::POST,
                &"/responses".parse().unwrap(),
                &headers,
                changed_body,
            )
            .await
            .unwrap()
            .unwrap()
            .prepare(NonZeroU64::new(7))
            .unwrap();
        let first: serde_json::Value = serde_json::from_slice(&first.body).unwrap();
        let retry: serde_json::Value = serde_json::from_slice(&retry.body).unwrap();
        let changed: serde_json::Value = serde_json::from_slice(&changed.body).unwrap();

        assert_eq!(first["stable_request_id"], retry["stable_request_id"]);
        assert_ne!(first["stable_request_id"], changed["stable_request_id"]);
        assert!(first["stable_request_id"]
            .as_str()
            .unwrap()
            .ends_with(first["request_digest"].as_str().unwrap()));
    }

    #[tokio::test]
    async fn missing_or_ambiguous_retry_identity_fails_closed() {
        let installed = authority("runtime-a");
        let gate = SponsoredModelGate::new(true, installed.session_id.clone()).unwrap();
        gate.install("runtime-a", installed).await.unwrap();
        let body = br#"{"model":"model-a"}"#;
        assert!(matches!(
            gate.validate(
                &Method::POST,
                &"/responses".parse().unwrap(),
                &HeaderMap::new(),
                body,
            )
            .await,
            Err(SponsoredModelRefusal::MalformedRequest)
        ));

        let mut duplicate = HeaderMap::new();
        duplicate.append(IDEMPOTENCY_KEY_HEADER, "one".parse().unwrap());
        duplicate.append(IDEMPOTENCY_KEY_HEADER, "two".parse().unwrap());
        assert!(matches!(
            gate.validate(
                &Method::POST,
                &"/responses".parse().unwrap(),
                &duplicate,
                body,
            )
            .await,
            Err(SponsoredModelRefusal::MalformedRequest)
        ));
    }

    #[tokio::test]
    async fn one_mib_raw_request_serializes_within_the_gateway_envelope_allowance() {
        let installed = authority("runtime-a");
        let generation = NonZeroU64::new(installed.session_credential_generation);
        let gate = SponsoredModelGate::new(true, installed.session_id.clone()).unwrap();
        gate.install("runtime-a", installed).await.unwrap();
        let mut request = serde_json::json!({
            "input": "",
            "max_output_tokens": 10,
            "model": "model-a",
            "stream": false,
        });
        let empty_length = serde_json::to_vec(&request).unwrap().len();
        request["input"] =
            serde_json::Value::String("x".repeat(MAX_REQUEST_BODY_BYTES - empty_length));
        let body = serde_json::to_vec(&request).unwrap();
        assert_eq!(body.len(), MAX_REQUEST_BODY_BYTES);

        let prepared = gate
            .validate(
                &Method::POST,
                &"/responses".parse().unwrap(),
                &HeaderMap::from_iter([(
                    IDEMPOTENCY_KEY_HEADER.parse().unwrap(),
                    "large-request".parse().unwrap(),
                )]),
                &body,
            )
            .await
            .unwrap()
            .unwrap()
            .prepare(generation)
            .unwrap();

        assert!(prepared.body.len() > MAX_REQUEST_BODY_BYTES);
        assert!(prepared.body.len() <= MAX_GATEWAY_ENVELOPE_BYTES);
    }
}
