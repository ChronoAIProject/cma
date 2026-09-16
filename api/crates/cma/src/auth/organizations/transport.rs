use std::time::Duration;

use layer2_core::resource_access::MembershipError;
use layer2_core::SubjectId;
use serde::de::DeserializeOwned;

use super::super::{persistence::SecretDigest, AuthMode, AuthState};

pub(super) const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BODY_BYTES: usize = 1024 * 1024;

impl AuthState {
    pub(super) fn organization_endpoint(
        &self,
        organization: Option<&SubjectId>,
    ) -> Result<reqwest::Url, MembershipError> {
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(MembershipError::Unavailable);
        };
        let mut endpoint =
            reqwest::Url::parse(base_url).map_err(|_| MembershipError::Unavailable)?;
        endpoint.set_path("/api/v1/orgs");
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        if let Some(organization) = organization {
            if !canonical_segment(organization.as_str()) {
                return Err(MembershipError::InvalidEvidence);
            }
            endpoint
                .path_segments_mut()
                .map_err(|_| MembershipError::Unavailable)?
                .push(organization.as_str())
                .push("authorization");
        }
        Ok(endpoint)
    }

    pub(super) async fn organization_json<T: DeserializeOwned>(
        &self,
        endpoint: reqwest::Url,
        credential: &str,
    ) -> Result<T, MembershipError> {
        let mut response = self
            .http
            .get(endpoint)
            .bearer_auth(credential)
            .send()
            .await
            .map_err(|_| MembershipError::Unavailable)?;
        match response.status().as_u16() {
            401 | 403 | 404 => return Err(MembershipError::NotMember),
            200 => {}
            _ => return Err(MembershipError::Unavailable),
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_BODY_BYTES as u64)
        {
            return Err(MembershipError::InvalidEvidence);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| MembershipError::Unavailable)?
        {
            if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
                return Err(MembershipError::InvalidEvidence);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| MembershipError::InvalidEvidence)
    }
}

pub(super) fn canonical_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn digest_id(digest: SecretDigest) -> String {
    digest
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
