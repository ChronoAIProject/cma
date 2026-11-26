use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubRepositoryScope {
    pub repository: String,
    pub required_scope: GithubRepositoryScopeNeed,
    pub manage_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubRepositoryScopeNeed {
    PublicRepo,
    Repo,
}

impl GithubRepositoryScopeNeed {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PublicRepo => "public_repo",
            Self::Repo => "repo",
        }
    }

    pub fn is_satisfied_by(self, scopes: &[&str]) -> bool {
        scopes.contains(&"repo") || scopes.contains(&self.as_str())
    }
}

impl GithubRepositoryScope {
    pub(crate) fn is_valid(&self, connection_id: &str) -> bool {
        let valid_repo = self
            .repository
            .split_once('/')
            .is_some_and(|(owner, name)| {
                [owner, name].into_iter().all(|part| {
                    !part.is_empty()
                        && part.len() <= 100
                        && !matches!(part, "." | "..")
                        && part.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                        })
                })
            });
        valid_repo
            && url::Url::parse(&self.manage_url).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path() == format!("/keys/{connection_id}")
                    && url.query().is_none()
                    && url.fragment().is_none()
            })
    }
}
