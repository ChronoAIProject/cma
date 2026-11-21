//! Read-only discovery of the current subject's existing bot API connections.
//! The request bearer is borrowed for these reads and is never retained or renewed.
use crate::auth::CreatorAuthenticator;
use futures_util::{stream, StreamExt, TryStreamExt};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use utoipa::ToSchema;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 1000;
const MAX_SHARED_ENDPOINTS: usize = 32;

#[derive(Clone)]
pub struct ResourceConnections {
    pub(crate) creator_auth: Arc<CreatorAuthenticator>,
    http: Client,
    base: Url,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePlatform {
    Lark,
    Feishu,
}
impl ResourcePlatform {
    fn service(self) -> (&'static str, &'static str) {
        match self {
            Self::Lark => ("api-lark-bot", "https://open.larksuite.com"),
            Self::Feishu => ("api-feishu-bot", "https://open.feishu.cn"),
        }
    }
}

/// Provenance of an existing connection, never an assertion about channel-key scope.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceConnectionSource {
    Personal,
    Organization,
}

/// Metadata only. Selection does not grant a channel key access to this connection.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct ResourceConnection {
    pub user_service_id: String,
    pub catalog_service_id: String,
    pub name: String,
    pub platform: ResourcePlatform,
    pub source: ResourceConnectionSource,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ResourceConnectionList {
    pub items: Vec<ResourceConnection>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DiscoveryError {
    Unauthenticated,
    Forbidden,
    NotFound,
    Unavailable,
}

impl ResourceConnections {
    pub fn new(
        creator_auth: Arc<CreatorAuthenticator>,
        origin: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Keep the existing trusted deployment-origin policy, including the
        // already-supported internal service origins. No caller supplies this URL.
        let base = Url::parse(&crate::config::validate_origin(
            crate::CMAEG_NYXID_BASE_URL,
            origin,
        )?)?;
        Ok(Self {
            creator_auth,
            base,
            http: Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(8))
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()?,
        })
    }

    pub(crate) async fn list(
        &self,
        bearer: &str,
        platform: ResourcePlatform,
    ) -> Result<ResourceConnectionList, DiscoveryError> {
        tokio::time::timeout(Duration::from_secs(8), self.discover(bearer, platform))
            .await
            .map_err(|_| DiscoveryError::Unavailable)?
    }

    async fn discover(
        &self,
        bearer: &str,
        platform: ResourcePlatform,
    ) -> Result<ResourceConnectionList, DiscoveryError> {
        let (services, mut endpoints, catalog) = tokio::try_join!(
            self.read::<Services>("/api/v1/user-services", bearer),
            self.read::<Endpoints>("/api/v1/endpoints", bearer),
            self.read::<Catalog>("/api/v1/services", bearer),
        )?;
        let missing = shared_endpoints(&services, &endpoints, &catalog, platform)?;
        let extra: Vec<Option<Endpoint>> =
            stream::iter(missing.into_iter().map(|(id, catalog_id)| async move {
                let evidence: Endpoint = match self
                    .read(&format!("/api/v1/endpoints/{id}/authorization"), bearer)
                    .await
                {
                    Ok(value) => value,
                    Err(DiscoveryError::Forbidden | DiscoveryError::NotFound) => return Ok(None),
                    Err(error) => return Err(error),
                };
                // This public projection omits custom URLs. Only the catalog-managed
                // endpoint convention can be established from its exact evidence.
                if evidence.id != id
                    || evidence.catalog_service_id.as_deref() != Some(&catalog_id)
                    || evidence.auto_connected != Some(true)
                    || evidence.url.is_some()
                {
                    return Ok(None);
                }
                Ok(Some(evidence))
            }))
            .buffer_unordered(4)
            .try_collect()
            .await?;
        endpoints.endpoints.extend(extra.into_iter().flatten());
        project(services, endpoints, catalog, platform)
    }

    async fn read<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        bearer: &str,
    ) -> Result<T, DiscoveryError> {
        let mut response = self
            .http
            .get(
                self.base
                    .join(path)
                    .map_err(|_| DiscoveryError::Unavailable)?,
            )
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|_| DiscoveryError::Unavailable)?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Err(DiscoveryError::Unauthenticated),
            StatusCode::FORBIDDEN => return Err(DiscoveryError::Forbidden),
            StatusCode::NOT_FOUND => return Err(DiscoveryError::NotFound),
            StatusCode::OK => {}
            _ => return Err(DiscoveryError::Unavailable),
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_BYTES as u64)
        {
            return Err(DiscoveryError::Unavailable);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| DiscoveryError::Unavailable)?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(DiscoveryError::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| DiscoveryError::Unavailable)
    }
}

#[derive(Deserialize)]
struct Services {
    services: Vec<Service>,
}
#[derive(Deserialize)]
struct Service {
    id: String,
    slug: Option<String>,
    label: Option<String>,
    endpoint_id: Option<String>,
    catalog_service_id: Option<String>,
    is_active: Option<bool>,
    auth_method: Option<String>,
    node_id: Option<String>,
    credential_source: Option<CredentialSource>,
}
#[derive(Deserialize)]
struct CredentialSource {
    #[serde(rename = "type")]
    kind: String,
    allowed: Option<bool>,
    role: Option<String>,
}
#[derive(Deserialize)]
struct Endpoints {
    endpoints: Vec<Endpoint>,
}
#[derive(Deserialize)]
struct Endpoint {
    id: String,
    catalog_service_id: Option<String>,
    url: Option<String>,
    auto_connected: Option<bool>,
}
#[derive(Deserialize)]
struct Catalog {
    services: Vec<CatalogService>,
}
#[derive(Deserialize)]
struct CatalogService {
    id: String,
    slug: Option<String>,
    name: Option<String>,
    base_url: Option<String>,
    auth_method: Option<String>,
    is_active: Option<bool>,
}

fn index<T>(rows: &[T], id: impl Fn(&T) -> &str) -> Result<BTreeMap<&str, &T>, DiscoveryError> {
    if rows.len() > MAX_ROWS {
        return Err(DiscoveryError::Unavailable);
    }
    let mut map = BTreeMap::new();
    for row in rows {
        let id = id(row);
        if !identifier(id) || map.insert(id, row).is_some() {
            return Err(DiscoveryError::Unavailable);
        }
    }
    Ok(map)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn display_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn source(service: &Service) -> Option<ResourceConnectionSource> {
    let source = service.credential_source.as_ref()?;
    match source.kind.as_str() {
        "personal" => Some(ResourceConnectionSource::Personal),
        "org"
            if source.allowed == Some(true)
                && matches!(source.role.as_deref(), Some("admin" | "member")) =>
        {
            Some(ResourceConnectionSource::Organization)
        }
        _ => None,
    }
}

fn catalog_matches(
    service: &Service,
    catalog: &CatalogService,
    platform: ResourcePlatform,
) -> bool {
    let (slug, origin) = platform.service();
    source(service).is_some()
        && service.slug.as_deref() == Some(slug)
        && service.is_active == Some(true)
        && service.auth_method.as_deref() == Some("token_exchange")
        && service.node_id.is_none()
        && catalog.slug.as_deref() == Some(slug)
        && catalog.is_active == Some(true)
        && catalog.auth_method.as_deref() == Some("token_exchange")
        && catalog
            .base_url
            .as_deref()
            .is_some_and(|v| v.trim_end_matches('/') == origin)
}

fn shared_endpoints(
    services: &Services,
    endpoints: &Endpoints,
    catalog: &Catalog,
    platform: ResourcePlatform,
) -> Result<BTreeMap<String, String>, DiscoveryError> {
    let services = index(&services.services, |row| &row.id)?;
    let endpoints = index(&endpoints.endpoints, |row| &row.id)?;
    let catalog = index(&catalog.services, |row| &row.id)?;
    let mut missing = BTreeMap::new();
    for service in services.values() {
        if source(service) != Some(ResourceConnectionSource::Organization) {
            continue;
        }
        let (Some(id), Some(catalog_id)) = (
            service.endpoint_id.as_deref(),
            service.catalog_service_id.as_deref(),
        ) else {
            continue;
        };
        if !identifier(id)
            || endpoints.contains_key(id)
            || !catalog
                .get(catalog_id)
                .is_some_and(|c| catalog_matches(service, c, platform))
        {
            continue;
        }
        if missing
            .insert(id.to_owned(), catalog_id.to_owned())
            .is_some_and(|previous| previous != catalog_id)
            || missing.len() > MAX_SHARED_ENDPOINTS
        {
            return Err(DiscoveryError::Unavailable);
        }
    }
    Ok(missing)
}

fn project(
    services: Services,
    endpoints: Endpoints,
    catalog: Catalog,
    platform: ResourcePlatform,
) -> Result<ResourceConnectionList, DiscoveryError> {
    let services = index(&services.services, |row| &row.id)?;
    let endpoints = index(&endpoints.endpoints, |row| &row.id)?;
    let catalog = index(&catalog.services, |row| &row.id)?;
    let (slug, origin) = platform.service();
    let official = |value: Option<&str>| value.is_some_and(|v| v.trim_end_matches('/') == origin);
    let mut items = Vec::new();
    for service in services.values() {
        let Some(source) = source(service) else {
            continue;
        };
        let Some(catalog_id) = service.catalog_service_id.as_deref() else {
            continue;
        };
        let Some(endpoint) = service
            .endpoint_id
            .as_deref()
            .and_then(|id| endpoints.get(id))
        else {
            continue;
        };
        let Some(catalog) = catalog.get(catalog_id) else {
            continue;
        };
        if !catalog_matches(service, catalog, platform)
            || endpoint.catalog_service_id.as_deref() != Some(catalog_id)
            || !(official(endpoint.url.as_deref())
                || endpoint.auto_connected == Some(true) && endpoint.url.is_none())
        {
            continue;
        }
        let name = service
            .label
            .as_deref()
            .filter(|v| display_name(v))
            .or_else(|| catalog.name.as_deref().filter(|v| display_name(v)))
            .unwrap_or(slug);
        items.push(ResourceConnection {
            user_service_id: service.id.clone(),
            catalog_service_id: catalog_id.to_owned(),
            name: name.to_owned(),
            platform,
            source,
        });
    }
    items.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.user_service_id.cmp(&b.user_service_id))
    });
    Ok(ResourceConnectionList { items })
}

#[cfg(test)]
mod tests;
