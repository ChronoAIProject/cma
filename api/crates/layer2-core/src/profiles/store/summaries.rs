use super::*;

/// Publication metadata from a verified immutable revision, not the current draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedProfileSummary {
    pub revision: u32,
    pub digest: String,
    pub published_at: String,
}

/// One owner-visible profile and its latest publication from the same read snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSummary<T> {
    pub profile: T,
    pub latest_revision: Option<PublishedProfileSummary>,
}

macro_rules! publication_summary {
    ($revision:ty) => {
        impl From<&$revision> for PublishedProfileSummary {
            fn from(value: &$revision) -> Self {
                Self {
                    revision: value.revision,
                    digest: value.digest.clone(),
                    published_at: value.published_at.clone(),
                }
            }
        }
    };
}
publication_summary!(AgentProfileRevision);
publication_summary!(WorkspaceProfileRevision);
publication_summary!(SandboxProfileRevision);

macro_rules! memory_summary_page {
    ($method:ident, $profiles:ident, $revisions:ident, $profile:ty) => {
        impl InMemoryProfileStore {
            pub(super) fn $method(
                &self,
                owner: &SubjectId,
                page: &ProfilePageRequest,
            ) -> Result<ProfilePage<ProfileSummary<$profile>>, ProfileStoreError> {
                let state = self.lock();
                let profiles = profile_page(
                    state
                        .$profiles
                        .values()
                        .filter(|profile| !profile.internal && &profile.owner == owner)
                        .map(|profile| (profile.id.to_string(), profile.clone()))
                        .collect(),
                    page,
                )?;
                Ok(ProfilePage {
                    items: profiles
                        .items
                        .into_iter()
                        .map(|profile| {
                            let latest_revision = state
                                .$revisions
                                .iter()
                                .filter(|((id, _), _)| id == &profile.id)
                                .max_by_key(|((_, number), _)| *number)
                                .map(|(_, revision)| PublishedProfileSummary::from(revision));
                            ProfileSummary {
                                profile,
                                latest_revision,
                            }
                        })
                        .collect(),
                    next: profiles.next,
                })
            }
        }
    };
}
memory_summary_page!(agent_summaries, agents, agent_revisions, AgentProfile);
memory_summary_page!(
    workspace_summaries,
    workspaces,
    workspace_revisions,
    WorkspaceProfile
);
memory_summary_page!(
    sandbox_summaries,
    sandboxes,
    sandbox_revisions,
    SandboxProfile
);
