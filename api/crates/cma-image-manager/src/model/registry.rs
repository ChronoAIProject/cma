use super::{Build, Error, Result};

/// Server-owned destination shared by build verification and public projections.
/// Keep this repository stable for retained builds; changing it is a migration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRegistry {
    repository: String,
    project: String,
}

impl ImageRegistry {
    pub fn parse(repository: String) -> Result<Self> {
        let parts: Vec<_> = repository.split('/').collect();
        if parts.len() != 3
            || !parts[0]
                .strip_suffix("-docker.pkg.dev")
                .is_some_and(resource_segment)
            || !project_id(parts[1])
            || !resource_segment(parts[2])
        {
            return Err(Error::Invalid("invalid image registry repository"));
        }
        let project = parts[1].to_owned();
        Ok(Self {
            repository,
            project,
        })
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn destination(&self, build: &Build) -> String {
        format!("{}/{}:{}", self.repository, build.image_id, build.id)
    }

    pub fn reference(&self, build: &Build) -> Option<String> {
        build
            .digest
            .as_ref()
            .map(|digest| format!("{}/{}@{digest}", self.repository, build.image_id))
    }
}

pub(crate) fn resource_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn project_id(value: &str) -> bool {
    (6..=30).contains(&value.len()) && resource_segment(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_explicit_regional_and_multi_region_repositories() {
        for region in ["us", "europe-west1", "asia-east1"] {
            let registry = ImageRegistry::parse(format!(
                "{region}-docker.pkg.dev/example-build-project/release-images"
            ))
            .unwrap();
            assert_eq!(registry.project(), "example-build-project");
        }
    }

    #[test]
    fn rejects_non_registry_urls_and_path_or_reference_injection() {
        for value in [
            "",
            "https://us-docker.pkg.dev/example-project/images",
            "user@us-docker.pkg.dev/example-project/images",
            "us-docker.pkg.dev.evil.example/example-project/images",
            "us-docker.pkg.dev/example-project/images/extra",
            "us-docker.pkg.dev/example-project/../images",
            "us-docker.pkg.dev/example-project/images:tag",
            "us-docker.pkg.dev/example-project/images@sha256:abcd",
            "us-docker.pkg.dev/example-project/images?x=1",
            "us-docker.pkg.dev/example-project/images#fragment",
            "us-docker.pkg.dev/example-project/images/",
            "us-docker.pkg.dev/example-project/images\n",
            "us-docker.pkg.dev/short/images",
            "us-docker.pkg.dev/Example-project/images",
        ] {
            assert!(ImageRegistry::parse(value.into()).is_err(), "{value}");
        }
    }
}
