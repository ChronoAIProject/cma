//! Resolve scoped retained authority through the published runtime catalog.
use std::sync::Arc;

use layer1_public::rest::{
    retained_runtime_transition::{
        RetainedRuntimeAbort, RetainedRuntimeGrantRequest, RetainedRuntimeGrantView,
        RetainedRuntimePreview, RetainedRuntimeProposal, RetainedRuntimeReceipt,
        RetainedRuntimeRedemption, RetainedRuntimeTransitions,
    },
    runtime_canary::{CanaryPolicyRequest, CanaryPolicyView, CanaryRefusal},
};
use layer2_core::{
    sandbox::{
        store::{valid_admission_fingerprint, SandboxStore, SandboxStoreError},
        RetainedRuntimeTransitionRequest, Sandbox, SandboxStatus,
    },
    SubjectId,
};
use layer3_adapter::SandboxAdapter;
use mongodb::{bson::DateTime, Database};

use crate::{
    config::{runtime_canary, RuntimeConfig},
    store::{
        runtime_canary::CanaryPolicy,
        sandbox_maintenance::retained_transition::{
            RetainedRuntimeGrant, RetainedRuntimeGrantPolicy,
        },
        MongoSandboxStore,
    },
};

pub(crate) struct RetainedRuntimeService {
    store: Arc<MongoSandboxStore>,
    database: Database,
    adapter: Arc<dyn SandboxAdapter>,
    catalog: runtime_canary::Catalog,
    scope: runtime_canary::WriteScope,
    catalog_sha256: String,
    scope_sha256: String,
    deployment: String,
}

impl RetainedRuntimeService {
    pub(crate) fn new(
        config: &RuntimeConfig,
        deployment: String,
        store: Arc<MongoSandboxStore>,
        database: Database,
        adapter: Arc<dyn SandboxAdapter>,
    ) -> Result<Self, String> {
        let catalog = config.runtime_canary_releases.clone();
        let scope = config.retained_runtime_writes.clone();
        Ok(Self {
            catalog_sha256: catalog.fingerprint()?,
            scope_sha256: scope.fingerprint()?,
            catalog,
            scope,
            store,
            database,
            adapter,
            deployment,
        })
    }

    fn release(&self, owner: &SubjectId, key: &str) -> Option<&runtime_canary::Release> {
        self.scope
            .allows(owner)
            .then(|| self.catalog.release(key))
            .flatten()
            .filter(|release| release.lane == runtime_canary::Lane::Agent)
    }

    async fn observe_predecessor(
        &self,
        sandbox: &Sandbox,
    ) -> Result<layer3_adapter::RetainedStartupPolicyPredecessor, CanaryRefusal> {
        if sandbox.status != SandboxStatus::Paused || sandbox.revive_eligibility().is_err() {
            return Err(CanaryRefusal::Conflict);
        }
        let binding = sandbox
            .lifecycle
            .suspended_runtime
            .as_deref()
            .ok_or(CanaryRefusal::Conflict)?;
        let request =
            super::runtime_attachment::retained_runtime_request(sandbox, binding, &self.deployment)
                .map_err(|_| CanaryRefusal::Conflict)?;
        self.adapter
            .retained_startup_policy()
            .ok_or(CanaryRefusal::Closed)?
            .inspect_retained_startup_policy(&request)
            .await
            .map_err(|error| match error {
                layer3_adapter::AdapterError::Rejected(_) => CanaryRefusal::Conflict,
                layer3_adapter::AdapterError::Unsupported(_) => CanaryRefusal::Closed,
                _ => CanaryRefusal::Unavailable,
            })
    }

    async fn plan(
        &self,
        proposal: &RetainedRuntimeProposal,
        grant_id: String,
    ) -> Result<RetainedRuntimeTransitionRequest, CanaryRefusal> {
        let release = self
            .release(&proposal.owner, &proposal.release_key)
            .ok_or(CanaryRefusal::Closed)?;
        let sandbox = self
            .store
            .get_sandbox(&proposal.sandbox_id)
            .await
            .map_err(refusal)?
            .filter(|sandbox| {
                sandbox.owner == proposal.owner && sandbox.revision == proposal.sandbox_revision
            })
            .ok_or(CanaryRefusal::Conflict)?;
        if sandbox.profile.content.cpu_millis != release.capacity.cpu_millis
            || sandbox.profile.content.memory_mib != release.capacity.memory_mib
            || !sandbox.supports_retained_runtime_update()
        {
            return Err(CanaryRefusal::Conflict);
        }
        let predecessor = self.observe_predecessor(&sandbox).await?;
        let request = RetainedRuntimeTransitionRequest {
            sandbox_id: proposal.sandbox_id.clone(),
            owner: proposal.owner.clone(),
            expected_revision: proposal.sandbox_revision,
            grant_id,
            policy_revision: proposal.policy_revision,
            predecessor,
            target_image: release.image.uri.clone(),
            target_config_digest: release.config_digest.clone(),
            journal_recovery: proposal.journal_recovery,
        };
        let mut planned = sandbox;
        // The actual admission rereads and write-fences the complete Agent and Workspace sets.
        planned
            .begin_retained_runtime_transition(request.clone(), &[], layer2_core::now_rfc3339())
            .map_err(|_| CanaryRefusal::Conflict)?;
        Ok(request)
    }

    async fn fresh_redemption_policy(
        &self,
        owner: &SubjectId,
        grant: &RetainedRuntimeGrant,
    ) -> Result<RetainedRuntimeGrantPolicy<'_>, CanaryRefusal> {
        let release = self
            .release(owner, &grant.release_key)
            .ok_or(CanaryRefusal::Closed)?;
        let sandbox = self
            .store
            .get_sandbox(&grant.request.sandbox_id)
            .await
            .map_err(refusal)?
            .filter(|sandbox| {
                sandbox.owner == *owner && sandbox.revision == grant.request.expected_revision
            })
            .ok_or(CanaryRefusal::Conflict)?;
        if self.observe_predecessor(&sandbox).await? != grant.request.predecessor {
            return Err(CanaryRefusal::Conflict);
        }
        Ok(RetainedRuntimeGrantPolicy {
            release_key: &release.release_key,
            catalog_sha256: &self.catalog_sha256,
            owner_scope_sha256: &self.scope_sha256,
            target_image: &release.image.uri,
            target_config_digest: &release.config_digest,
        })
    }

    async fn redeem_loaded_grant(
        &self,
        owner: &SubjectId,
        grant: RetainedRuntimeGrant,
    ) -> Result<RetainedRuntimeReceipt, CanaryRefusal> {
        let preflight = if grant.receipt.is_some() {
            Ok(None)
        } else {
            self.fresh_redemption_policy(owner, &grant).await.map(Some)
        };
        self.finish_redemption(grant.request, preflight).await
    }

    async fn finish_redemption(
        &self,
        request: RetainedRuntimeTransitionRequest,
        preflight: Result<Option<RetainedRuntimeGrantPolicy<'_>>, CanaryRefusal>,
    ) -> Result<RetainedRuntimeReceipt, CanaryRefusal> {
        let (current, preflight_error) = match preflight {
            Ok(policy) => (policy, None),
            Err(error) => (None, Some(error)),
        };
        // Without fresh policy the store can only read back an exact committed
        // receipt. A concurrent redemption can win after the initial grant read.
        let receipt = self
            .store
            .redeem_retained_runtime_grant(&self.database, request, current)
            .await
            .map_err(|error| match refusal(error) {
                CanaryRefusal::Conflict => preflight_error.unwrap_or(CanaryRefusal::Conflict),
                error => error,
            })?;
        Ok(RetainedRuntimeReceipt {
            operation_id: receipt.operation_id,
            successor_generation: receipt.successor_generation,
        })
    }
}

#[async_trait::async_trait]
impl RetainedRuntimeTransitions for RetainedRuntimeService {
    async fn configure(
        &self,
        request: CanaryPolicyRequest,
    ) -> Result<CanaryPolicyView, CanaryRefusal> {
        if request.open && !self.scope.enabled() {
            return Err(CanaryRefusal::Closed);
        }
        let revision = CanaryPolicy::configure_retained(
            &self.database,
            request.expected_revision,
            request.open,
            self.catalog_sha256.clone(),
            self.scope_sha256.clone(),
        )
        .await
        .map_err(|error| match error {
            crate::store::runtime_canary::CanaryWriteError::Refused => CanaryRefusal::Conflict,
            _ => CanaryRefusal::Unavailable,
        })?;
        Ok(CanaryPolicyView { revision })
    }

    async fn preview(
        &self,
        proposal: RetainedRuntimeProposal,
    ) -> Result<RetainedRuntimePreview, CanaryRefusal> {
        let plan = self.plan(&proposal, "preview".into()).await?;
        Ok(RetainedRuntimePreview {
            plan_sha256: plan_digest(&plan, &self.catalog_sha256)?,
            predecessor_image: plan.predecessor.image,
            predecessor_generation: plan.predecessor.generation,
            predecessor_journal_recovery: plan.predecessor.journal_recovery,
            target_image: plan.target_image,
            target_journal_recovery: plan.journal_recovery,
        })
    }

    async fn grant(
        &self,
        operator: &SubjectId,
        request: RetainedRuntimeGrantRequest,
    ) -> Result<RetainedRuntimeGrantView, CanaryRefusal> {
        if ![
            &request.plan_sha256,
            &request.consent_sha256,
            &request.custody_sha256,
        ]
        .into_iter()
        .all(|digest| valid_admission_fingerprint(digest))
        {
            return Err(CanaryRefusal::Invalid);
        }
        let expires_at =
            DateTime::parse_rfc3339_str(&request.expires_at).map_err(|_| CanaryRefusal::Invalid)?;
        let plan = match self
            .store
            .read_retained_runtime_grant(&request.proposal.owner, &request.grant_id)
            .await
        {
            Ok(existing) => existing.request,
            Err(SandboxStoreError::NotFound) => {
                self.plan(&request.proposal, request.grant_id.clone())
                    .await?
            }
            Err(error) => return Err(refusal(error)),
        };
        if !same_proposal(&plan, &request.proposal)
            || plan_digest(&plan, &self.catalog_sha256)? != request.plan_sha256
        {
            return Err(CanaryRefusal::Conflict);
        }
        self.store
            .issue_retained_runtime_grant(
                &self.database,
                &RetainedRuntimeGrant {
                    request: plan,
                    release_key: request.proposal.release_key,
                    catalog_sha256: self.catalog_sha256.clone(),
                    owner_scope_sha256: self.scope_sha256.clone(),
                    issued_by: operator.clone(),
                    consent_sha256: request.consent_sha256,
                    custody_sha256: request.custody_sha256,
                    expires_at,
                    receipt: None,
                    aborted_at: None,
                },
            )
            .await
            .map_err(refusal)?;
        Ok(RetainedRuntimeGrantView {
            grant_id: request.grant_id,
        })
    }

    async fn abort(
        &self,
        request: RetainedRuntimeAbort,
    ) -> Result<RetainedRuntimeGrantView, CanaryRefusal> {
        self.store
            .abort_retained_runtime_grant(&self.database, &request.owner, &request.grant_id)
            .await
            .map_err(refusal)?;
        Ok(RetainedRuntimeGrantView {
            grant_id: request.grant_id,
        })
    }

    async fn redeem(
        &self,
        owner: &SubjectId,
        redemption: RetainedRuntimeRedemption,
    ) -> Result<RetainedRuntimeReceipt, CanaryRefusal> {
        let grant = self
            .store
            .read_retained_runtime_grant(owner, &redemption.grant_id)
            .await
            .map_err(refusal)?;
        self.redeem_loaded_grant(owner, grant).await
    }
}

fn same_proposal(
    plan: &RetainedRuntimeTransitionRequest,
    proposal: &RetainedRuntimeProposal,
) -> bool {
    plan.sandbox_id == proposal.sandbox_id
        && plan.owner == proposal.owner
        && plan.expected_revision == proposal.sandbox_revision
        && plan.policy_revision == proposal.policy_revision
        && plan.journal_recovery == proposal.journal_recovery
}

fn plan_digest(
    plan: &RetainedRuntimeTransitionRequest,
    catalog: &str,
) -> Result<String, CanaryRefusal> {
    let mut plan = plan.clone();
    plan.grant_id.clear();
    layer2_core::profiles::content_digest(&("retained-runtime-plan-v1", catalog, plan))
        .map_err(|_| CanaryRefusal::Unavailable)
}

fn refusal(error: SandboxStoreError) -> CanaryRefusal {
    match error {
        SandboxStoreError::Conflict
        | SandboxStoreError::InvalidTransaction(_)
        | SandboxStoreError::NotFound => CanaryRefusal::Conflict,
        _ => CanaryRefusal::Unavailable,
    }
}

#[cfg(test)]
mod tests;
