use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DOCUMENT_VERSION: i32 = 1;
mod registry;
pub use registry::ImageRegistry;
pub(crate) use registry::{project_id, resource_segment};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid request: {0}")]
    Invalid(&'static str),
    #[error("resource not found")]
    NotFound,
    #[error("request conflicts with the current resource")]
    Conflict,
    #[error("service dependency is unavailable")]
    Unavailable,
    #[error("build submission was refused before acceptance")]
    BuildRejected,
    #[error("database operation failed")]
    Database(#[from] mongodb::error::Error),
    #[error("database schema is not ready")]
    Schema(#[from] crate::schema::SchemaError),
}
pub type Result<T> = std::result::Result<T, Error>;

pub fn bounded_text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
pub fn hash(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Invalid("invalid JSON"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub fn id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
pub fn valid_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(&format!("{prefix}_")).is_some_and(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateImage {
    pub name: String,
    #[serde(default)]
    pub description: String,
}
impl CreateImage {
    pub fn validate(&self) -> Result<()> {
        if !bounded_text(&self.name, 128)
            || self.description.len() > 4096
            || self.description.contains('\0')
        {
            return Err(Error::Invalid("invalid image name or description"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Image {
    #[serde(rename = "_id")]
    pub id: String,
    pub schema_version: i32,
    pub owner: String,
    pub request_key_hash: String,
    pub request_sha256: String,
    pub spec: CreateImage,
    pub shares: Vec<String>,
    #[serde(default)]
    pub visibility: ImageVisibility,
    pub retired: bool,
    pub revision: i64,
    pub created_at_ms: i64,
}
impl Image {
    pub fn accessible(&self, actor: &str) -> bool {
        self.owner == actor
            || self.shares.iter().any(|s| s == actor)
            || (self.visibility == ImageVisibility::Public && !self.retired)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageVisibility {
    #[default]
    Private,
    Public,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildInput {
    pub repository: String,
    pub commit: String,
    #[serde(default = "default_dockerfile")]
    pub dockerfile: String,
    #[serde(default = "default_context")]
    pub context: String,
    /// Optional Dockerfile imported from the user's local file picker. The
    /// repository remains the build context; Cloud Build writes this content
    /// to a fixed workspace path before invoking Docker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dockerfile_content: Option<String>,
}
fn default_dockerfile() -> String {
    "Dockerfile".into()
}
fn default_context() -> String {
    ".".into()
}
impl BuildInput {
    pub fn validate(&self) -> Result<()> {
        let url = url::Url::parse(&self.repository)
            .map_err(|_| Error::Invalid("repository must be an HTTPS GitHub URL"))?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.port().is_some()
            || self.repository.len() > 1024
            || url.path().trim_matches('/').split('/').count() != 2
            || !url.path().trim_matches('/').split('/').all(|s| {
                !s.is_empty()
                    && s != "."
                    && s != ".."
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
        {
            return Err(Error::Invalid(
                "repository must identify one GitHub repository",
            ));
        }
        if self.commit.len() != 40
            || !self
                .commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("source commit must be a full lowercase SHA"));
        }
        if !relative_path(&self.dockerfile, false) || !relative_path(&self.context, true) {
            return Err(Error::Invalid(
                "build paths must stay within the repository",
            ));
        }
        if self.dockerfile_content.as_deref().is_some_and(|content| {
            content.is_empty() || content.len() > 16 * 1024 || content.contains('\0')
        }) {
            return Err(Error::Invalid(
                "imported Dockerfile must be non-empty and at most 16 KiB",
            ));
        }
        Ok(())
    }
}
fn relative_path(value: &str, dot: bool) -> bool {
    dot && value == "."
        || !value.is_empty()
            && value.len() <= 512
            && !value.starts_with(['/', '-'])
            && value.split('/').all(|s| {
                s != "."
                    && s != ".."
                    && !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuildPhase {
    Queued,
    Dispatching,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Build {
    #[serde(rename = "_id")]
    pub id: String,
    pub schema_version: i32,
    pub image_id: String,
    pub owner: String,
    pub request_key_hash: String,
    pub request_sha256: String,
    pub source: BuildInput,
    pub phase: BuildPhase,
    pub revision: i64,
    pub cloud_id: Option<String>,
    pub digest: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
