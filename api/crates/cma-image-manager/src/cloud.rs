//! Cloud Build submits immutable sources and publishes to the configured CMA repository.
use crate::model::*;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct CloudBuild {
    http: reqwest::Client,
    pub project: String,
    pub location: String,
    pub service_account: String,
    pub builder_image: String,
    token_file: Option<std::path::PathBuf>,
}

#[derive(Debug)]
pub struct Observation {
    pub id: String,
    pub phase: BuildPhase,
    pub digest: Option<String>,
}

#[async_trait::async_trait]
pub trait BuildExecutor: Send + Sync {
    async fn submit(&self, build: &Build) -> Result<()>;
    async fn observe(&self, build: &Build) -> Result<Option<Observation>>;
    async fn logs(&self, build: &Build) -> Result<Value>;
}

impl CloudBuild {
    pub fn new(
        project: String,
        location: String,
        service_account: String,
        builder_image: String,
        token_file: Option<std::path::PathBuf>,
    ) -> Result<Self> {
        if project != "chronoai-501608"
            || location != "asia-northeast1"
            || !service_account.starts_with("projects/chronoai-501608/serviceAccounts/")
            || !service_account.ends_with("@chronoai-501608.iam.gserviceaccount.com")
            || service_account.chars().any(char::is_whitespace)
            || !builder_image.rsplit_once('@').is_some_and(|(image, d)| {
                image == "gcr.io/cloud-builders/docker" && valid_digest(d)
            })
        {
            return Err(Error::Invalid("invalid Cloud Build configuration"));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            http,
            project,
            location,
            service_account,
            builder_image,
            token_file,
        })
    }
    fn collection(&self) -> String {
        format!(
            "https://cloudbuild.googleapis.com/v1/projects/{}/locations/{}/builds",
            self.project, self.location
        )
    }
    async fn token(&self) -> Result<String> {
        if let Some(path) = &self.token_file {
            let value = std::fs::read_to_string(path).map_err(|_| Error::Unavailable)?;
            let value = value.trim();
            if !bounded_text(value, 16384) {
                return Err(Error::Unavailable);
            }
            return Ok(value.into());
        }
        let response = self.http.get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
            .header("Metadata-Flavor","Google").timeout(Duration::from_secs(3)).send().await.map_err(|_|Error::Unavailable)?;
        let value = read_json(response).await?;
        value["access_token"]
            .as_str()
            .filter(|s| bounded_text(s, 16384))
            .map(str::to_owned)
            .ok_or(Error::Unavailable)
    }
    pub fn request(&self, build: &Build) -> Result<Value> {
        build.source.validate()?;
        if !valid_id(&build.id, "bld") || !valid_id(&build.image_id, "img") {
            return Err(Error::Invalid("invalid build identity"));
        }
        Ok(json!({
            "source":{"gitSource":{"url":build.source.repository,"revision":build.source.commit}},
            "steps":[{"name":self.builder_image,"args":["build","--file",build.source.dockerfile,"--tag",build.destination(),build.source.context]}],
            "images":[build.destination()],"tags":[format!("cma-im-{}",build.id)],
            "serviceAccount":self.service_account,"timeout":"1800s",
            "options":{"logging":"CLOUD_LOGGING_ONLY"}
        }))
    }
    fn observation(&self, build: &Build, value: &Value) -> Result<Observation> {
        let expected = self.request(build)?;
        if value["source"]["gitSource"] != expected["source"]["gitSource"]
            || value["images"] != expected["images"]
            || value["steps"].as_array().is_none_or(|steps| {
                steps.len() != 1
                    || steps[0]["name"] != expected["steps"][0]["name"]
                    || steps[0]["args"] != expected["steps"][0]["args"]
            })
            || value["serviceAccount"] != expected["serviceAccount"]
            || !value["tags"]
                .as_array()
                .is_some_and(|tags| tags.contains(&expected["tags"][0]))
        {
            return Err(Error::Conflict);
        }
        let id = value["id"]
            .as_str()
            .filter(|s| uuid::Uuid::parse_str(s).is_ok())
            .ok_or(Error::Unavailable)?
            .to_owned();
        let phase = match value["status"].as_str() {
            Some("PENDING" | "QUEUED" | "WORKING") => BuildPhase::Running,
            Some("SUCCESS") => BuildPhase::Succeeded,
            Some("CANCELLED") => BuildPhase::Cancelled,
            Some("FAILURE" | "INTERNAL_ERROR" | "TIMEOUT" | "EXPIRED") => BuildPhase::Failed,
            _ => return Err(Error::Unavailable),
        };
        let digest = if phase == BuildPhase::Succeeded {
            let images = value["results"]["images"]
                .as_array()
                .ok_or(Error::Unavailable)?;
            let result = images
                .iter()
                .find(|v| v["name"] == build.destination())
                .ok_or(Error::Conflict)?;
            Some(
                result["digest"]
                    .as_str()
                    .filter(|s| valid_digest(s))
                    .ok_or(Error::Conflict)?
                    .to_owned(),
            )
        } else {
            None
        };
        Ok(Observation { id, phase, digest })
    }
}

#[async_trait::async_trait]
impl BuildExecutor for CloudBuild {
    async fn submit(&self, build: &Build) -> Result<()> {
        // Exactly one submission after a durable claim. A lost response leaves
        // the build dispatching and is recovered by its unique tag, never POSTed again.
        let body = self.request(build).map_err(|_| Error::BuildRejected)?;
        let token = self.token().await.map_err(|_| Error::BuildRejected)?;
        let response = self
            .http
            .post(self.collection())
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        // Only definitive validation/authentication rejections prove no build was
        // accepted. Timeouts and server failures must reconcile the original tag.
        if matches!(
            response.status().as_u16(),
            400 | 401 | 403 | 404 | 405 | 413 | 415 | 422
        ) {
            return Err(Error::BuildRejected);
        }
        read_json(response).await?;
        Ok(())
    }
    async fn observe(&self, build: &Build) -> Result<Option<Observation>> {
        let token = self.token().await?;
        if let Some(id) = &build.cloud_id {
            uuid::Uuid::parse_str(id).map_err(|_| Error::Conflict)?;
            let value = read_json(
                self.http
                    .get(format!("{}/{}", self.collection(), id))
                    .bearer_auth(token)
                    .send()
                    .await
                    .map_err(|_| Error::Unavailable)?,
            )
            .await?;
            return self.observation(build, &value).map(Some);
        }
        let mut cursor = String::new();
        let mut observed = None;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..10 {
            let page = read_json(
                self.http
                    .get(self.collection())
                    .bearer_auth(&token)
                    .query(&[
                        ("filter", format!("tags=\"cma-im-{}\"", build.id)),
                        ("pageSize", "100".into()),
                        ("pageToken", cursor.clone()),
                    ])
                    .send()
                    .await
                    .map_err(|_| Error::Unavailable)?,
            )
            .await?;
            if let Some(builds) = page["builds"].as_array() {
                for value in builds {
                    if observed.is_some() {
                        return Err(Error::Conflict);
                    }
                    observed = Some(self.observation(build, value)?);
                }
            }
            cursor = page["nextPageToken"].as_str().unwrap_or_default().into();
            if cursor.is_empty() {
                return Ok(observed);
            }
            if !seen.insert(cursor.clone()) {
                return Err(Error::Unavailable);
            }
        }
        Err(Error::Unavailable)
    }
    async fn logs(&self, build: &Build) -> Result<Value> {
        let id = build.cloud_id.as_ref().ok_or(Error::Conflict)?;
        uuid::Uuid::parse_str(id).map_err(|_| Error::Conflict)?;
        let value=read_json(self.http.post("https://logging.googleapis.com/v2/entries:list").bearer_auth(self.token().await?)
            .json(&json!({"resourceNames":[format!("projects/{}",self.project)],
                "filter":format!("resource.type=\"build\" AND resource.labels.build_id=\"{id}\""),"pageSize":200,"orderBy":"timestamp desc"}))
            .send().await.map_err(|_|Error::Unavailable)?).await?;
        let entries: Vec<_> = value["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| json!({"timestamp":v["timestamp"],"text":v["textPayload"]}))
            .collect();
        Ok(
            json!({"entries":entries,"truncated":value["nextPageToken"].as_str().is_some_and(|s|!s.is_empty())}),
        )
    }
}

async fn read_json(mut response: reqwest::Response) -> Result<Value> {
    if !response.status().is_success() {
        return Err(Error::Unavailable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(Error::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)
}

pub type Executor = Arc<dyn BuildExecutor>;

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (CloudBuild, Build) {
        let executor=CloudBuild::new("chronoai-501608".into(),"asia-northeast1".into(),"projects/chronoai-501608/serviceAccounts/test-build@chronoai-501608.iam.gserviceaccount.com".into(),format!("gcr.io/cloud-builders/docker@sha256:{}","a".repeat(64)),None).unwrap();
        let build = Build {
            id: id("bld"),
            schema_version: DOCUMENT_VERSION,
            image_id: id("img"),
            owner: "alice".into(),
            request_key_hash: "a".repeat(64),
            request_sha256: "b".repeat(64),
            source: BuildInput {
                repository: "https://github.com/example/runtime".into(),
                commit: "a".repeat(40),
                dockerfile: "infra/Dockerfile".into(),
                context: ".".into(),
            },
            phase: BuildPhase::Dispatching,
            revision: 2,
            cloud_id: None,
            digest: None,
            error: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        (executor, build)
    }
    #[test]
    fn accepts_only_the_exact_build_source_destination_and_identity() {
        let (executor, build) = fixture();
        let mut response = executor.request(&build).unwrap();
        response["id"] = json!(uuid::Uuid::new_v4());
        response["status"] = json!("SUCCESS");
        response["results"] = json!({"images":[{"name":build.destination(),"digest":format!("sha256:{}","b".repeat(64))}]});
        let result = executor.observation(&build, &response).unwrap();
        assert_eq!(result.phase, BuildPhase::Succeeded);
        for field in [
            "source",
            "images",
            "steps",
            "serviceAccount",
            "tags",
            "results",
        ] {
            let mut changed = response.clone();
            changed[field] = json!(null);
            assert!(
                executor.observation(&build, &changed).is_err(),
                "accepted changed {field}"
            );
        }
        assert!(build.destination().starts_with(REGISTRY));
    }
    #[test]
    fn source_paths_cannot_inject_build_flags_or_escape_checkout() {
        let (executor, mut build) = fixture();
        for path in [
            "../Dockerfile",
            "/etc/passwd",
            "--output=elsewhere",
            "a/../../file",
            "a\\file",
        ] {
            build.source.dockerfile = path.into();
            assert!(executor.request(&build).is_err());
        }
    }
}
