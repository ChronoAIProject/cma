//! Idempotent cross-fork pull-request creation and update.

use layer2_core::{
    CheckSummaryState, ConnectionSnapshot, ForkRecord, Launch, Mergeability, PullRequestOperation,
    PullRequestRecord, PullRequestState, RepositoryIdentity, SubjectId,
};
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};

use super::{
    connection_drifted, validate_commit_sha, validate_repository_rebinding,
    validate_repository_url, GithubBroker, GithubBrokerError, GithubRejectionKind, Observed,
    RepositoryEvidence,
};

const MAX_PULL_REQUEST_PAGES: u32 = 10;
const PULL_REQUEST_PAGE_SIZE: usize = 100;
const CHECK_STATUS_PAGE_SIZE: u64 = 100;
const MAX_MERGEABILITY_POLLS: usize = 3;

#[derive(Debug, Clone)]
pub(crate) struct PullRequestContext {
    owner: SubjectId,
    connection: ConnectionSnapshot,
    upstream: RepositoryIdentity,
    fork: ForkRecord,
    record: PullRequestRecord,
    launch_head_revision: String,
}

impl TryFrom<&Launch> for PullRequestContext {
    type Error = GithubBrokerError;

    fn try_from(launch: &Launch) -> Result<Self, Self::Error> {
        let record = launch.pull_request.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no pull-request intent".into())
        })?;
        record
            .validate_intent(&launch.id)
            .map_err(|error| GithubBrokerError::Malformed(error.to_string()))?;
        if !matches!(
            record.state,
            PullRequestState::Creating
                | PullRequestState::Updating
                | PullRequestState::RecoveryRequired
        ) {
            return Err(GithubBrokerError::Malformed(
                "the pull-request intent is not writable".into(),
            ));
        }
        let contribution = launch.contribution.as_ref().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no published contribution".into())
        })?;
        let published_head = contribution.resulting_head.as_deref().ok_or_else(|| {
            GithubBrokerError::Malformed("the published contribution has no head".into())
        })?;
        if contribution.state != layer2_core::PublicationState::Published
            || !published_head.eq_ignore_ascii_case(&record.intent.expected_head_revision)
            || !record
                .intent
                .expected_base_revision
                .eq_ignore_ascii_case(&launch.source_sha)
            || record.confirmation.subject != launch.owner
            || launch.branch.as_ref().is_none_or(|branch| {
                branch.name != record.intent.head_branch
                    || !branch.current_head.eq_ignore_ascii_case(published_head)
            })
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let upstream = launch.revision.content.upstream.repository.clone();
        let fork = launch.fork.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no reconciled fork".into())
        })?;
        if record.intent.upstream_repository_id != upstream.repository_id
            || record.intent.upstream_repository_node_id != upstream.repository_node_id
            || record.intent.fork_repository_id != fork.repository.repository_id
            || record.intent.fork_repository_node_id != fork.repository.repository_node_id
            || record.intent.base_ref != launch.revision.content.upstream.contribution_base_ref
            || record.intent.head_owner != fork.repository.canonical_owner
            || record.intent.head_branch != format!("cma/{}", launch.id.0)
            || record.intent.head_ref
                != format!(
                    "{}:{}",
                    fork.repository.canonical_owner, record.intent.head_branch
                )
            || (record.intent.operation == PullRequestOperation::Update
                && record.intent.previous_head_revision.as_deref()
                    != Some(contribution.expected_head.as_str()))
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let launch_head_revision = launch
            .branch
            .as_ref()
            .expect("the branch was validated above")
            .current_head
            .clone();
        Ok(Self {
            owner: launch.owner.clone(),
            connection: launch.connection.clone().ok_or_else(|| {
                GithubBrokerError::Malformed("the Launch has no selected connection".into())
            })?,
            upstream,
            fork,
            record,
            launch_head_revision,
        })
    }
}

impl PullRequestContext {
    pub(crate) fn for_status(launch: &Launch) -> Result<Self, GithubBrokerError> {
        if launch.state != layer2_core::LaunchState::PrOpen {
            return Err(GithubBrokerError::Malformed(
                "only a Launch with a linked pull request has provider status to refresh".into(),
            ));
        }
        let record = launch.pull_request.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no pull-request record".into())
        })?;
        record
            .validate_intent(&launch.id)
            .map_err(|error| GithubBrokerError::Malformed(error.to_string()))?;
        if record.state != PullRequestState::Open {
            return Err(GithubBrokerError::Malformed(
                "only an open pull request requires provider status refresh".into(),
            ));
        }
        record.head_revision.as_deref().ok_or_else(|| {
            GithubBrokerError::Malformed("the linked pull request has no observed head".into())
        })?;
        if launch
            .branch
            .as_ref()
            .is_none_or(|branch| branch.name != record.intent.head_branch)
            || launch.contribution.as_ref().is_none_or(|contribution| {
                contribution.state != layer2_core::PublicationState::Published
                    || contribution.resulting_head.is_none()
            })
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let upstream = launch.revision.content.upstream.repository.clone();
        let fork = launch.fork.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the Launch has no reconciled fork".into())
        })?;
        if record.intent.upstream_repository_id != upstream.repository_id
            || record.intent.upstream_repository_node_id != upstream.repository_node_id
            || record.intent.fork_repository_id != fork.repository.repository_id
            || record.intent.fork_repository_node_id != fork.repository.repository_node_id
            || record.intent.base_ref != launch.revision.content.upstream.contribution_base_ref
            || record.intent.head_owner != fork.repository.canonical_owner
            || record.intent.head_branch != format!("cma/{}", launch.id.0)
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let launch_head_revision = launch
            .branch
            .as_ref()
            .expect("the branch was validated above")
            .current_head
            .clone();
        Ok(Self {
            owner: launch.owner.clone(),
            connection: launch.connection.clone().ok_or_else(|| {
                GithubBrokerError::Malformed("the Launch has no selected connection".into())
            })?,
            upstream,
            fork,
            record,
            launch_head_revision,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PullRequestAdvance {
    Absent,
    Ready(Box<PullRequestRecord>),
}

#[derive(Debug)]
struct CurrentRepositories {
    upstream: RepositoryIdentity,
    fork: RepositoryIdentity,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestWire {
    id: u64,
    node_id: String,
    number: u64,
    html_url: String,
    state: String,
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    mergeable: Option<bool>,
    head: PullRequestRefWire,
    base: PullRequestRefWire,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestRefWire {
    #[serde(rename = "ref")]
    reference: String,
    sha: String,
    repo: Option<PullRequestRepositoryWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestRepositoryWire {
    id: u64,
    node_id: String,
    name: String,
    owner: PullRequestOwnerWire,
}

#[derive(Debug, Clone, Deserialize)]
struct PullRequestOwnerWire {
    id: u64,
    node_id: String,
    login: String,
}

#[derive(Debug, Deserialize)]
struct CheckRunsWire {
    total_count: u64,
    #[serde(default)]
    check_runs: Vec<CheckRunWire>,
}

#[derive(Debug, Deserialize)]
struct CheckRunWire {
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRunsWire {
    total_count: u64,
    #[serde(default)]
    workflow_runs: Vec<WorkflowRunWire>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRunWire {
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
}

impl GithubBroker {
    pub(crate) async fn write_pull_request(
        &self,
        context: &PullRequestContext,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        self.advance_pull_request(context, false).await
    }

    pub(crate) async fn reconcile_pull_request(
        &self,
        context: &PullRequestContext,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        self.advance_pull_request(context, true).await
    }

    /// Refresh one already-linked pull request by its immutable numeric and
    /// node identities. This path performs provider reads only and cannot
    /// create, update, reopen, or otherwise mutate a pull request.
    pub(crate) async fn observe_pull_request(
        &self,
        context: &PullRequestContext,
    ) -> Result<PullRequestRecord, GithubBrokerError> {
        let repositories = self.current_pull_request_repositories(context).await?;
        let number = context.record.number.ok_or_else(|| {
            GithubBrokerError::Malformed("the linked pull request has no number".into())
        })?;
        let wire = self
            .pull_request_by_number(context, &repositories, number)
            .await?;
        if context.record.provider_id != Some(wire.id)
            || context.record.provider_node_id.as_deref() != Some(wire.node_id.as_str())
            || context.record.number != Some(wire.number)
            || context.record.canonical_url.as_deref() != Some(wire.html_url.as_str())
            || context.record.provider_created_at.as_deref() != Some(wire.created_at.as_str())
        {
            return Err(GithubBrokerError::PullRequestChanged);
        }
        let observed = self
            .observed_record(context, &repositories, wire, None, false)
            .await?;
        if observed.provider_id != context.record.provider_id
            || observed.provider_node_id != context.record.provider_node_id
            || observed.number != context.record.number
            || observed.canonical_url != context.record.canonical_url
            || observed.provider_created_at != context.record.provider_created_at
        {
            return Err(GithubBrokerError::PullRequestChanged);
        }
        Ok(observed)
    }

    async fn advance_pull_request(
        &self,
        context: &PullRequestContext,
        reconcile_only: bool,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        let repositories = self.current_pull_request_repositories(context).await?;
        let (base_revision, head_revision) = self
            .current_pull_request_revisions(context, &repositories)
            .await?;

        let existing = self.discover_pull_request(context, &repositories).await?;
        match context.record.intent.operation {
            PullRequestOperation::Open => {
                if let Some(existing) = existing {
                    let record = self
                        .reconciled_record(
                            context,
                            &repositories,
                            existing,
                            &base_revision,
                            &head_revision,
                            false,
                        )
                        .await?;
                    return Ok(PullRequestAdvance::Ready(Box::new(record)));
                }
                if reconcile_only {
                    return Ok(PullRequestAdvance::Absent);
                }
                self.create_pull_request(context, &repositories, &base_revision, &head_revision)
                    .await
            }
            PullRequestOperation::Update => {
                let Some(existing) = existing else {
                    return Err(GithubBrokerError::PullRequestChanged);
                };
                self.update_pull_request(
                    context,
                    &repositories,
                    existing,
                    &base_revision,
                    &head_revision,
                    reconcile_only,
                )
                .await
            }
        }
    }

    async fn current_pull_request_repositories(
        &self,
        context: &PullRequestContext,
    ) -> Result<CurrentRepositories, GithubBrokerError> {
        let connection = self
            .preflight_connection(&context.owner, &context.connection.service_id)
            .await?;
        if connection_drifted(&context.connection, &connection.snapshot) {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let upstream = self
            .repository_by_id_with_connection(
                &context.owner,
                &context.connection,
                context.upstream.repository_id,
            )
            .await?;
        let fork = self
            .repository_by_id_with_connection(
                &context.owner,
                &context.connection,
                context.fork.repository.repository_id,
            )
            .await?;
        validate_current_repository(&context.upstream, &upstream, false)?;
        validate_current_repository(&context.fork.repository, &fork, true)?;
        if fork.identity.owner_id != connection.snapshot.account.account_id
            || fork.identity.owner_node_id != connection.snapshot.account.account_node_id
            || fork.identity.fork_parent_id != Some(upstream.identity.repository_id)
            || fork.identity.fork_source_id
                != Some(
                    upstream
                        .identity
                        .fork_source_id
                        .unwrap_or(upstream.identity.repository_id),
                )
            || !fork.permissions.push
            || !upstream.permissions.pull
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        validate_repository_url(&upstream.html_url, &upstream.identity)?;
        validate_repository_url(&fork.html_url, &fork.identity)?;
        Ok(CurrentRepositories {
            upstream: upstream.identity,
            fork: fork.identity,
        })
    }

    async fn current_pull_request_revisions(
        &self,
        context: &PullRequestContext,
        repositories: &CurrentRepositories,
    ) -> Result<(String, String), GithubBrokerError> {
        let base_revision = self
            .resolve_branch_sha_with_connection(
                &context.owner,
                &context.connection,
                &repositories.upstream,
                &context.record.intent.base_ref,
            )
            .await?;
        let head_revision = self
            .resolve_branch_sha_with_connection(
                &context.owner,
                &context.connection,
                &repositories.fork,
                &context.record.intent.head_branch,
            )
            .await?;
        if !base_revision.eq_ignore_ascii_case(&context.record.intent.expected_base_revision) {
            return Err(GithubBrokerError::UpstreamBaseConflict);
        }
        if !head_revision.eq_ignore_ascii_case(&context.record.intent.expected_head_revision) {
            return Err(GithubBrokerError::BranchHeadConflict);
        }
        Ok((base_revision, head_revision))
    }

    async fn discover_pull_request(
        &self,
        context: &PullRequestContext,
        repositories: &CurrentRepositories,
    ) -> Result<Option<PullRequestWire>, GithubBrokerError> {
        let head = format!(
            "{}:{}",
            repositories.fork.canonical_owner, context.record.intent.head_branch
        );
        let mut matched = None;
        for page_number in 1..=MAX_PULL_REQUEST_PAGES {
            let page = page_number.to_string();
            let response: Observed<Vec<PullRequestWire>> = self
                .github_json_with_connection(
                    &context.owner,
                    &context.connection,
                    Method::GET,
                    &[
                        "repos",
                        repositories.upstream.canonical_owner.as_str(),
                        repositories.upstream.canonical_name.as_str(),
                        "pulls",
                    ],
                    &[
                        ("state", "all"),
                        ("head", head.as_str()),
                        ("base", context.record.intent.base_ref.as_str()),
                        ("per_page", "100"),
                        ("page", page.as_str()),
                    ],
                    None,
                )
                .await?;
            let count = response.value.len();
            for candidate in response.value {
                if !matches_pull_request_identity(context, repositories, &candidate) {
                    continue;
                }
                if matched.replace(candidate).is_some() {
                    return Err(GithubBrokerError::AuthorityDrift);
                }
            }
            if count < PULL_REQUEST_PAGE_SIZE {
                break;
            }
            if page_number == MAX_PULL_REQUEST_PAGES {
                return Err(GithubBrokerError::Malformed(
                    "the matching pull-request inventory exceeds its reconciliation bound".into(),
                ));
            }
        }
        if context.record.intent.operation == PullRequestOperation::Update {
            let candidate = matched
                .as_ref()
                .ok_or(GithubBrokerError::PullRequestChanged)?;
            if context.record.intent.target_number != Some(candidate.number)
                || context.record.intent.target_provider_id != Some(candidate.id)
                || context.record.intent.target_provider_node_id.as_deref()
                    != Some(candidate.node_id.as_str())
            {
                return Err(GithubBrokerError::PullRequestChanged);
            }
        }
        Ok(matched)
    }

    async fn create_pull_request(
        &self,
        context: &PullRequestContext,
        _repositories: &CurrentRepositories,
        base_revision: &str,
        head_revision: &str,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        // The search above is only an absence proof. Resolve both numeric
        // repositories again at the write boundary and use these canonical
        // names for the cross-fork POST.
        let repositories = self.current_pull_request_repositories(context).await?;
        self.current_pull_request_revisions(context, &repositories)
            .await?;
        let head = format!(
            "{}:{}",
            repositories.fork.canonical_owner, context.record.intent.head_branch
        );
        let created: Result<Observed<PullRequestWire>, GithubBrokerError> = self
            .github_json_with_connection(
                &context.owner,
                &context.connection,
                Method::POST,
                &[
                    "repos",
                    repositories.upstream.canonical_owner.as_str(),
                    repositories.upstream.canonical_name.as_str(),
                    "pulls",
                ],
                &[],
                Some(json!({
                    "title": context.record.intent.title,
                    "body": context.record.intent.provider_body(),
                    "head": head,
                    "base": context.record.intent.base_ref,
                })),
            )
            .await;
        let created = match created {
            Ok(created) => created.value,
            Err(GithubBrokerError::Rejected {
                status: 422,
                kind: GithubRejectionKind::DuplicatePullRequest,
                ..
            }) => {
                let repositories = self.current_pull_request_repositories(context).await?;
                return match self.discover_pull_request(context, &repositories).await? {
                    Some(existing) => self
                        .reconciled_record(
                            context,
                            &repositories,
                            existing,
                            base_revision,
                            head_revision,
                            false,
                        )
                        .await
                        .map(|record| PullRequestAdvance::Ready(Box::new(record))),
                    None => Err(GithubBrokerError::AmbiguousWrite),
                };
            }
            Err(error) => return Err(error),
        };
        if created.title != context.record.intent.title
            || created.body.as_deref() != Some(context.record.intent.provider_body().as_str())
        {
            return Err(GithubBrokerError::PullRequestChanged);
        }
        self.reconciled_record(
            context,
            &repositories,
            created,
            base_revision,
            head_revision,
            true,
        )
        .await
        .map(|record| PullRequestAdvance::Ready(Box::new(record)))
    }

    async fn update_pull_request(
        &self,
        context: &PullRequestContext,
        repositories: &CurrentRepositories,
        existing: PullRequestWire,
        base_revision: &str,
        head_revision: &str,
        reconcile_only: bool,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        validate_pull_request_wire(
            context,
            repositories,
            &existing,
            base_revision,
            head_revision,
        )?;
        if pull_request_state(&existing)? != PullRequestState::Open {
            return self
                .reconciled_record(
                    context,
                    repositories,
                    existing,
                    base_revision,
                    head_revision,
                    false,
                )
                .await
                .map(|record| PullRequestAdvance::Ready(Box::new(record)));
        }
        if !context.record.intent.update_metadata {
            return self
                .reconciled_record(
                    context,
                    repositories,
                    existing,
                    base_revision,
                    head_revision,
                    false,
                )
                .await
                .map(|record| PullRequestAdvance::Ready(Box::new(record)));
        }
        let desired_body = context.record.intent.provider_body();
        let current_matches = existing.title == context.record.intent.title
            && existing.body.as_deref() == Some(desired_body.as_str());
        let (updated, rebound_repositories) = if current_matches {
            (existing, None)
        } else {
            let current_body_digest = format!(
                "{:x}",
                Sha256::digest(existing.body.as_deref().unwrap_or_default().as_bytes())
            );
            if context.record.intent.expected_previous_title.as_deref()
                != Some(existing.title.as_str())
                || context
                    .record
                    .intent
                    .expected_previous_body_digest
                    .as_deref()
                    != Some(current_body_digest.as_str())
            {
                return Err(GithubBrokerError::PullRequestChanged);
            }
            if reconcile_only {
                return Ok(PullRequestAdvance::Absent);
            }
            let number = context
                .record
                .intent
                .target_number
                .expect("a validated update has a target number")
                .to_string();
            // Rebind after discovery and optimistic metadata checks. A rename
            // can change only the address; a transfer or ID/node mismatch is
            // rejected by `current_pull_request_repositories`.
            let repositories = self.current_pull_request_repositories(context).await?;
            self.current_pull_request_revisions(context, &repositories)
                .await?;
            let response: Observed<PullRequestWire> = self
                .github_json_with_connection(
                    &context.owner,
                    &context.connection,
                    Method::PATCH,
                    &[
                        "repos",
                        repositories.upstream.canonical_owner.as_str(),
                        repositories.upstream.canonical_name.as_str(),
                        "pulls",
                        number.as_str(),
                    ],
                    &[],
                    Some(json!({
                        "title": context.record.intent.title,
                        "body": desired_body,
                    })),
                )
                .await?;
            if response.value.title != context.record.intent.title
                || response.value.body.as_deref()
                    != Some(context.record.intent.provider_body().as_str())
            {
                return Err(GithubBrokerError::PullRequestChanged);
            }
            (response.value, Some(repositories))
        };
        let repositories = rebound_repositories.as_ref().unwrap_or(repositories);
        self.reconciled_record(
            context,
            repositories,
            updated,
            base_revision,
            head_revision,
            true,
        )
        .await
        .map(|record| PullRequestAdvance::Ready(Box::new(record)))
    }

    async fn reconciled_record(
        &self,
        context: &PullRequestContext,
        repositories: &CurrentRepositories,
        wire: PullRequestWire,
        base_revision: &str,
        head_revision: &str,
        require_intended_content: bool,
    ) -> Result<PullRequestRecord, GithubBrokerError> {
        self.observed_record(
            context,
            repositories,
            wire,
            Some((base_revision, head_revision)),
            require_intended_content,
        )
        .await
    }

    async fn observed_record(
        &self,
        context: &PullRequestContext,
        repositories: &CurrentRepositories,
        mut wire: PullRequestWire,
        expected_revisions: Option<(&str, &str)>,
        require_intended_content: bool,
    ) -> Result<PullRequestRecord, GithubBrokerError> {
        validate_observed_pull_request_wire(context, repositories, &wire)?;
        if let Some((base_revision, head_revision)) = expected_revisions {
            validate_pull_request_wire(context, repositories, &wire, base_revision, head_revision)?;
        }
        if require_intended_content
            && (wire.title != context.record.intent.title
                || wire.body.as_deref() != Some(context.record.intent.provider_body().as_str()))
        {
            return Err(GithubBrokerError::PullRequestChanged);
        }
        for _ in 0..MAX_MERGEABILITY_POLLS {
            if wire.mergeable.is_some() || pull_request_state(&wire)? != PullRequestState::Open {
                break;
            }
            wire = self
                .pull_request_by_number(context, repositories, wire.number)
                .await?;
            validate_observed_pull_request_wire(context, repositories, &wire)?;
            if let Some((base_revision, head_revision)) = expected_revisions {
                validate_pull_request_wire(
                    context,
                    repositories,
                    &wire,
                    base_revision,
                    head_revision,
                )?;
            }
        }
        let base_revision = wire.base.sha.to_ascii_lowercase();
        let head_revision = wire.head.sha.to_ascii_lowercase();
        let checks = self
            .pull_request_checks(context, repositories, &head_revision)
            .await;
        let mut record = context.record.clone();
        record.provider_id = Some(wire.id);
        record.provider_node_id = Some(wire.node_id.clone());
        record.number = Some(wire.number);
        record.canonical_url = Some(wire.html_url.clone());
        record.state = pull_request_state(&wire)?;
        record.base_revision = Some(base_revision);
        record.head_revision = Some(head_revision);
        record.observed_title = Some(wire.title.clone());
        record.observed_body_digest = Some(format!(
            "{:x}",
            Sha256::digest(wire.body.as_deref().unwrap_or_default().as_bytes())
        ));
        record.mergeability = match wire.mergeable {
            Some(true) => Mergeability::Mergeable,
            Some(false) => Mergeability::Conflicting,
            None => Mergeability::Computing,
        };
        record.checks = checks;
        record.updated_at = layer2_core::rfc3339_at(std::time::SystemTime::now());
        record.provider_created_at = Some(wire.created_at);
        record.provider_updated_at = Some(wire.updated_at);
        record.status_sync.head_matches_launch = Some(
            record
                .head_revision
                .as_deref()
                .is_some_and(|head| head.eq_ignore_ascii_case(&context.launch_head_revision)),
        );
        record
            .validate_intent(&record.intent.launch_id)
            .map_err(|error| GithubBrokerError::Malformed(error.to_string()))?;
        Ok(record)
    }

    async fn pull_request_by_number(
        &self,
        context: &PullRequestContext,
        repositories: &CurrentRepositories,
        number: u64,
    ) -> Result<PullRequestWire, GithubBrokerError> {
        let expected_number = number;
        let number = expected_number.to_string();
        let response: Observed<PullRequestWire> = self
            .github_json_with_connection(
                &context.owner,
                &context.connection,
                Method::GET,
                &[
                    "repos",
                    repositories.upstream.canonical_owner.as_str(),
                    repositories.upstream.canonical_name.as_str(),
                    "pulls",
                    number.as_str(),
                ],
                &[],
                None,
            )
            .await?;
        if response.value.number != expected_number {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        Ok(response.value)
    }

    async fn pull_request_checks(
        &self,
        context: &PullRequestContext,
        repositories: &CurrentRepositories,
        head_revision: &str,
    ) -> CheckSummaryState {
        let checks: Result<Observed<CheckRunsWire>, GithubBrokerError> = self
            .github_json_with_connection(
                &context.owner,
                &context.connection,
                Method::GET,
                &[
                    "repos",
                    repositories.upstream.canonical_owner.as_str(),
                    repositories.upstream.canonical_name.as_str(),
                    "commits",
                    head_revision,
                    "check-runs",
                ],
                &[("per_page", "100")],
                None,
            )
            .await;
        let runs: Result<Observed<WorkflowRunsWire>, GithubBrokerError> = self
            .github_json_with_connection(
                &context.owner,
                &context.connection,
                Method::GET,
                &[
                    "repos",
                    repositories.upstream.canonical_owner.as_str(),
                    repositories.upstream.canonical_name.as_str(),
                    "actions",
                    "runs",
                ],
                &[("head_sha", head_revision), ("per_page", "100")],
                None,
            )
            .await;
        summarize_check_sources(
            checks.map(|observed| observed.value),
            runs.map(|observed| observed.value),
        )
    }
}

fn validate_current_repository(
    expected: &RepositoryIdentity,
    observed: &RepositoryEvidence,
    fork: bool,
) -> Result<(), GithubBrokerError> {
    validate_repository_rebinding(expected, observed)?;
    if fork
        && (observed.identity.fork_parent_id.is_none()
            || observed.identity.fork_source_id.is_none())
    {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    Ok(())
}

fn matches_pull_request_identity(
    context: &PullRequestContext,
    repositories: &CurrentRepositories,
    wire: &PullRequestWire,
) -> bool {
    marker_count(
        wire.body.as_deref().unwrap_or_default(),
        &context.record.intent.marker,
    ) == 1
        && wire.base.reference == context.record.intent.base_ref
        && wire.head.reference == context.record.intent.head_branch
        && wire
            .base
            .repo
            .as_ref()
            .is_some_and(|repository| same_wire_repository(repository, &repositories.upstream))
        && wire
            .head
            .repo
            .as_ref()
            .is_some_and(|repository| same_wire_repository(repository, &repositories.fork))
}

fn validate_pull_request_wire(
    context: &PullRequestContext,
    repositories: &CurrentRepositories,
    wire: &PullRequestWire,
    expected_base: &str,
    expected_head: &str,
) -> Result<(), GithubBrokerError> {
    validate_observed_pull_request_wire(context, repositories, wire)?;
    if !wire.base.sha.eq_ignore_ascii_case(expected_base)
        || !wire.head.sha.eq_ignore_ascii_case(expected_head)
    {
        return Err(GithubBrokerError::PullRequestChanged);
    }
    Ok(())
}

fn validate_observed_pull_request_wire(
    context: &PullRequestContext,
    repositories: &CurrentRepositories,
    wire: &PullRequestWire,
) -> Result<(), GithubBrokerError> {
    validate_commit_sha(&wire.head.sha)?;
    validate_commit_sha(&wire.base.sha)?;
    if wire.id == 0
        || wire.number == 0
        || wire.node_id.trim().is_empty()
        || wire.node_id.len() > 255
        || !matches_pull_request_identity(context, repositories, wire)
        || wire.title.trim().is_empty()
        || wire.title.len() > 256
        || wire.created_at.trim().is_empty()
        || wire.updated_at.trim().is_empty()
    {
        return Err(GithubBrokerError::PullRequestChanged);
    }
    validate_pull_request_url(&wire.html_url, repositories, wire.number)
}

fn validate_pull_request_url(
    value: &str,
    repositories: &CurrentRepositories,
    number: u64,
) -> Result<(), GithubBrokerError> {
    let url = Url::parse(value)
        .map_err(|_| GithubBrokerError::Malformed("the pull-request URL is invalid".into()))?;
    let expected_path = format!(
        "/{}/{}/pull/{number}",
        repositories.upstream.canonical_owner, repositories.upstream.canonical_name
    );
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != expected_path
    {
        return Err(GithubBrokerError::Malformed(
            "the pull-request URL does not match its numeric repository".into(),
        ));
    }
    Ok(())
}

fn same_wire_repository(wire: &PullRequestRepositoryWire, expected: &RepositoryIdentity) -> bool {
    wire.id == expected.repository_id
        && wire.node_id == expected.repository_node_id
        && wire.name == expected.canonical_name
        && wire.owner.id == expected.owner_id
        && wire.owner.node_id == expected.owner_node_id
        && wire.owner.login == expected.canonical_owner
}

fn pull_request_state(wire: &PullRequestWire) -> Result<PullRequestState, GithubBrokerError> {
    if wire.merged_at.is_some() {
        return Ok(PullRequestState::Merged);
    }
    match wire.state.as_str() {
        "open" => Ok(PullRequestState::Open),
        "closed" => Ok(PullRequestState::ClosedUnmerged),
        _ => Err(GithubBrokerError::Malformed(
            "GitHub returned an unknown pull-request state".into(),
        )),
    }
}

fn marker_count(body: &str, marker: &str) -> usize {
    body.match_indices(marker).count()
}

fn summarize_check_sources(
    checks: Result<CheckRunsWire, GithubBrokerError>,
    workflows: Result<WorkflowRunsWire, GithubBrokerError>,
) -> CheckSummaryState {
    let (Ok(checks), Ok(workflows)) = (checks, workflows) else {
        return CheckSummaryState::Unavailable;
    };
    if !complete_check_inventory(checks.total_count, checks.check_runs.len())
        || !complete_check_inventory(workflows.total_count, workflows.workflow_runs.len())
    {
        return CheckSummaryState::Unavailable;
    }
    summarize_checks(checks.check_runs, workflows.workflow_runs)
}

fn complete_check_inventory(total_count: u64, returned_count: usize) -> bool {
    total_count <= CHECK_STATUS_PAGE_SIZE
        && u64::try_from(returned_count).is_ok_and(|returned| returned == total_count)
}

fn summarize_checks(
    checks: Vec<CheckRunWire>,
    workflows: Vec<WorkflowRunWire>,
) -> CheckSummaryState {
    let mut observed = false;
    let mut pending = false;
    let mut failing = false;
    let mut approval = false;
    for (status, conclusion) in checks
        .iter()
        .map(|run| (run.status.as_str(), run.conclusion.as_deref()))
        .chain(
            workflows
                .iter()
                .map(|run| (run.status.as_str(), run.conclusion.as_deref())),
        )
    {
        observed = true;
        if conclusion == Some("action_required") {
            approval = true;
        } else if status != "completed" || conclusion.is_none() {
            pending = true;
        } else if !matches!(conclusion, Some("success" | "neutral" | "skipped")) {
            failing = true;
        }
    }
    if approval {
        CheckSummaryState::AwaitingMaintainerApproval
    } else if failing {
        CheckSummaryState::Failing
    } else if pending {
        CheckSummaryState::Pending
    } else if observed {
        CheckSummaryState::Passing
    } else {
        CheckSummaryState::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use axum::body::to_bytes;
    use axum::extract::{Request, State};
    use axum::http::StatusCode;
    use axum::routing::any;
    use axum::{Json, Router};
    use layer2_core::{
        BranchRecord, CommitIdentity, Confirmation, ConfirmationAction, ContributionId,
        ContributionRecord, DiffArtifactId, ForkRecord, LaunchId, PublicationState,
        PullRequestDraft, RepositoryVisibility,
    };
    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    use super::*;
    use crate::trigger::github::{
        SubjectAuthoritySource, SubjectRequestAuthority, GITHUB_API_ORIGIN, GITHUB_SERVICE_SLUG,
        NYXID_PROXY_SERVICE_PAGE_SIZE, PUBLIC_REPOSITORY_SCOPE,
    };

    const PROVIDER_GITHUB_CATALOG_ID: &str = "123e4567-e89b-42d3-a456-426614174200";
    const PROVIDER_GITHUB_PROVIDER_ID: &str = "123e4567-e89b-42d3-a456-426614174100";

    #[derive(Debug, Clone, Copy)]
    enum CreateBehavior {
        Confirmed,
        ConcurrentDuplicate,
        BranchProtected,
        AmbiguousApplied,
        DuplicateApplied,
        ValidationRejected,
    }

    #[derive(Debug, Clone)]
    struct ProviderPullRequest {
        title: String,
        body: String,
        state: String,
        merged_at: Option<String>,
        head_sha: String,
        mergeable: Option<bool>,
    }

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        method: String,
        uri: String,
        body: Option<Value>,
    }

    #[derive(Debug, Clone)]
    enum ReboundAuthority {
        Unchanged,
        ServiceInactive,
        ServiceStateVersion(i64),
        BackingKey(String),
        GrantedScopes(Vec<String>),
        AccountIdentity {
            id: u64,
            node_id: String,
            login: String,
        },
        AccountLogin(String),
    }

    #[derive(Clone)]
    struct ProviderFixture {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        nyxid_base_url: String,
        pull_request: Arc<Mutex<Option<ProviderPullRequest>>>,
        create_behavior: CreateBehavior,
        create_barrier: Option<Arc<tokio::sync::Barrier>>,
        fork_name: String,
        upstream_repository_lookups: Arc<AtomicUsize>,
        fork_repository_lookups: Arc<AtomicUsize>,
        upstream_ref_lookups: Arc<AtomicUsize>,
        fork_ref_lookups: Arc<AtomicUsize>,
        connection_preflights: Arc<AtomicUsize>,
        rebound_authority: ReboundAuthority,
        rebound_upstream_name: Option<String>,
        rebound_fork_name: Option<String>,
        rebound_fork_owner_id: Option<u64>,
        rebound_fork_owner_login: Option<String>,
        rebound_base_head: Option<String>,
        rebound_branch_head: Option<String>,
        branch_head: String,
        base_head: String,
        patch_ambiguous_once: Arc<AtomicBool>,
        patch_unapplied_ambiguous_once: Arc<AtomicBool>,
        workflow_approval_pending: bool,
    }

    impl Default for ProviderFixture {
        fn default() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                nyxid_base_url: String::new(),
                pull_request: Arc::new(Mutex::new(None)),
                create_behavior: CreateBehavior::Confirmed,
                create_barrier: None,
                fork_name: "project".into(),
                upstream_repository_lookups: Arc::new(AtomicUsize::new(0)),
                fork_repository_lookups: Arc::new(AtomicUsize::new(0)),
                upstream_ref_lookups: Arc::new(AtomicUsize::new(0)),
                fork_ref_lookups: Arc::new(AtomicUsize::new(0)),
                connection_preflights: Arc::new(AtomicUsize::new(0)),
                rebound_authority: ReboundAuthority::Unchanged,
                rebound_upstream_name: None,
                rebound_fork_name: None,
                rebound_fork_owner_id: None,
                rebound_fork_owner_login: None,
                rebound_base_head: None,
                rebound_branch_head: None,
                branch_head: "d".repeat(40),
                base_head: "a".repeat(40),
                patch_ambiguous_once: Arc::new(AtomicBool::new(false)),
                patch_unapplied_ambiguous_once: Arc::new(AtomicBool::new(false)),
                workflow_approval_pending: false,
            }
        }
    }

    impl ProviderFixture {
        fn authority_rebound(&self) -> bool {
            // Exact authorization is intentionally read before active service
            // membership. One completed service read therefore means the next
            // authorization response belongs to the write-boundary preflight.
            self.connection_preflights.load(Ordering::SeqCst) > 0
        }

        fn observe_connection_preflight(&self) -> bool {
            self.connection_preflights.fetch_add(1, Ordering::SeqCst) > 0
        }

        fn service_is_active(&self, rebound: bool) -> bool {
            !(rebound && matches!(&self.rebound_authority, ReboundAuthority::ServiceInactive))
        }

        fn service_state_version(&self, rebound: bool) -> i64 {
            if rebound {
                if let ReboundAuthority::ServiceStateVersion(version) = &self.rebound_authority {
                    return *version;
                }
            }
            2
        }

        fn backing_key_id(&self, rebound: bool) -> &str {
            if rebound {
                if let ReboundAuthority::BackingKey(key) = &self.rebound_authority {
                    return key;
                }
            }
            "key-1"
        }

        fn granted_scopes(&self) -> Vec<String> {
            if self.authority_rebound() {
                if let ReboundAuthority::GrantedScopes(scopes) = &self.rebound_authority {
                    return scopes.clone();
                }
            }
            vec![PUBLIC_REPOSITORY_SCOPE.into()]
        }

        fn current_account(&self) -> (u64, &str, &str) {
            if self.authority_rebound() {
                match &self.rebound_authority {
                    ReboundAuthority::AccountIdentity { id, node_id, login } => {
                        return (*id, node_id, login)
                    }
                    ReboundAuthority::AccountLogin(login) => return (21, "U_contributor", login),
                    _ => {}
                }
            }
            (21, "U_contributor", "contributor")
        }

        fn current_upstream_name(&self) -> &str {
            if self.upstream_repository_lookups.load(Ordering::SeqCst) > 1 {
                self.rebound_upstream_name.as_deref().unwrap_or("project")
            } else {
                "project"
            }
        }

        fn observe_upstream_name(&self) -> &str {
            let prior = self
                .upstream_repository_lookups
                .fetch_add(1, Ordering::SeqCst);
            if prior > 0 {
                self.rebound_upstream_name.as_deref().unwrap_or("project")
            } else {
                "project"
            }
        }

        fn current_fork_name(&self) -> &str {
            if self.fork_repository_lookups.load(Ordering::SeqCst) > 1 {
                self.rebound_fork_name.as_deref().unwrap_or(&self.fork_name)
            } else {
                &self.fork_name
            }
        }

        fn current_fork_owner_id(&self) -> u64 {
            if self.fork_repository_lookups.load(Ordering::SeqCst) > 1 {
                self.rebound_fork_owner_id.unwrap_or(21)
            } else {
                21
            }
        }

        fn current_fork_owner_login(&self) -> &str {
            if self.fork_repository_lookups.load(Ordering::SeqCst) > 1 {
                self.rebound_fork_owner_login
                    .as_deref()
                    .unwrap_or("contributor")
            } else {
                "contributor"
            }
        }

        fn observe_fork_binding(&self) -> (&str, u64) {
            let prior = self.fork_repository_lookups.fetch_add(1, Ordering::SeqCst);
            if prior > 0 {
                (
                    self.rebound_fork_name.as_deref().unwrap_or(&self.fork_name),
                    self.rebound_fork_owner_id.unwrap_or(21),
                )
            } else {
                (&self.fork_name, 21)
            }
        }

        fn observe_base_head(&self) -> &str {
            if self.upstream_ref_lookups.fetch_add(1, Ordering::SeqCst) > 0 {
                self.rebound_base_head.as_deref().unwrap_or(&self.base_head)
            } else {
                &self.base_head
            }
        }

        fn observe_branch_head(&self) -> &str {
            if self.fork_ref_lookups.fetch_add(1, Ordering::SeqCst) > 0 {
                self.rebound_branch_head
                    .as_deref()
                    .unwrap_or(&self.branch_head)
            } else {
                &self.branch_head
            }
        }
    }

    #[derive(Clone)]
    struct ProviderAuthority;

    #[async_trait]
    impl SubjectAuthoritySource for ProviderAuthority {
        async fn current(
            &self,
            _subject: &SubjectId,
        ) -> Result<Option<SubjectRequestAuthority>, GithubBrokerError> {
            Ok(Some(SubjectRequestAuthority {
                bearer: "subject-token".into(),
                token_generation: 7,
            }))
        }
    }

    async fn provider_answer(
        State(fixture): State<ProviderFixture>,
        request: Request,
    ) -> (StatusCode, Json<Value>) {
        let (parts, body) = request.into_parts();
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        let body = (!bytes.is_empty()).then(|| serde_json::from_slice(&bytes).unwrap());
        let method = parts.method.as_str().to_string();
        let uri = parts.uri.to_string();
        let original_path = parts.uri.path();
        let path = original_path
            .strip_prefix("/api/v1/proxy/")
            .and_then(|remainder| remainder.split_once('/'))
            .filter(|(route, _)| *route != "s" && *route != "services")
            .map_or_else(
                || original_path.to_string(),
                |(_, suffix)| format!("/api/v1/proxy/s/api-github/{suffix}"),
            );
        if !matches!(
            original_path,
            "/api/v1/catalog/api-github" | "/api/v1/proxy/services"
        ) {
            fixture.requests.lock().await.push(RecordedRequest {
                method: method.clone(),
                uri,
                body: body.clone(),
            });
        }

        let upstream_name = fixture.current_upstream_name().to_string();
        let fork_name = fixture.current_fork_name().to_string();
        let fork_owner = fixture.current_fork_owner_login().to_string();
        let fork_ref_path = format!(
            "/api/v1/proxy/s/api-github/repos/{fork_owner}/{}/git/ref/heads/{}",
            fork_name,
            provider_launch_id_branch()
        );
        let upstream_prefix = format!("/api/v1/proxy/s/api-github/repos/upstream/{upstream_name}");
        let upstream_ref_path = format!("{upstream_prefix}/git/ref/heads/main");
        let pulls_path = format!("{upstream_prefix}/pulls");
        let pull_path = format!("{pulls_path}/7");
        let check_d_path = format!(
            "{upstream_prefix}/commits/dddddddddddddddddddddddddddddddddddddddd/check-runs"
        );
        let check_e_path = format!(
            "{upstream_prefix}/commits/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee/check-runs"
        );
        let actions_path = format!("{upstream_prefix}/actions/runs");
        match (method.as_str(), path.as_str()) {
            ("GET", "/api/v1/catalog/api-github") => (
                StatusCode::OK,
                Json(json!({
                    "slug": GITHUB_SERVICE_SLUG,
                    "resource_uri": format!(
                        "{}/api/v1/proxy/s/{GITHUB_SERVICE_SLUG}",
                        fixture.nyxid_base_url
                    ),
                    "base_url": GITHUB_API_ORIGIN,
                    "auth_method": "bearer",
                    "auth_key_name": "Authorization",
                    "provider_config_id": PROVIDER_GITHUB_PROVIDER_ID,
                    "provider_type": "oauth2"
                })),
            ),
            ("GET", "/api/v1/proxy/services") => (
                StatusCode::OK,
                Json(json!({
                    "services": [{
                        "id": PROVIDER_GITHUB_CATALOG_ID,
                        "slug": GITHUB_SERVICE_SLUG
                    }],
                    "custom_services": [],
                    "total": 1,
                    "page": 1,
                    "per_page": NYXID_PROXY_SERVICE_PAGE_SIZE
                })),
            ),
            ("GET", "/api/v1/user-services") => {
                let rebound = fixture.observe_connection_preflight();
                (
                    StatusCode::OK,
                    Json(json!({
                        "services": [{
                            "id": "service-1",
                            "slug": "api-github",
                            "catalog_service_id": PROVIDER_GITHUB_CATALOG_ID,
                            "resource_uri": format!(
                                "{}/api/v1/proxy/s/api-github",
                                fixture.nyxid_base_url
                            ),
                            "api_key_id": fixture.backing_key_id(rebound),
                            "is_active": fixture.service_is_active(rebound),
                            "state_version": fixture.service_state_version(rebound),
                            "credential_source": { "type": "personal" }
                        }]
                    })),
                )
            }
            ("GET", "/api/v1/keys/service-1/authorization") => {
                let rebound = fixture.authority_rebound();
                let active = fixture.service_is_active(rebound);
                (
                    StatusCode::OK,
                    Json(json!({
                        "id": "service-1",
                        "api_key_id": fixture.backing_key_id(rebound),
                        "is_active": active,
                        "status": if active { "active" } else { "inactive" },
                        "connection_status": if active { "active" } else { "inactive" },
                        "granted_scopes": fixture.granted_scopes(),
                        "last_authorized_at": "2026-08-27T00:00:00Z",
                        "state_version": fixture.service_state_version(rebound)
                    })),
                )
            }
            ("GET", "/api/v1/proxy/s/api-github/user") => {
                let (id, node_id, login) = fixture.current_account();
                (
                    StatusCode::OK,
                    Json(json!({
                        "id": id,
                        "node_id": node_id,
                        "login": login
                    })),
                )
            }
            ("GET", "/api/v1/proxy/s/api-github/repositories/10") => (
                StatusCode::OK,
                Json(repository_wire(
                    10,
                    "R_upstream",
                    (20, "U_owner", "upstream"),
                    fixture.observe_upstream_name(),
                    None,
                    false,
                )),
            ),
            ("GET", "/api/v1/proxy/s/api-github/repositories/30") => {
                let (name, owner_id) = fixture.observe_fork_binding();
                let current_owner = fixture.current_fork_owner_login();
                let owner = if owner_id == 21 {
                    (21, "U_contributor", current_owner)
                } else {
                    (owner_id, "U_transferred", "transferred-owner")
                };
                (
                    StatusCode::OK,
                    Json(repository_wire(30, "R_fork", owner, name, Some(10), true)),
                )
            }
            ("GET", actual) if actual == upstream_ref_path => (
                StatusCode::OK,
                Json(ref_wire("main", fixture.observe_base_head())),
            ),
            ("GET", actual) if actual == fork_ref_path => (
                StatusCode::OK,
                Json(ref_wire(
                    &provider_launch_id_branch(),
                    fixture.observe_branch_head(),
                )),
            ),
            ("GET", actual) if actual == pulls_path => {
                let value = fixture
                    .pull_request
                    .lock()
                    .await
                    .as_ref()
                    .map(|pull_request| vec![pull_request_wire(&fixture, pull_request)])
                    .unwrap_or_default();
                (StatusCode::OK, Json(Value::Array(value)))
            }
            ("GET", actual) if actual == pull_path => {
                match fixture.pull_request.lock().await.as_ref() {
                    Some(pull_request) => (
                        StatusCode::OK,
                        Json(pull_request_wire(&fixture, pull_request)),
                    ),
                    None => (
                        StatusCode::NOT_FOUND,
                        Json(json!({ "message": "Not Found" })),
                    ),
                }
            }
            ("POST", actual) if actual == pulls_path => {
                if matches!(fixture.create_behavior, CreateBehavior::ValidationRejected) {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(json!({
                            "message": "Validation Failed",
                            "errors": [{
                                "resource": "PullRequest",
                                "field": "base",
                                "code": "invalid"
                            }]
                        })),
                    );
                }
                if matches!(fixture.create_behavior, CreateBehavior::BranchProtected) {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({
                            "message": "Changes must be made through a pull request because this is a protected branch",
                            "errors": [{
                                "resource": "PullRequest",
                                "field": "base",
                                "code": "protected_branch"
                            }]
                        })),
                    );
                }
                let body = body.expect("create request body");
                let pull_request = ProviderPullRequest {
                    title: body["title"].as_str().unwrap().to_string(),
                    body: body["body"].as_str().unwrap().to_string(),
                    state: "open".into(),
                    merged_at: None,
                    head_sha: fixture.branch_head.clone(),
                    mergeable: Some(true),
                };
                if matches!(fixture.create_behavior, CreateBehavior::ConcurrentDuplicate) {
                    fixture
                        .create_barrier
                        .as_ref()
                        .expect("the concurrent fixture has a barrier")
                        .wait()
                        .await;
                    let mut stored = fixture.pull_request.lock().await;
                    if stored.is_some() {
                        return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            Json(json!({
                                "message": "Validation Failed",
                                "errors": [{
                                    "resource": "PullRequest",
                                    "field": "head",
                                    "code": "custom",
                                    "message": "A pull request already exists for contributor:branch."
                                }]
                            })),
                        );
                    }
                    *stored = Some(pull_request.clone());
                    drop(stored);
                    return (
                        StatusCode::CREATED,
                        Json(pull_request_wire(&fixture, &pull_request)),
                    );
                }
                *fixture.pull_request.lock().await = Some(pull_request.clone());
                match fixture.create_behavior {
                    CreateBehavior::Confirmed => (
                        StatusCode::CREATED,
                        Json(pull_request_wire(&fixture, &pull_request)),
                    ),
                    CreateBehavior::AmbiguousApplied => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "message": "response lost after create" })),
                    ),
                    CreateBehavior::DuplicateApplied => (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(json!({
                            "message": "Validation Failed",
                            "errors": [{
                                "resource": "PullRequest",
                                "field": "head",
                                "code": "custom",
                                "message": "A pull request already exists for contributor:branch."
                            }]
                        })),
                    ),
                    CreateBehavior::ValidationRejected => unreachable!(),
                    CreateBehavior::ConcurrentDuplicate | CreateBehavior::BranchProtected => {
                        unreachable!()
                    }
                }
            }
            ("PATCH", actual) if actual == pull_path => {
                if fixture
                    .patch_unapplied_ambiguous_once
                    .swap(false, Ordering::SeqCst)
                {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "message": "request outcome unavailable" })),
                    );
                }
                let body = body.expect("update request body");
                let mut pull_request = fixture.pull_request.lock().await;
                let pull_request = pull_request.as_mut().expect("existing PR");
                pull_request.title = body["title"].as_str().unwrap().to_string();
                pull_request.body = body["body"].as_str().unwrap().to_string();
                let response = pull_request_wire(&fixture, pull_request);
                if fixture.patch_ambiguous_once.swap(false, Ordering::SeqCst) {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "message": "response lost after update" })),
                    )
                } else {
                    (StatusCode::OK, Json(response))
                }
            }
            ("GET", actual) if actual == check_d_path || actual == check_e_path => (
                StatusCode::OK,
                Json(json!({ "total_count": 0, "check_runs": [] })),
            ),
            ("GET", actual) if actual == actions_path => {
                let runs = if fixture.workflow_approval_pending {
                    vec![json!({
                        "status": "completed",
                        "conclusion": "action_required"
                    })]
                } else {
                    Vec::new()
                };
                (
                    StatusCode::OK,
                    Json(json!({ "total_count": runs.len(), "workflow_runs": runs })),
                )
            }
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": format!("unexpected fixture request: {method} {path}") })),
            ),
        }
    }

    fn repository_wire(
        id: u64,
        node_id: &str,
        owner: (u64, &str, &str),
        name: &str,
        parent: Option<u64>,
        push: bool,
    ) -> Value {
        let (owner_id, owner_node_id, owner) = owner;
        json!({
            "id": id,
            "node_id": node_id,
            "name": name,
            "owner": { "id": owner_id, "node_id": owner_node_id, "login": owner },
            "private": false,
            "visibility": "public",
            "default_branch": "main",
            "allow_forking": true,
            "archived": false,
            "disabled": false,
            "html_url": format!("https://github.com/{owner}/{name}"),
            "permissions": { "pull": true, "push": push },
            "parent": parent.map(|id| json!({ "id": id })),
            "source": parent.map(|id| json!({ "id": id }))
        })
    }

    fn ref_wire(reference: &str, sha: &str) -> Value {
        json!({
            "ref": format!("refs/heads/{reference}"),
            "object": { "sha": sha, "type": "commit" }
        })
    }

    fn pull_request_wire(fixture: &ProviderFixture, pull_request: &ProviderPullRequest) -> Value {
        let upstream_name = fixture.current_upstream_name();
        let fork_name = fixture.current_fork_name();
        let fork_owner_id = fixture.current_fork_owner_id();
        let (fork_owner_node_id, fork_owner) = if fork_owner_id == 21 {
            ("U_contributor", fixture.current_fork_owner_login())
        } else {
            ("U_transferred", "transferred-owner")
        };
        json!({
            "id": 70,
            "node_id": "PR_node_7",
            "number": 7,
            "html_url": format!("https://github.com/upstream/{upstream_name}/pull/7"),
            "state": pull_request.state,
            "title": pull_request.title,
            "body": pull_request.body,
            "merged_at": pull_request.merged_at,
            "mergeable": pull_request.mergeable,
            "head": {
                "ref": provider_launch_id_branch(),
                "sha": pull_request.head_sha,
                "repo": {
                    "id": 30,
                    "node_id": "R_fork",
                    "name": fork_name,
                    "owner": {
                        "id": fork_owner_id,
                        "node_id": fork_owner_node_id,
                        "login": fork_owner
                    }
                }
            },
            "base": {
                "ref": "main",
                "sha": "a".repeat(40),
                "repo": {
                    "id": 10,
                    "node_id": "R_upstream",
                    "name": upstream_name,
                    "owner": {
                        "id": 20,
                        "node_id": "U_owner",
                        "login": "upstream"
                    }
                }
            },
            "created_at": "2026-08-27T00:00:04Z",
            "updated_at": "2026-08-27T00:00:05Z"
        })
    }

    async fn provider_server(
        mut fixture: ProviderFixture,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let base_url = format!("http://{address}");
        fixture.nyxid_base_url = base_url.clone();
        let app = Router::new()
            .fallback(any(provider_answer))
            .with_state(fixture);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (base_url, server)
    }

    fn provider_broker(base_url: &str) -> GithubBroker {
        GithubBroker::with_authority(base_url, Arc::new(ProviderAuthority)).unwrap()
    }

    fn provider_launch_id_branch() -> String {
        format!("cma/{}", provider_launch_id().0)
    }

    fn provider_launch_id() -> LaunchId {
        LaunchId("123e4567-e89b-12d3-a456-426614174111".into())
    }

    fn provider_launch(operation: PullRequestOperation) -> Launch {
        provider_launch_with_metadata_update(operation, false)
    }

    fn provider_launch_with_metadata_update(
        operation: PullRequestOperation,
        update_metadata: bool,
    ) -> Launch {
        let mut launch = crate::trigger::worker::tests::launch_fixture();
        let connection = launch
            .connection
            .as_mut()
            .expect("provider Launch has a selected connection");
        connection.github_catalog_service_id = Some(PROVIDER_GITHUB_CATALOG_ID.into());
        connection.github_user_service_slug = Some(GITHUB_SERVICE_SLUG.into());
        launch.id = provider_launch_id();
        launch.fork = Some(ForkRecord {
            repository: RepositoryIdentity {
                repository_id: 30,
                repository_node_id: "R_fork".into(),
                owner_id: 21,
                owner_node_id: "U_contributor".into(),
                canonical_owner: "contributor".into(),
                canonical_name: "project".into(),
                visibility: RepositoryVisibility::Public,
                allow_forking: true,
                default_branch: "main".into(),
                selected_ref: "main".into(),
                fork_parent_id: Some(10),
                fork_source_id: Some(10),
            },
            reused: true,
            source_revision: launch.source_sha.clone(),
            canonical_url: "https://github.com/contributor/project".into(),
            ready_at: "2026-08-27T00:00:00Z".into(),
        });
        launch.branch = Some(BranchRecord {
            name: provider_launch_id_branch(),
            expected_head: launch.source_sha.clone(),
            current_head: if operation == PullRequestOperation::Open {
                "d".repeat(40)
            } else {
                "e".repeat(40)
            },
            canonical_url: format!(
                "https://github.com/contributor/project/tree/{}",
                provider_launch_id_branch()
            ),
            created_at: "2026-08-27T00:00:00Z".into(),
        });
        let expected_head = if operation == PullRequestOperation::Open {
            launch.source_sha.clone()
        } else {
            "d".repeat(40)
        };
        let resulting_head = if operation == PullRequestOperation::Open {
            "d".repeat(40)
        } else {
            "e".repeat(40)
        };
        launch.contribution = Some(ContributionRecord {
            id: ContributionId::new(),
            launch_id: launch.id.clone(),
            diff_artifact_id: DiffArtifactId::new(),
            revision_digest: launch.revision_digest.clone(),
            baseline_digest: "1".repeat(64),
            checkpoint_digest: "2".repeat(64),
            diff_digest: "3".repeat(64),
            branch_name: provider_launch_id_branch(),
            state: PublicationState::Published,
            expected_head,
            base_tree_id: Some("b".repeat(40)),
            tree_id: Some("c".repeat(40)),
            commit_id: Some(resulting_head.clone()),
            resulting_head: Some(resulting_head.clone()),
            object_ids: vec!["f".repeat(40)],
            object_write: None,
            author: CommitIdentity {
                name: "Contributor".into(),
                email: "21+contributor@users.noreply.github.com".into(),
            },
            committer: CommitIdentity {
                name: "CMA".into(),
                email: "cma@chrono.ai".into(),
            },
            confirmation: Confirmation {
                action: ConfirmationAction::PublishBranch,
                subject: launch.owner.clone(),
                snapshot_digest: "3".repeat(64),
                confirmed_at: "2026-08-27T00:00:02Z".into(),
                idempotency_key_hash: "publish-confirmation".into(),
            },
            intent_key: "cma-trigger:provider:publish".into(),
            created_at: "2026-08-27T00:00:02Z".into(),
            updated_at: "2026-08-27T00:00:03Z".into(),
        });
        let draft = PullRequestDraft::seal(
            launch.id.clone(),
            operation,
            10,
            "R_upstream".into(),
            30,
            "R_fork".into(),
            "main".into(),
            "contributor".into(),
            provider_launch_id_branch(),
            launch.source_sha.clone(),
            resulting_head,
            (operation == PullRequestOperation::Update).then(|| "d".repeat(40)),
            (operation == PullRequestOperation::Update).then_some(7),
            (operation == PullRequestOperation::Update).then_some(70),
            (operation == PullRequestOperation::Update).then(|| "PR_node_7".into()),
            operation == PullRequestOperation::Update && update_metadata,
            (operation == PullRequestOperation::Update && update_metadata)
                .then(|| "CMA contribution".into()),
            (operation == PullRequestOperation::Update && update_metadata).then(|| {
                layer2_core::pull_request_body_digest(
                    "Created by CMA",
                    &layer2_core::pull_request_marker(&launch.id),
                )
            }),
            if operation == PullRequestOperation::Open || !update_metadata {
                "CMA contribution".into()
            } else {
                "Updated CMA contribution".into()
            },
            if operation == PullRequestOperation::Open || !update_metadata {
                "Created by CMA".into()
            } else {
                "Updated after a second reviewed commit".into()
            },
        )
        .unwrap();
        let confirmation = Confirmation {
            action: operation.confirmation_action(),
            subject: launch.owner.clone(),
            snapshot_digest: draft.intent_digest.clone(),
            confirmed_at: "2026-08-27T00:00:04Z".into(),
            idempotency_key_hash: "pr-confirmation".into(),
        };
        let mut record = PullRequestRecord {
            intent: draft,
            provider_id: None,
            provider_node_id: None,
            number: None,
            canonical_url: None,
            state: PullRequestState::Creating,
            base_revision: None,
            head_revision: None,
            observed_title: None,
            observed_body_digest: None,
            mergeability: Mergeability::Computing,
            checks: CheckSummaryState::Unavailable,
            confirmation,
            intent_key: String::new(),
            created_at: "2026-08-27T00:00:04Z".into(),
            updated_at: "2026-08-27T00:00:04Z".into(),
            provider_created_at: None,
            provider_updated_at: None,
            status_sync: layer2_core::PullRequestStatusSync::default(),
        };
        record.intent_key = format!(
            "cma-trigger:{}:pr:{}:{}",
            launch.id.0,
            if operation == PullRequestOperation::Open {
                "open"
            } else {
                "update"
            },
            record.intent.intent_digest
        );
        if operation == PullRequestOperation::Update {
            record.provider_id = Some(70);
            record.provider_node_id = Some("PR_node_7".into());
            record.number = Some(7);
            record.canonical_url = Some("https://github.com/upstream/project/pull/7".into());
            record.state = PullRequestState::Updating;
            record.base_revision = Some(launch.source_sha.clone());
            record.head_revision = Some("d".repeat(40));
            record.observed_title = Some("CMA contribution".into());
            record.observed_body_digest = Some("4".repeat(64));
            record.provider_created_at = Some("2026-08-27T00:00:03Z".into());
            record.provider_updated_at = Some("2026-08-27T00:00:03Z".into());
        }
        record.validate_intent(&launch.id).unwrap();
        launch.pull_request = Some(record);
        launch.state = layer2_core::LaunchState::PrPending;
        launch
    }

    fn assert_write_boundary_preflight(requests: &[RecordedRequest], write_index: usize) {
        let boundary = &requests[write_index - 7..write_index];
        assert_eq!(boundary[0].uri, "/api/v1/keys/service-1/authorization");
        assert_eq!(boundary[1].uri, "/api/v1/user-services");
        assert!(boundary[2]
            .uri
            .starts_with("/api/v1/proxy/123e4567-e89b-42d3-a456-426614174200/user?"));
        assert!(boundary[3]
            .uri
            .starts_with("/api/v1/proxy/123e4567-e89b-42d3-a456-426614174200/repositories/10?"));
        assert!(boundary[4]
            .uri
            .starts_with("/api/v1/proxy/123e4567-e89b-42d3-a456-426614174200/repositories/30?"));
        assert!(boundary[5].uri.contains("/git/ref/heads/main?"));
        assert!(boundary[6]
            .uri
            .contains(format!("/git/ref/heads/{}?", provider_launch_id_branch()).as_str()));
        assert!(boundary.iter().all(|request| request.method == "GET"));
    }

    async fn assert_rebound_authority_blocks_mutation(
        operation: PullRequestOperation,
        rebound_authority: ReboundAuthority,
    ) {
        let update = operation == PullRequestOperation::Update;
        let launch = provider_launch_with_metadata_update(operation, update);
        let marker = launch.pull_request.as_ref().unwrap().intent.marker.clone();
        let fixture = ProviderFixture {
            branch_head: if update {
                "e".repeat(40)
            } else {
                "d".repeat(40)
            },
            rebound_authority: rebound_authority.clone(),
            ..ProviderFixture::default()
        };
        if update {
            *fixture.pull_request.lock().await = Some(ProviderPullRequest {
                title: "CMA contribution".into(),
                body: format!("Created by CMA\n\n{marker}"),
                state: "open".into(),
                merged_at: None,
                head_sha: "e".repeat(40),
                mergeable: Some(true),
            });
        }
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();

        let result = provider_broker(&base_url)
            .write_pull_request(&context)
            .await;
        server.abort();

        assert!(
            matches!(
                result,
                Err(GithubBrokerError::AuthorityDrift)
                    | Err(GithubBrokerError::ConnectionInactive)
                    | Err(GithubBrokerError::UnapprovedScope { .. })
            ),
            "{operation:?} with {rebound_authority:?} returned {result:?}"
        );
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| !matches!(request.method.as_str(), "POST" | "PATCH")));
    }

    async fn assert_rebound_revision_blocks_mutation(
        operation: PullRequestOperation,
        upstream_drift: bool,
    ) {
        let update = operation == PullRequestOperation::Update;
        let launch = provider_launch_with_metadata_update(operation, update);
        let marker = launch.pull_request.as_ref().unwrap().intent.marker.clone();
        let fixture = ProviderFixture {
            branch_head: if update {
                "e".repeat(40)
            } else {
                "d".repeat(40)
            },
            rebound_base_head: upstream_drift.then(|| "9".repeat(40)),
            rebound_branch_head: (!upstream_drift).then(|| "8".repeat(40)),
            ..ProviderFixture::default()
        };
        if update {
            *fixture.pull_request.lock().await = Some(ProviderPullRequest {
                title: "CMA contribution".into(),
                body: format!("Created by CMA\n\n{marker}"),
                state: "open".into(),
                merged_at: None,
                head_sha: "e".repeat(40),
                mergeable: Some(true),
            });
        }
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();

        let result = provider_broker(&base_url)
            .write_pull_request(&context)
            .await;
        server.abort();

        if upstream_drift {
            assert!(matches!(
                result,
                Err(GithubBrokerError::UpstreamBaseConflict)
            ));
        } else {
            assert!(matches!(result, Err(GithubBrokerError::BranchHeadConflict)));
        }
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| !matches!(request.method.as_str(), "POST" | "PATCH")));
    }

    #[tokio::test]
    async fn create_and_update_revalidate_complete_authority_before_the_write() {
        for operation in [PullRequestOperation::Open, PullRequestOperation::Update] {
            for rebound_authority in [
                ReboundAuthority::ServiceInactive,
                ReboundAuthority::ServiceStateVersion(3),
                ReboundAuthority::BackingKey("key-replaced".into()),
                ReboundAuthority::GrantedScopes(vec![
                    PUBLIC_REPOSITORY_SCOPE.into(),
                    "workflow".into(),
                ]),
                ReboundAuthority::AccountIdentity {
                    id: 99,
                    node_id: "U_other".into(),
                    login: "other".into(),
                },
            ] {
                assert_rebound_authority_blocks_mutation(operation, rebound_authority).await;
            }
        }
    }

    #[tokio::test]
    async fn create_and_update_recheck_exact_revisions_at_the_write_boundary() {
        for operation in [PullRequestOperation::Open, PullRequestOperation::Update] {
            assert_rebound_revision_blocks_mutation(operation, true).await;
            assert_rebound_revision_blocks_mutation(operation, false).await;
        }
    }

    #[tokio::test]
    async fn create_uses_cross_fork_head_and_repeated_calls_return_the_marker_match() {
        let fixture = ProviderFixture::default();
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let launch = provider_launch(PullRequestOperation::Open);
        let context = PullRequestContext::try_from(&launch).unwrap();
        let broker = provider_broker(&base_url);

        let first = broker.write_pull_request(&context).await.unwrap();
        let second = broker.write_pull_request(&context).await.unwrap();
        server.abort();

        assert!(matches!(
            first,
            PullRequestAdvance::Ready(record)
                if record.state == PullRequestState::Open && record.number == Some(7)
        ));
        assert!(matches!(second, PullRequestAdvance::Ready(_)));
        let requests = requests.lock().await;
        let posts = requests
            .iter()
            .filter(|request| request.method == "POST")
            .collect::<Vec<_>>();
        assert_eq!(posts.len(), 1);
        let body = posts[0].body.as_ref().unwrap();
        assert_eq!(
            body["head"],
            format!("contributor:{}", provider_launch_id_branch())
        );
        assert_eq!(body["base"], "main");
        assert!(body["body"]
            .as_str()
            .unwrap()
            .ends_with(&layer2_core::pull_request_marker(&provider_launch_id())));
        let list = requests
            .iter()
            .find(|request| {
                request.method == "GET" && request.uri.contains("/repos/upstream/project/pulls?")
            })
            .unwrap();
        let url = Url::parse(&format!("http://fixture{}", list.uri)).unwrap();
        let query = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            query.get("head").map(|value| value.as_ref()),
            Some(format!("contributor:{}", provider_launch_id_branch()).as_str())
        );
        assert_eq!(query.get("base").map(|value| value.as_ref()), Some("main"));
        assert_eq!(query.get("state").map(|value| value.as_ref()), Some("all"));
    }

    #[tokio::test]
    async fn concurrent_confirmations_converge_on_the_single_provider_pull_request() {
        let fixture = ProviderFixture {
            create_behavior: CreateBehavior::ConcurrentDuplicate,
            create_barrier: Some(Arc::new(tokio::sync::Barrier::new(2))),
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let provider_record = Arc::clone(&fixture.pull_request);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();
        let broker = provider_broker(&base_url);

        let (first, second) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                broker.write_pull_request(&context),
                broker.write_pull_request(&context),
            )
        })
        .await
        .expect("both concurrent confirmations reach the provider");
        server.abort();

        for result in [first, second] {
            assert!(matches!(
                result,
                Ok(PullRequestAdvance::Ready(record))
                    if record.number == Some(7) && record.provider_id == Some(70)
            ));
        }
        assert!(provider_record.lock().await.is_some());
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            2,
            "both confirmations crossed the absence proof before provider uniqueness resolved them",
        );
    }

    #[tokio::test]
    async fn timeout_after_create_is_reconciled_without_a_second_post() {
        let fixture = ProviderFixture {
            create_behavior: CreateBehavior::AmbiguousApplied,
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();
        let broker = provider_broker(&base_url);

        assert!(matches!(
            broker.write_pull_request(&context).await,
            Err(GithubBrokerError::AmbiguousWrite)
        ));
        assert!(matches!(
            broker.reconcile_pull_request(&context).await.unwrap(),
            PullRequestAdvance::Ready(_)
        ));
        server.abort();
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn content_proven_duplicate_422_resolves_the_existing_pr() {
        let fixture = ProviderFixture {
            create_behavior: CreateBehavior::DuplicateApplied,
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        let result = provider_broker(&base_url)
            .write_pull_request(&context)
            .await
            .unwrap();
        server.abort();
        assert!(matches!(
            result,
            PullRequestAdvance::Ready(record) if record.number == Some(7)
        ));
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn unrelated_422_remains_a_terminal_provider_validation() {
        let fixture = ProviderFixture {
            create_behavior: CreateBehavior::ValidationRejected,
            ..ProviderFixture::default()
        };
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        let result = provider_broker(&base_url)
            .write_pull_request(&context)
            .await;
        server.abort();
        assert!(matches!(
            result,
            Err(GithubBrokerError::Rejected {
                status: 422,
                kind: GithubRejectionKind::Validation,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn branch_protection_refusal_is_not_flattened_into_generic_forbidden() {
        let fixture = ProviderFixture {
            create_behavior: CreateBehavior::BranchProtected,
            ..ProviderFixture::default()
        };
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        let result = provider_broker(&base_url)
            .write_pull_request(&context)
            .await;
        server.abort();

        assert!(matches!(
            result,
            Err(GithubBrokerError::Rejected {
                status: 403,
                kind: GithubRejectionKind::BranchProtection,
                provider_code: Some(code),
            }) if code == "protected_branch"
        ));
    }

    #[tokio::test]
    async fn mergeability_null_stays_computing_after_the_bounded_poll_budget() {
        let launch = provider_launch(PullRequestOperation::Open);
        let intent = &launch.pull_request.as_ref().unwrap().intent;
        let fixture = ProviderFixture::default();
        *fixture.pull_request.lock().await = Some(ProviderPullRequest {
            title: intent.title.clone(),
            body: intent.provider_body(),
            state: "open".into(),
            merged_at: None,
            head_sha: "d".repeat(40),
            mergeable: None,
        });
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();

        let result = provider_broker(&base_url)
            .write_pull_request(&context)
            .await
            .unwrap();
        server.abort();

        assert!(matches!(
            result,
            PullRequestAdvance::Ready(record)
                if record.mergeability == Mergeability::Computing
        ));
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.uri.contains("/pulls/7?"))
                .count(),
            MAX_MERGEABILITY_POLLS,
        );
    }

    #[tokio::test]
    async fn a_renamed_fork_is_readdressed_from_numeric_identity() {
        let fixture = ProviderFixture {
            fork_name: "project-renamed".into(),
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        assert!(matches!(
            provider_broker(&base_url)
                .write_pull_request(&context)
                .await
                .unwrap(),
            PullRequestAdvance::Ready(_)
        ));
        server.abort();
        assert!(requests.lock().await.iter().any(|request| {
            request.uri.contains(
                format!(
                    "/repos/contributor/project-renamed/git/ref/heads/{}",
                    provider_launch_id_branch()
                )
                .as_str(),
            )
        }));
    }

    #[tokio::test]
    async fn create_rebinds_both_names_between_discovery_and_post() {
        let fixture = ProviderFixture {
            rebound_authority: ReboundAuthority::AccountLogin("contributor-renamed".into()),
            rebound_upstream_name: Some("project-renamed".into()),
            rebound_fork_name: Some("fork-renamed".into()),
            rebound_fork_owner_login: Some("contributor-renamed".into()),
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        assert!(matches!(
            provider_broker(&base_url)
                .write_pull_request(&context)
                .await,
            Ok(PullRequestAdvance::Ready(_))
        ));
        server.abort();

        let requests = requests.lock().await;
        let post_index = requests
            .iter()
            .position(|request| request.method == "POST")
            .expect("pull-request create");
        assert!(requests[post_index]
            .uri
            .contains("/repos/upstream/project-renamed/pulls"));
        assert_eq!(
            requests[post_index].body.as_ref().unwrap()["head"],
            format!("contributor-renamed:{}", provider_launch_id_branch())
        );
        assert_write_boundary_preflight(&requests, post_index);
    }

    #[tokio::test]
    async fn create_rejects_fork_transfer_between_discovery_and_post() {
        let fixture = ProviderFixture {
            rebound_fork_owner_id: Some(99),
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        assert!(matches!(
            provider_broker(&base_url)
                .write_pull_request(&context)
                .await,
            Err(GithubBrokerError::AuthorityDrift)
        ));
        server.abort();
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method != "POST"));
    }

    #[tokio::test]
    async fn ambiguous_create_reconciliation_resolves_numeric_names_again() {
        let fixture = ProviderFixture {
            create_behavior: CreateBehavior::AmbiguousApplied,
            rebound_upstream_name: Some("project-renamed".into()),
            rebound_fork_name: Some("fork-renamed".into()),
            rebound_fork_owner_login: Some("contributor-renamed".into()),
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();
        let broker = provider_broker(&base_url);

        assert!(matches!(
            broker.write_pull_request(&context).await,
            Err(GithubBrokerError::AmbiguousWrite)
        ));
        assert!(matches!(
            broker.reconcile_pull_request(&context).await,
            Ok(PullRequestAdvance::Ready(_))
        ));
        server.abort();

        let requests = requests.lock().await;
        let post_index = requests
            .iter()
            .position(|request| request.method == "POST")
            .unwrap();
        let reconciliation = &requests[post_index + 1..];
        let upstream_lookup = reconciliation
            .iter()
            .position(|request| request.uri.contains("/repositories/10?"))
            .expect("reconciliation upstream numeric lookup");
        let fork_lookup = reconciliation
            .iter()
            .position(|request| request.uri.contains("/repositories/30?"))
            .expect("reconciliation fork numeric lookup");
        let discovery = reconciliation
            .iter()
            .position(|request| {
                request
                    .uri
                    .contains("/repos/upstream/project-renamed/pulls?")
            })
            .expect("reconciliation discovery");
        assert!(upstream_lookup < fork_lookup && fork_lookup < discovery);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn closed_and_merged_pull_requests_are_returned_without_mutation() {
        for (provider_state, merged_at, expected) in [
            ("closed", None, PullRequestState::ClosedUnmerged),
            (
                "closed",
                Some("2026-08-27T00:10:00Z".to_string()),
                PullRequestState::Merged,
            ),
        ] {
            let launch = provider_launch(PullRequestOperation::Open);
            let intent = &launch.pull_request.as_ref().unwrap().intent;
            let fixture = ProviderFixture::default();
            *fixture.pull_request.lock().await = Some(ProviderPullRequest {
                title: intent.title.clone(),
                body: intent.provider_body(),
                state: provider_state.into(),
                merged_at,
                head_sha: "d".repeat(40),
                mergeable: Some(true),
            });
            let requests = Arc::clone(&fixture.requests);
            let (base_url, server) = provider_server(fixture).await;
            let context = PullRequestContext::try_from(&launch).unwrap();

            let result = provider_broker(&base_url)
                .write_pull_request(&context)
                .await
                .unwrap();
            server.abort();
            assert!(matches!(
                result,
                PullRequestAdvance::Ready(record) if record.state == expected
            ));
            assert!(requests
                .lock()
                .await
                .iter()
                .all(|request| !matches!(request.method.as_str(), "POST" | "PATCH")));
        }
    }

    #[tokio::test]
    async fn ordinary_update_preserves_provider_metadata_without_a_patch() {
        let launch = provider_launch(PullRequestOperation::Update);
        let marker = launch.pull_request.as_ref().unwrap().intent.marker.clone();
        let fixture = ProviderFixture {
            branch_head: "e".repeat(40),
            ..ProviderFixture::default()
        };
        *fixture.pull_request.lock().await = Some(ProviderPullRequest {
            title: "Maintainer-owned title".into(),
            body: format!("Maintainer notes\n\n{marker}"),
            state: "open".into(),
            merged_at: None,
            head_sha: "e".repeat(40),
            mergeable: Some(true),
        });
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();

        let result = provider_broker(&base_url)
            .write_pull_request(&context)
            .await
            .unwrap();
        server.abort();

        assert!(matches!(
            result,
            PullRequestAdvance::Ready(record)
                if record.observed_title.as_deref() == Some("Maintainer-owned title")
        ));
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "PATCH")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn update_rebinds_numeric_names_immediately_before_patch() {
        let launch = provider_launch_with_metadata_update(PullRequestOperation::Update, true);
        let marker = launch.pull_request.as_ref().unwrap().intent.marker.clone();
        let fixture = ProviderFixture {
            branch_head: "e".repeat(40),
            rebound_authority: ReboundAuthority::AccountLogin("contributor-renamed".into()),
            rebound_upstream_name: Some("project-renamed".into()),
            rebound_fork_name: Some("fork-renamed".into()),
            rebound_fork_owner_login: Some("contributor-renamed".into()),
            ..ProviderFixture::default()
        };
        *fixture.pull_request.lock().await = Some(ProviderPullRequest {
            title: "CMA contribution".into(),
            body: format!("Created by CMA\n\n{marker}"),
            state: "open".into(),
            merged_at: None,
            head_sha: "e".repeat(40),
            mergeable: Some(true),
        });
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();

        assert!(matches!(
            provider_broker(&base_url)
                .write_pull_request(&context)
                .await,
            Ok(PullRequestAdvance::Ready(_))
        ));
        server.abort();

        let requests = requests.lock().await;
        let patch_index = requests
            .iter()
            .position(|request| request.method == "PATCH")
            .expect("pull-request update");
        assert!(requests[patch_index]
            .uri
            .contains("/repos/upstream/project-renamed/pulls/7"));
        assert_write_boundary_preflight(&requests, patch_index);
    }

    #[tokio::test]
    async fn update_rejects_transfer_between_discovery_and_patch() {
        let launch = provider_launch_with_metadata_update(PullRequestOperation::Update, true);
        let marker = launch.pull_request.as_ref().unwrap().intent.marker.clone();
        let fixture = ProviderFixture {
            branch_head: "e".repeat(40),
            rebound_fork_owner_id: Some(99),
            ..ProviderFixture::default()
        };
        *fixture.pull_request.lock().await = Some(ProviderPullRequest {
            title: "CMA contribution".into(),
            body: format!("Created by CMA\n\n{marker}"),
            state: "open".into(),
            merged_at: None,
            head_sha: "e".repeat(40),
            mergeable: Some(true),
        });
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();

        assert!(matches!(
            provider_broker(&base_url)
                .write_pull_request(&context)
                .await,
            Err(GithubBrokerError::AuthorityDrift)
        ));
        server.abort();
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method != "PATCH"));
    }

    #[tokio::test]
    async fn ambiguous_explicit_metadata_update_reconciles_without_a_second_patch() {
        let launch = provider_launch_with_metadata_update(PullRequestOperation::Update, true);
        let marker = launch.pull_request.as_ref().unwrap().intent.marker.clone();
        let fixture = ProviderFixture {
            branch_head: "e".repeat(40),
            patch_ambiguous_once: Arc::new(AtomicBool::new(true)),
            ..ProviderFixture::default()
        };
        *fixture.pull_request.lock().await = Some(ProviderPullRequest {
            title: "CMA contribution".into(),
            body: format!("Created by CMA\n\n{marker}"),
            state: "open".into(),
            merged_at: None,
            head_sha: "e".repeat(40),
            mergeable: Some(true),
        });
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();
        let broker = provider_broker(&base_url);

        assert!(matches!(
            broker.write_pull_request(&context).await,
            Err(GithubBrokerError::AmbiguousWrite)
        ));
        assert!(matches!(
            broker.reconcile_pull_request(&context).await.unwrap(),
            PullRequestAdvance::Ready(record)
                if record.head_revision.as_deref() == Some("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
        ));
        server.abort();
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "PATCH")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn unapplied_ambiguous_update_recovery_never_sends_a_second_patch() {
        let launch = provider_launch_with_metadata_update(PullRequestOperation::Update, true);
        let marker = launch.pull_request.as_ref().unwrap().intent.marker.clone();
        let fixture = ProviderFixture {
            branch_head: "e".repeat(40),
            patch_unapplied_ambiguous_once: Arc::new(AtomicBool::new(true)),
            ..ProviderFixture::default()
        };
        *fixture.pull_request.lock().await = Some(ProviderPullRequest {
            title: "CMA contribution".into(),
            body: format!("Created by CMA\n\n{marker}"),
            state: "open".into(),
            merged_at: None,
            head_sha: "e".repeat(40),
            mergeable: Some(true),
        });
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();
        let broker = provider_broker(&base_url);

        assert!(matches!(
            broker.write_pull_request(&context).await,
            Err(GithubBrokerError::AmbiguousWrite)
        ));
        assert_eq!(
            broker.reconcile_pull_request(&context).await.unwrap(),
            PullRequestAdvance::Absent
        );
        server.abort();
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "PATCH")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn explicit_metadata_update_refuses_to_overwrite_a_concurrent_edit() {
        let launch = provider_launch_with_metadata_update(PullRequestOperation::Update, true);
        let marker = launch.pull_request.as_ref().unwrap().intent.marker.clone();
        let fixture = ProviderFixture {
            branch_head: "e".repeat(40),
            ..ProviderFixture::default()
        };
        *fixture.pull_request.lock().await = Some(ProviderPullRequest {
            title: "Maintainer changed this".into(),
            body: format!("Created by CMA\n\n{marker}"),
            state: "open".into(),
            merged_at: None,
            head_sha: "e".repeat(40),
            mergeable: Some(true),
        });
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context = PullRequestContext::try_from(&launch).unwrap();

        assert!(matches!(
            provider_broker(&base_url)
                .write_pull_request(&context)
                .await,
            Err(GithubBrokerError::PullRequestChanged)
        ));
        server.abort();
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "PATCH")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn manual_branch_change_stops_before_pr_create() {
        let fixture = ProviderFixture {
            branch_head: "f".repeat(40),
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        assert!(matches!(
            provider_broker(&base_url)
                .write_pull_request(&context)
                .await,
            Err(GithubBrokerError::BranchHeadConflict)
        ));
        server.abort();
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method != "POST"));
    }

    #[tokio::test]
    async fn upstream_base_change_stops_before_any_pull_request_write() {
        let fixture = ProviderFixture {
            base_head: "f".repeat(40),
            ..ProviderFixture::default()
        };
        let requests = Arc::clone(&fixture.requests);
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        assert!(matches!(
            provider_broker(&base_url)
                .write_pull_request(&context)
                .await,
            Err(GithubBrokerError::UpstreamBaseConflict)
        ));
        server.abort();
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| !matches!(request.method.as_str(), "POST" | "PATCH")));
    }

    #[tokio::test]
    async fn maintainer_workflow_approval_is_a_distinct_provider_status() {
        let fixture = ProviderFixture {
            workflow_approval_pending: true,
            ..ProviderFixture::default()
        };
        let (base_url, server) = provider_server(fixture).await;
        let context =
            PullRequestContext::try_from(&provider_launch(PullRequestOperation::Open)).unwrap();

        let result = provider_broker(&base_url)
            .write_pull_request(&context)
            .await
            .unwrap();
        server.abort();
        assert!(matches!(
            result,
            PullRequestAdvance::Ready(record)
                if record.checks == CheckSummaryState::AwaitingMaintainerApproval
        ));
    }

    #[test]
    fn workflow_approval_is_not_flattened_into_a_retryable_check_failure() {
        assert_eq!(
            summarize_checks(
                vec![CheckRunWire {
                    status: "queued".into(),
                    conclusion: None,
                }],
                vec![WorkflowRunWire {
                    status: "completed".into(),
                    conclusion: Some("action_required".into()),
                }],
            ),
            CheckSummaryState::AwaitingMaintainerApproval
        );
    }

    fn passing_check_runs(total_count: u64, returned_count: usize) -> CheckRunsWire {
        CheckRunsWire {
            total_count,
            check_runs: (0..returned_count)
                .map(|_| CheckRunWire {
                    status: "completed".into(),
                    conclusion: Some("success".into()),
                })
                .collect(),
        }
    }

    fn passing_workflow_runs(total_count: u64, returned_count: usize) -> WorkflowRunsWire {
        WorkflowRunsWire {
            total_count,
            workflow_runs: (0..returned_count)
                .map(|_| WorkflowRunWire {
                    status: "completed".into(),
                    conclusion: Some("success".into()),
                })
                .collect(),
        }
    }

    fn check_source_failure<T>() -> Result<T, GithubBrokerError> {
        Err(GithubBrokerError::Malformed(
            "simulated check-source failure".into(),
        ))
    }

    #[test]
    fn complete_check_sources_can_report_passing() {
        assert_eq!(
            summarize_check_sources(
                Ok(passing_check_runs(1, 1)),
                Ok(passing_workflow_runs(1, 1)),
            ),
            CheckSummaryState::Passing
        );
    }

    #[test]
    fn truncated_or_inconsistent_check_sources_are_unavailable() {
        for (checks, workflows) in [
            (
                passing_check_runs(CHECK_STATUS_PAGE_SIZE + 1, CHECK_STATUS_PAGE_SIZE as usize),
                passing_workflow_runs(0, 0),
            ),
            (
                passing_check_runs(0, 0),
                passing_workflow_runs(CHECK_STATUS_PAGE_SIZE + 1, CHECK_STATUS_PAGE_SIZE as usize),
            ),
            (passing_check_runs(2, 1), passing_workflow_runs(0, 0)),
            (passing_check_runs(0, 0), passing_workflow_runs(0, 1)),
        ] {
            assert_eq!(
                summarize_check_sources(Ok(checks), Ok(workflows)),
                CheckSummaryState::Unavailable
            );
        }
    }

    #[test]
    fn any_check_source_failure_is_unavailable() {
        assert_eq!(
            summarize_check_sources(check_source_failure(), Ok(passing_workflow_runs(1, 1)),),
            CheckSummaryState::Unavailable
        );
        assert_eq!(
            summarize_check_sources(Ok(passing_check_runs(1, 1)), check_source_failure()),
            CheckSummaryState::Unavailable
        );
        assert_eq!(
            summarize_check_sources(check_source_failure(), check_source_failure()),
            CheckSummaryState::Unavailable
        );
    }

    #[test]
    fn check_source_total_counts_are_required_on_the_wire() {
        assert!(serde_json::from_value::<CheckRunsWire>(json!({ "check_runs": [] })).is_err());
        assert!(
            serde_json::from_value::<WorkflowRunsWire>(json!({ "workflow_runs": [] })).is_err()
        );
    }

    #[test]
    fn hidden_marker_must_occur_exactly_once() {
        let marker = "<!-- cma-trigger:launch:one:v1 -->";
        assert_eq!(marker_count(&format!("body\n\n{marker}"), marker), 1);
        assert_eq!(marker_count(&format!("{marker}\n{marker}"), marker), 2);
    }

    #[test]
    fn provider_body_digest_covers_the_hidden_marker() {
        let marker = "<!-- cma-trigger:launch:one:v1 -->";
        let rendered = format!("body\n\n{marker}");
        assert_eq!(
            layer2_core::pull_request_body_digest("body", marker),
            format!("{:x}", Sha256::digest(rendered.as_bytes()))
        );
    }
}
