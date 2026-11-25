use async_trait::async_trait;
use layer1_public::rest::{AgentLifecycleImageReference, AgentLifecycleProvisioningError};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[async_trait]
pub(crate) trait ImageResolver: Send + Sync {
    fn retirement_enabled(&self) -> bool {
        false
    }

    async fn resolve(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError>;

    async fn resolve_retained(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
        _pinned_reference: &str,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        self.resolve(delegated_credential, image).await
    }
}

/// Thin, bounded client for the independently deployed Image Manager. The
/// caller's delegated credential is used for the exact image permission check;
/// CMA never stores or substitutes a service-wide token.
pub(crate) struct ImageManagerResolver {
    base_url: String,
    http: reqwest::Client,
    retirement_enabled: bool,
}

impl ImageManagerResolver {
    pub(crate) fn new(base_url: String, retirement_enabled: bool) -> Result<Arc<Self>, String> {
        let parsed = reqwest::Url::parse(&base_url)
            .map_err(|_| "Image Manager base URL is not a valid URL".to_owned())?;
        if parsed.host_str().is_none()
            || !matches!(parsed.scheme(), "http" | "https")
            || parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || base_url.ends_with('/')
        {
            return Err("Image Manager base URL is not a safe service URL".into());
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Image Manager HTTP client could not be initialized".to_owned())?;
        Ok(Arc::new(Self {
            base_url,
            http,
            retirement_enabled,
        }))
    }
}

#[derive(serde::Deserialize)]
struct ResolvedImage {
    id: Option<String>,
    reference: Option<String>,
    digest: Option<String>,
    created_at_ms: Option<i64>,
}

#[async_trait]
impl ImageResolver for ImageManagerResolver {
    fn retirement_enabled(&self) -> bool {
        self.retirement_enabled
    }

    async fn resolve_retained(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
        pinned_reference: &str,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        if !self.retirement_enabled {
            return self
                .resolve_catalog_selection(delegated_credential, image)
                .await;
        }
        // Revalidate current use access to the original immutable version.
        // Historical selections without a version cannot authorize a new
        // profile through the mutable latest-version lookup.
        if image.version.is_none() {
            return Err(AgentLifecycleProvisioningError::ImageSelectionUnsupported);
        }
        let reference = self
            .resolve_catalog_selection(delegated_credential, image)
            .await?;
        if reference != pinned_reference {
            return Err(AgentLifecycleProvisioningError::ImageSelectionUnsupported);
        }
        Ok(reference)
    }

    async fn resolve(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        if self.retirement_enabled {
            return Err(AgentLifecycleProvisioningError::ImageSelectionUnsupported);
        }
        self.resolve_catalog_selection(delegated_credential, image)
            .await
    }
}

impl ImageManagerResolver {
    async fn resolve_catalog_selection(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        let credential =
            delegated_credential.ok_or(AgentLifecycleProvisioningError::SignInRequired)?;
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
            segments
                .push("v1")
                .push("images")
                .push(&image.id)
                .push("resolve");
        }
        let version = match image.version.as_deref() {
            Some(value) => value.to_owned(),
            None => self.latest_version(&image.id, credential).await?,
        };
        let response = self
            .http
            .post(url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {credential}"),
            )
            .json(&json!({"version": version.as_str()}))
            .send()
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        let status = response.status();
        require_image_authority(status)?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "the selected Image Manager image version was not found".into(),
            ));
        }
        if !status.is_success() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        let resolved: ResolvedImage = serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        if self.retirement_enabled && resolved.id.as_deref() != Some(version.as_str()) {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        let reference = resolved
            .reference
            .ok_or_else(|| AgentLifecycleProvisioningError::Unavailable)?;
        let digest = resolved
            .digest
            .ok_or_else(|| AgentLifecycleProvisioningError::Unavailable)?;
        if !valid_immutable_image(&reference)
            || digest
                != reference
                    .rsplit_once('@')
                    .map(|(_, value)| value)
                    .unwrap_or_default()
        {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        Ok(reference)
    }
    async fn latest_version(
        &self,
        image_id: &str,
        credential: &str,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
            segments
                .push("v1")
                .push("images")
                .push(image_id)
                .push("versions");
        }
        let response = self
            .http
            .get(url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {credential}"),
            )
            .send()
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        let status = response.status();
        require_image_authority(status)?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "the selected Image Manager image was not found".into(),
            ));
        }
        if !status.is_success() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        #[derive(serde::Deserialize)]
        struct Page {
            items: Vec<ResolvedImage>,
        }
        let page: Page = serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        page.items
            .into_iter()
            .filter_map(|value| {
                value
                    .id
                    .map(|id| (value.created_at_ms.unwrap_or_default(), id))
            })
            .max_by(|left, right| left.cmp(right))
            .map(|(_, id)| id)
            .ok_or_else(|| {
                AgentLifecycleProvisioningError::Invalid(
                    "the selected Image Manager image has no successful version".into(),
                )
            })
    }
}

fn require_image_authority(
    status: reqwest::StatusCode,
) -> Result<(), AgentLifecycleProvisioningError> {
    match status {
        reqwest::StatusCode::UNAUTHORIZED => Err(AgentLifecycleProvisioningError::SignInRequired),
        reqwest::StatusCode::FORBIDDEN => Err(AgentLifecycleProvisioningError::ImageAccessDenied),
        _ => Ok(()),
    }
}

async fn bounded_body(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, AgentLifecycleProvisioningError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?
    {
        if chunk.len() > (64 * 1024_usize).saturating_sub(body.len()) {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
#[path = "agent_lifecycle_image_tests.rs"]
mod tests;

pub(crate) fn valid_immutable_image(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 2_048
        && !name.contains('@')
        && !name.contains("://")
        && !name.starts_with(['.', '-', '_', '/', ':'])
        && !name.ends_with(['.', '-', '_', '/', ':'])
        && !name.contains("//")
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
