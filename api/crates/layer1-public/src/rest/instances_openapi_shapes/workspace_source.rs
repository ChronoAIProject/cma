//! Public projection of the prepared Workspace source.
use layer2_core::sandbox::WorkspaceSource;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceSourceView {
    RepositorySnapshot {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_commit: Option<String>,
    },
    GitRepository {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Empty,
}

impl From<&WorkspaceSource> for WorkspaceSourceView {
    fn from(value: &WorkspaceSource) -> Self {
        match value {
            WorkspaceSource::RepositorySnapshot {
                url,
                reference,
                source_commit,
            } => Self::RepositorySnapshot {
                url: url.clone(),
                reference: reference.clone(),
                source_commit: source_commit.clone(),
            },
            WorkspaceSource::PublicRepositorySnapshot {
                url,
                commit,
                source_commit,
            } => Self::RepositorySnapshot {
                url: url.clone(),
                reference: Some(commit.clone()),
                source_commit: source_commit.clone(),
            },
            WorkspaceSource::GitRepository { url, reference } => Self::GitRepository {
                url: url.clone(),
                reference: reference.clone(),
            },
            WorkspaceSource::Empty => Self::Empty,
        }
    }
}
