use async_trait::async_trait;
use layer1_public::rest::{AgentLifecycleImageReference, AgentLifecycleProvisioningError};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[async_trait]
pub(crate) trait ImageResolver: Send + Sync {
    async fn resolve(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError>;
}

/// Thin, bounded client for the independently deployed Image Manager. The
/// caller's delegated credential is used for the exact image permission check;
/// CMA never stores or substitutes a service-wide token.
pub(crate) struct ImageManagerResolver {
    base_url: String,
    http: reqwest::Client,
}

impl ImageManagerResolver {
    pub(crate) fn new(base_url: String) -> Result<Arc<Self>, String> {
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
        Ok(Arc::new(Self { base_url, http }))
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
    async fn resolve(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        let credential = delegated_credential.ok_or_else(|| {
            AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-image-manager".into()],
                connection_target: json!({"type":"image_manager","image_id":image.id}),
            }
        })?;
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
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        if bytes.len() > 64 * 1024 {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-image-manager".into()],
                connection_target: json!({"type":"image_manager","image_id":image.id,"version":version}),
            });
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "the selected Image Manager image version was not found".into(),
            ));
        }
        if !status.is_success() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        let resolved: ResolvedImage = serde_json::from_slice(&bytes)
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
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
}

impl ImageManagerResolver {
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
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        if bytes.len() > 64 * 1024 {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["cma-image-manager".into()],
                connection_target: json!({"type":"image_manager","image_id":image_id}),
            });
        }
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
        let page: Page = serde_json::from_slice(&bytes)
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

fn valid_immutable_image(value: &str) -> bool {
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
