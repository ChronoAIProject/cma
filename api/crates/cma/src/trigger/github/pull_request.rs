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
    connection_drifted, validate_commit_sha, validate_repository_url, GithubBroker,
    GithubBrokerError, GithubRejectionKind, Observed, RepositoryEvidence,
};

const MAX_PULL_REQUEST_PAGES: u32 = 10;
const PULL_REQUEST_PAGE_SIZE: usize = 100;
const MAX_MERGEABILITY_POLLS: usize = 3;

#[derive(Debug, Clone)]
pub(crate) struct PullRequestContext {
    owner: SubjectId,
    connection: ConnectionSnapshot,
    upstream: RepositoryIdentity,
    fork: ForkRecord,
    record: PullRequestRecord,
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
        Ok(Self {
            owner: launch.owner.clone(),
            connection: launch.connection.clone().ok_or_else(|| {
                GithubBrokerError::Malformed("the Launch has no selected connection".into())
            })?,
            upstream,
            fork,
            record,
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

    async fn advance_pull_request(
        &self,
        context: &PullRequestContext,
        reconcile_only: bool,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        let repositories = self.current_pull_request_repositories(context).await?;
        let base_revision = self
            .resolve_branch_sha(
                &context.owner,
                &context.connection.service_id,
                &repositories.upstream,
                &context.record.intent.base_ref,
            )
            .await?;
        let head_revision = self
            .resolve_branch_sha(
                &context.owner,
                &context.connection.service_id,
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
            .repository_by_id(
                &context.owner,
                &context.connection.service_id,
                context.upstream.repository_id,
            )
            .await?;
        let fork = self
            .repository_by_id(
                &context.owner,
                &context.connection.service_id,
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
                .github_json(
                    &context.owner,
                    &context.connection.service_id,
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
        repositories: &CurrentRepositories,
        base_revision: &str,
        head_revision: &str,
    ) -> Result<PullRequestAdvance, GithubBrokerError> {
        let head = format!(
            "{}:{}",
            repositories.fork.canonical_owner, context.record.intent.head_branch
        );
        let created: Result<Observed<PullRequestWire>, GithubBrokerError> = self
            .github_json(
                &context.owner,
                &context.connection.service_id,
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
                return match self.discover_pull_request(context, repositories).await? {
                    Some(existing) => self
                        .reconciled_record(
                            context,
                            repositories,
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
            repositories,
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
        let updated = if current_matches {
            existing
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
            let number = context
                .record
                .intent
                .target_number
                .expect("a validated update has a target number")
                .to_string();
            let response: Observed<PullRequestWire> = self
                .github_json(
                    &context.owner,
                    &context.connection.service_id,
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
            response.value
        };
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
        mut wire: PullRequestWire,
        base_revision: &str,
        head_revision: &str,
        require_intended_content: bool,
    ) -> Result<PullRequestRecord, GithubBrokerError> {
        validate_pull_request_wire(context, repositories, &wire, base_revision, head_revision)?;
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
            validate_pull_request_wire(context, repositories, &wire, base_revision, head_revision)?;
        }
        let checks = self
            .pull_request_checks(context, repositories, head_revision)
            .await;
        let mut record = context.record.clone();
        record.provider_id = Some(wire.id);
        record.provider_node_id = Some(wire.node_id.clone());
        record.number = Some(wire.number);
        record.canonical_url = Some(wire.html_url.clone());
        record.state = pull_request_state(&wire)?;
        record.base_revision = Some(base_revision.to_ascii_lowercase());
        record.head_revision = Some(head_revision.to_ascii_lowercase());
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
            .github_json(
                &context.owner,
                &context.connection.service_id,
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
            .github_json(
                &context.owner,
                &context.connection.service_id,
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
            .github_json(
                &context.owner,
                &context.connection.service_id,
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
        summarize_checks(
            checks
                .ok()
                .map(|observed| observed.value.check_runs)
                .unwrap_or_default(),
            runs.ok()
                .map(|observed| observed.value.workflow_runs)
                .unwrap_or_default(),
        )
    }
}

fn validate_current_repository(
    expected: &RepositoryIdentity,
    observed: &RepositoryEvidence,
    fork: bool,
) -> Result<(), GithubBrokerError> {
    let actual = &observed.identity;
    if actual.repository_id != expected.repository_id
        || actual.repository_node_id != expected.repository_node_id
        || actual.owner_id != expected.owner_id
        || actual.owner_node_id != expected.owner_node_id
        || actual.visibility != expected.visibility
        || actual.fork_parent_id != expected.fork_parent_id
        || actual.fork_source_id != expected.fork_source_id
        || observed.archived
        || observed.disabled
        || (fork && (actual.fork_parent_id.is_none() || actual.fork_source_id.is_none()))
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
    validate_commit_sha(&wire.head.sha)?;
    validate_commit_sha(&wire.base.sha)?;
    if wire.id == 0
        || wire.number == 0
        || wire.node_id.trim().is_empty()
        || wire.node_id.len() > 255
        || !matches_pull_request_identity(context, repositories, wire)
        || !wire.base.sha.eq_ignore_ascii_case(expected_base)
        || !wire.head.sha.eq_ignore_ascii_case(expected_head)
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
        SubjectAuthoritySource, SubjectRequestAuthority, PUBLIC_REPOSITORY_SCOPE,
    };

    #[derive(Debug, Clone, Copy)]
    enum CreateBehavior {
        Confirmed,
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

    #[derive(Clone)]
    struct ProviderFixture {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        pull_request: Arc<Mutex<Option<ProviderPullRequest>>>,
        create_behavior: CreateBehavior,
        fork_name: String,
        branch_head: String,
        base_head: String,
        patch_ambiguous_once: Arc<AtomicBool>,
        workflow_approval_pending: bool,
    }

    impl Default for ProviderFixture {
        fn default() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                pull_request: Arc::new(Mutex::new(None)),
                create_behavior: CreateBehavior::Confirmed,
                fork_name: "project".into(),
                branch_head: "d".repeat(40),
                base_head: "a".repeat(40),
                patch_ambiguous_once: Arc::new(AtomicBool::new(false)),
                workflow_approval_pending: false,
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
        let path = parts.uri.path().to_string();
        fixture.requests.lock().await.push(RecordedRequest {
            method: method.clone(),
            uri,
            body: body.clone(),
        });

        let fork_ref_path = format!(
            "/api/v1/proxy/s/api-github/repos/contributor/{}/git/ref/heads/{}",
            fixture.fork_name,
            provider_launch_id_branch()
        );
        match (method.as_str(), path.as_str()) {
            ("GET", "/api/v1/user-services") => (
                StatusCode::OK,
                Json(json!({
                    "services": [{
                        "id": "service-1",
                        "slug": "api-github",
                        "resource_uri": "https://nyxid.test/api/v1/proxy/s/api-github",
                        "api_key_id": "key-1",
                        "is_active": true,
                        "state_version": 2,
                        "credential_source": { "type": "personal" }
                    }]
                })),
            ),
            ("GET", "/api/v1/keys/service-1/authorization") => (
                StatusCode::OK,
                Json(json!({
                    "id": "service-1",
                    "api_key_id": "key-1",
                    "is_active": true,
                    "status": "active",
                    "connection_status": "active",
                    "granted_scopes": [PUBLIC_REPOSITORY_SCOPE],
                    "last_authorized_at": "2026-08-27T00:00:00Z",
                    "state_version": 2
                })),
            ),
            ("GET", "/api/v1/proxy/s/api-github/user") => (
                StatusCode::OK,
                Json(json!({
                    "id": 21,
                    "node_id": "U_contributor",
                    "login": "contributor"
                })),
            ),
            ("GET", "/api/v1/proxy/s/api-github/repositories/10") => (
                StatusCode::OK,
                Json(repository_wire(
                    10,
                    "R_upstream",
                    (20, "U_owner", "upstream"),
                    "project",
                    None,
                    false,
                )),
            ),
            ("GET", "/api/v1/proxy/s/api-github/repositories/30") => (
                StatusCode::OK,
                Json(repository_wire(
                    30,
                    "R_fork",
                    (21, "U_contributor", "contributor"),
                    &fixture.fork_name,
                    Some(10),
                    true,
                )),
            ),
            (
                "GET",
                "/api/v1/proxy/s/api-github/repos/upstream/project/git/ref/heads/main",
            ) => (
                StatusCode::OK,
                Json(ref_wire("main", &fixture.base_head)),
            ),
            ("GET", actual) if actual == fork_ref_path => (
                StatusCode::OK,
                Json(ref_wire(
                    &provider_launch_id_branch(),
                    &fixture.branch_head,
                )),
            ),
            ("GET", "/api/v1/proxy/s/api-github/repos/upstream/project/pulls") => {
                let value = fixture
                    .pull_request
                    .lock()
                    .await
                    .as_ref()
                    .map(|pull_request| vec![pull_request_wire(&fixture, pull_request)])
                    .unwrap_or_default();
                (StatusCode::OK, Json(Value::Array(value)))
            }
            ("GET", "/api/v1/proxy/s/api-github/repos/upstream/project/pulls/7") => {
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
            ("POST", "/api/v1/proxy/s/api-github/repos/upstream/project/pulls") => {
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
                let body = body.expect("create request body");
                let pull_request = ProviderPullRequest {
                    title: body["title"].as_str().unwrap().to_string(),
                    body: body["body"].as_str().unwrap().to_string(),
                    state: "open".into(),
                    merged_at: None,
                    head_sha: fixture.branch_head.clone(),
                    mergeable: Some(true),
                };
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
                }
            }
            ("PATCH", "/api/v1/proxy/s/api-github/repos/upstream/project/pulls/7") => {
                let body = body.expect("update request body");
                let mut pull_request = fixture.pull_request.lock().await;
                let pull_request = pull_request.as_mut().expect("existing PR");
                pull_request.title = body["title"].as_str().unwrap().to_string();
                pull_request.body = body["body"].as_str().unwrap().to_string();
                let response = pull_request_wire(&fixture, pull_request);
                if fixture
                    .patch_ambiguous_once
                    .swap(false, Ordering::SeqCst)
                {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "message": "response lost after update" })),
                    )
                } else {
                    (StatusCode::OK, Json(response))
                }
            }
            (
                "GET",
                "/api/v1/proxy/s/api-github/repos/upstream/project/commits/dddddddddddddddddddddddddddddddddddddddd/check-runs",
            )
            | (
                "GET",
                "/api/v1/proxy/s/api-github/repos/upstream/project/commits/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee/check-runs",
            ) => (StatusCode::OK, Json(json!({ "total_count": 0, "check_runs": [] }))),
            ("GET", "/api/v1/proxy/s/api-github/repos/upstream/project/actions/runs") => {
                let runs = if fixture.workflow_approval_pending {
                    vec![json!({
                        "status": "completed",
                        "conclusion": "action_required"
                    })]
                } else {
                    Vec::new()
                };
                (StatusCode::OK, Json(json!({ "workflow_runs": runs })))
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
        json!({
            "id": 70,
            "node_id": "PR_node_7",
            "number": 7,
            "html_url": "https://github.com/upstream/project/pull/7",
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
                    "name": fixture.fork_name,
                    "owner": {
                        "id": 21,
                        "node_id": "U_contributor",
                        "login": "contributor"
                    }
                }
            },
            "base": {
                "ref": "main",
                "sha": "a".repeat(40),
                "repo": {
                    "id": 10,
                    "node_id": "R_upstream",
                    "name": "project",
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

    async fn provider_server(fixture: ProviderFixture) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .fallback(any(provider_answer))
            .with_state(fixture);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), server)
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
